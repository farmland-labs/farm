# ADR 0001 — Run Directory Retention and History

- **Status:** Accepted
- **Date:** 2026-08-03
- **Deciders:** haai

## Context

Running the same goal twice locally destroys the first run's output. There is no way
to compare a failing run against the previous, working one.

The cause sits at two levels:

1. **Log files are truncated.** `create_log_file` opens with `.truncate(true)`
   ([src/build_logger.rs:256-264](../../src/build_logger.rs#L256-L264)).
2. **The entire run directory is reused.** Locally, `effective_build_id` falls back to the
   goal name ([src/engine.rs:141-144](../../src/engine.rs#L141-L144)), so every run of a goal
   writes into the same `.farm/run/{goal}/`. `manifest.json`, `work.json`, `in/` and `out/`
   are overwritten alongside the log.

The second point matters more than the first. A log without its manifest often does not
explain a failure, so fixing only the logger would solve half the problem.

This is specifically a **local-dev** defect: CI sets a unique `FARM_BUILD_ID`, so run
directories there are already distinct. CI has the mirror-image problem instead — run
directories accumulate indefinitely because nothing prunes them.

A related constraint shapes the solution. `.farm/INTERNAL-DO-NOT-RELY.md` already states
that the directory layout is an internal implementation detail. Honouring that contract
means we owe no path stability to users, which removes the main obstacle to restructuring.

## Decision

### 1. Preserve the whole run directory, not just logs

Retention operates on `.farm/run/{build_id}/` as a unit: `log/`, `manifest.json`,
`work.json`, `in/`, `out/`.

### 2. Keep `build_id` as the directory key; change only the local default

`build_id` is a genuine user-facing handle — `--build-id`, `FARM_BUILD_ID`, and `farm ctx`
all share it, and in CI a single `build_id` may legitimately span multiple goals. The
`.farm/run/{build_id}/` layout therefore stays as it is.

Only the local fallback changes, at [src/engine.rs:141-144](../../src/engine.rs#L141-L144):

```
{goal}                    ->  {goal}-{YYYYMMDDTHHMMSS.mmmZ}-{suffix}
```

Millisecond precision plus a short suffix (pid or random) so that parallel runs of the same
goal cannot collide.

### 3. No symlinks

No `latest`, no `prev`. Windows junctions require either elevated privileges or developer
mode, and a stable pointer would recreate exactly the path dependency that decision 5
removes.

### 4. Discovery reads metadata, never filenames

`manifest.json` already contains `build_id`, `goal`, `variant`, `success`, `duration_ms`,
`created_at` and `schema_version`. Pruning and the CLI read those fields. Nothing parses
directory names, so the naming scheme remains free to change.

**Required change:** the manifest is currently written only after a run completes
([src/engine.rs:243](../../src/engine.rs#L243)), leaving in-flight and crashed runs with no
metadata at all. The manifest is therefore written **twice**: a stub at run start with
`status: "running"`, rewritten at completion with `status: "ok" | "fail"` and the results.
This gives three things at once — discovery of the currently running build (so
`farm log --follow` works), a way for pruning to distinguish a crashed run from an active
one, and a `running` state for `farm log --list`.

### 5. Access through the CLI only, via a new `farm log` porcelain

The on-disk layout stays internal. Anything users need is exposed as a command:

| Command | Purpose |
| --- | --- |
| `farm log [<goal>]` | Show the latest log; no goal means the most recent run of any goal (`--variant`, `--follow`) |
| `farm log <goal> --list` | Recent runs: time, status, duration, mode |
| `farm log <goal> --diff [--against N]` | Diff latest against the previous run |
| `farm clean --runs [--all]` | Manual prune |

`--path` exists as a deliberate escape hatch and prints a warning that the path is unstable
and must not be scripted against.

**Why a new top-level command rather than an existing namespace.** `farm run <target>` is
already the execute shortcut with a required positional
([src/bin/farm/cli.rs:119-120](../../src/bin/farm/cli.rs#L119-L120)), so `farm run list`
cannot exist — clap would bind `list` to `target`. The other candidate, extending
`farm ctx run init|show|list` ([src/bin/farm/cmd/ctx.rs:78](../../src/bin/farm/cmd/ctx.rs#L78)),
was rejected: that namespace is **plumbing** — build_id-keyed, `--format json`, designed for
tooling to consume — whereas reading your own last build is **porcelain**: goal-keyed,
human-readable, and unrelated to contexts. The split mirrors `git log` versus
`git rev-list`. Routing the everyday case through `farm ctx run log --goal test` would make
the most common action the most verbose and drag context semantics into a question that has
none.

**Shared resolver.** Both front doors sit on one module: *given a goal and variant, return
the matching run directories newest-first, from manifest metadata*. No duplicated logic.

**`farm replay` must use it too.** `farm replay <build_dir>` takes a build-id positional
([src/bin/farm/cli.rs:279-281](../../src/bin/farm/cli.rs#L279-L281)). Today `farm replay test`
works *only because* `build_id` happens to equal the goal name; unique ids break it. This is
exactly the class of breakage accepted for filesystem paths but explicitly **not** for
commands. `farm replay` therefore accepts a goal name and resolves to the latest run, with
explicit build ids continuing to work unchanged.

### 6. Retention policy

Hardcoded defaults initially. A `Farmfile` configuration block is deferred until there is
demand for it.

1. Keep the **last 5** runs per `goal` + `variant`.
2. Discard anything older than **7 days**.
3. **Always keep the newest failure**, even when rule 1 would evict it. The common pattern
   is one failure followed by several exploratory reruns; plain keep-N discards precisely
   the run worth reading.
4. *(Phase 2)* Collapse adjacent byte-identical logs, so repeated no-op runs evict nothing.

Pruning runs at the start of a build, scoped to the matching `goal` + `variant` group: one
`read_dir`, no global sweep, no background process. Runs with an explicit `--build-id` prune
under the same rules — exempting them would leave CI unbounded, which is one of the problems
being fixed. A run whose manifest says `running` is never pruned; a manifest-less directory
is treated as crashed and pruned by mtime.

### 7. Diffing: `similar`, with read-side normalization

Diffs are produced in-process with the [`similar`](https://crates.io/crates/similar) crate
(`TextDiff::from_lines` plus unified output). No shelling out to `diff` — it does not exist
on Windows by default, and farm already ships a Windows target. `similar` is
dependency-light, MIT, and offers optional word-level inline highlighting; `colored` is
already a dependency, so coloured output costs a few lines. Exact feature flags to be
confirmed at implementation.

Comparison happens after a **read-side normalization layer**, on by default, bypassed with
`--raw`:

- Drop `timestamp_start`, `timestamp_end` and `duration_ms` from the `[farm]` framing.
  `exit_code` is kept — it is signal, not noise.
- Normalize absolute paths containing the build id.
- Collapse carriage returns: within a line, keep only the text following the last `\r`.
- `--only-output` additionally strips the `[farm]` framing entirely.

Without this, every diff opens with guaranteed-different framing lines that the reader has
to scroll past, and progress-bar output reports as changed on every run.

Normalization is strictly **read-side**. Log files are written byte-faithful; see the
alternatives section for why write-time collapsing was rejected.

### 8. Interactive (PTY) runs

Retention treats PTY runs like any other — no special case, they count against keep-N.
Execution mode is not a retention concern.

The manifest does record `interactive: true|false`, for two reasons. A PTY exposes a single
stream, so stdout and stderr are merged and `--split-streams` is rejected in interactive
mode ([src/cmd.rs:279-285](../../src/cmd.rs#L279-L285)); an interactive run and a CI run of
the same goal therefore produce structurally different logs. `farm log --list` shows the
mode, and `farm log --diff` warns when comparing across modes rather than emitting a wall of
meaningless changes.

## Consequences

### Positive

- Diffing a failing run against the previous one becomes possible, with `manifest.json` and
  `work.json` preserved alongside the log rather than only the log itself.
- CI gains pruning it does not currently have.
- `in/` and `out/` become genuinely per-run. The doc comment at
  [src/context/mod.rs:358-362](../../src/context/mod.rs#L358-L362) already claims this
  isolation; with `build_id = goal` it was never actually true.
- The internal-layout contract is enforced rather than merely documented, so future layout
  changes stay cheap.

### Negative

- **Disk usage.** `in/` and `out/` are the bulk of a run directory, and keep-5 multiplies
  them. This is the most likely source of surprise. Release valve if it bites: tiered
  retention — strip `in/`/`out/` from older runs while keeping `log/` and `manifest.json`.
- `farm ctx list` ([src/bin/farm/cmd/ctx.rs:415](../../src/bin/farm/cmd/ctx.rs#L415)) will
  show many more entries. It needs to group or filter by goal, and its relationship to
  `farm log --list` should be settled (see open questions).
- Anyone who has informally scripted against `.farm/run/{goal}/log/…` breaks. This is
  sanctioned by the existing internal-use notice, but it will still be noticed.
- Writing the manifest twice adds an I/O round trip per run — negligible against build cost.
- One new dependency (`similar`). `uuid` is already present with the `v7` feature, so the
  collision suffix on generated build ids needs nothing new.
- Read-side normalization means `farm log` output is not byte-identical to the file on disk.
  `--raw` covers the case where that matters, but it is a surprise worth documenting in
  `--help`.

### Migration

None. Pre-existing `.farm/run/{goal}/` directories lack the new metadata; they age out under
the max-age sweep or are ignored. No compatibility is owed for an internal layout.

## Alternatives considered

**Two-slot rotation in the logger** (`x.log` → `x.log.prev` before truncating). Roughly
fifteen lines, no layout change, disk bounded at 2×. Rejected because it only rotates the
log, leaving `manifest.json` and `work.json` clobbered, and it offers exactly one step of
history.

**Numbered rotation N deep** (`.log.1` … `.log.N`, logrotate style). Rejected: the meaning of
`.log.2` shifts on every run, so a path noted down or shared with a colleague silently comes
to refer to something else.

**Archive-aside hybrid.** Keep `.farm/run/{build_id}/` as a stable "latest" path and move the
previous run's small diagnostic set into `.farm/run/.history/{build_id}/{timestamp}/`. This
was the leading candidate while path stability was assumed to be a requirement. Once the
internal-layout contract was affirmed, it became pure overhead — an extra move per run and a
second layout to maintain, in service of a guarantee we do not offer.

**Content-addressed by target key** (name logs by input hash, reusing `cache/target_key.rs`).
Attractive because unchanged inputs produce no new files. Rejected because a flaky test
rerun with identical inputs would overwrite the failing log — precisely the case where
history is wanted.

**Append-only with run separators.** One file, `[farm] === run start ===` between runs.
Trivial to implement, but diffing requires splitting the file apart again and it grows
without bound.

**Extending `farm ctx run` instead of adding `farm log`.** Rejected — see decision 5.

**Write-time carriage-return collapsing.** The PTY tee reassembles lines on `\n` and strips
ANSI plus a *trailing* `\r` ([src/cmd.rs:381-395](../../src/cmd.rs#L381-L395)), so intra-line
`\r` survives. Progress bars (cargo, npm, docker) emit frames separated by `\r` with no
newline until completion, and every frame accumulates into one enormous log line. Collapsing
at write time in `write_line` ([src/build_logger.rs:195-204](../../src/build_logger.rs#L195-L204))
would shrink those logs and make them readable in a pager.

Rejected on asymmetry: read-side collapsing (decision 7) produces identical diff quality, so
write-time collapsing buys the feature nothing, and the two directions are not equally
recoverable — a raw log can always be collapsed when read, a collapsed log can never be
restored. Three further points against it. Bare `\r` as a line terminator still occurs in
some toolchain output, and collapse-to-last-segment would silently delete real content with
no way to notice. It would change log content for every run including CI, which has neither
PTY nor progress bars — out of scope for a retention decision. And it pulls against the
direction of the `handle non utf8` work, which is about capturing odd byte streams
faithfully.

The one surviving argument was file size, and it is currently unmeasured: across the three
existing run logs there are **zero carriage returns** and the longest line is 311 bytes.
That evidence is weak — those logs predate the PTY path, and podman and cargo suppress
progress rendering when not attached to a tty — but weak evidence is not grounds for an
irreversible lossy transform. If PTY logs do prove to be megabytes of progress frames, the
answer is a max-total-bytes retention rule, which is additive and reversible.

**Terminal emulation for log rendering** (replaying the byte stream through `vt100`/`vte` to
render a true final screen). Would correctly handle multi-line rewrites — cursor-up plus
erase, as used by docker compose and cargo's multi-progress — which CR collapsing cannot.
Rejected because a screen model requires scrollback semantics that do not fit long build
logs, where the whole point is that output scrolls past. Recorded here so it is not
rediscovered as an obvious idea.

## Open questions

None outstanding. Resolved during review: command naming (decision 5), diff rendering
(decision 7), and PTY handling (decision 8, plus the write-time collapsing alternative).

## Implementation order

1. Unique local `build_id`; write the manifest stub at run start (with `interactive`) and
   rewrite at completion.
2. Shared run resolver; repoint `farm replay` at it so `farm replay <goal>` keeps working.
3. Pruning (keep-N, max age, keep-newest-failure) at run start.
4. `farm log`, `--list`, and `--diff` with the normalization layer.
5. Identical-log deduplication and a configuration surface, if demand appears.
