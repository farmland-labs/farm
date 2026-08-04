<div align="center">
   <img src="assets/Farmland_Logo_only_parts_plain.svg" alt="farm" width="116" height="116" />
</div>
<div align="center">
   <div><h1>Farm - Build Execution Runner</h1></div>
   <div><h3>with Simple Dependency System and Caching</h3></div>
</div>

`Farm` is an intuitive command-line interface for executing build operations defined in TOML-like `Farmfile`.
A simple dependency system maps the stages into a dependency tree and the execution plan can be visualized as a graph.
A execution context is managed by `farm` and provides a unified logging system.
`Farm`'s effective and simple cache system gives the possibility to skip stages when nothing has changed.
It basically acts as a **streamlined wrapper between the User and the CI.**

## Architecture Overview

```
┌──────────────────────────────────────────────────────────────────────────────┐
│                              farm CLI                                        │
│                                                                              │
│   ┌─────────────────┐    ┌─────────────────┐    ┌─────────────────────────┐  │
│   │   CLI Parser    │    │  Plan Builder   │    │   Execution Engine      │  │
│   │   (clap)        │───▶│  (vendor/parse) │───▶│   (farm/engine.rs)      │  │
│   └─────────────────┘    └─────────────────┘    └───────────┬─────────────┘  │
│                                                             │                │
│   ┌─────────────────┐    ┌─────────────────┐                │                │
│   │   Farmfile      │    │   Dependency    │                ▼                │
│   │   Reader        │───▶│   Graph (DAG)   │    ┌─────────────────────────┐  │
│   └─────────────────┘    └─────────────────┘    │  Work Executor          │  │
│                                                 │   • Spawn processes     │  │
│   ┌─────────────────┐    ┌─────────────────┐    │   • Stream output       │  │
│   │   Variant       │    │   Environment   │    │   • Handle signals      │  │
│   │   Resolver      │───▶│   Manager       │───▶│   • Log results         │  │
│   └─────────────────┘    └─────────────────┘    └─────────────────────────┘  │
│                                                                              │
└──────────────────────────────────────────────────────────────────────────────┘
                                      │
                                      ▼
                          ┌─────────────────────┐
                          │   Build Artifacts   │
                          │   • Logs            │
                          │   • Outputs         │
                          └─────────────────────┘
```

## Data Flow

1. **Parse CLI** - Parse command-line arguments and resolve Farmfile path
2. **Load Farmfile** - Read and parse the Farmfile (TOML-like syntax)
3. **Build Plan** - Construct dependency graph and resolve variants
4. **Execute DAG** - Execute stages in topological order
5. **Stream Output** - Show real-time build output (unless `--silent`)
6. **Report Results** - Display final build status and timing

## Overview

Farm reads a `Farmfile` that describes build operations with their dependencies.
By calling Farm with a goal it executes all dependent operations in the correct order based on the dependency graph.
Simple cache declarations are used to determine if the step needs to be called or can be skipped entirely.
From the principle its like a Makefile with defined goals, which can be called from Farm Buddy in the Farmland Project.

## Installation

```bash
cargo install --path .
```

## Usage

```bash
# Show the parsed default Farmfile (no subcommand defaults to `plan`)
farm

# Run a target (and its dependencies)
farm run build

# Run with a specific variant
farm run build --variant debug

# Use a specific Farmfile
farm run build -f Farmfile.dev

# Run only the target, skipping dependencies
farm run build --only

# Run with verbose (debug) logging
farm run build --verbose

# View the build plan
farm plan

# View plan in DOT format for visualization
farm plan --format dot | dot -Tpng > plan.png
```

## Farmfile Format

A Farmfile starts with `version: 1` and is composed of sections. Operations
(`[operation.<name>]`) declare the work and its dependencies; the other
sections are optional.

```toml
version: 1

# Variants you can build for (selected with --variant).
[variant]
debug
release

# Environment variables injected into EVERY operation's command.
[env]
NODE_ENV=production
RUST_LOG=warn

# User-facing goal names mapped to operation labels (entry points).
[goal]
build: build_app
test: run_tests

[operation.prepare]
work: mkdir -p build

[operation.build_app]
variant: debug
after: prepare
input: src/**/*.rs, Cargo.toml, Cargo.lock
output: target/debug/app
env: RUSTFLAGS              # cache-key env (see Target Cache, below)
work: make build VARIANT=debug

[operation.run_tests]
after: build_app
work: make test
```

Section reference:

| Section | Purpose |
|---------|---------|
| `[variant]` | Build variants selectable via `--variant` |
| `[env]` | Env vars set for all operations (see [`[env]` Section](#env-section)) |
| `[goal]` | Friendly goal names → operation labels |
| `[cache]` | Global cache config (see [`[cache]` Configuration Section](#cache-configuration-section)) |
| `[operation.<name>]` | An operation: `work:`, `after:`, `input:`, `output:`, `env:`, `variant:` |

### <a id="env-section"></a>`[env]` Section

The `[env]` section declares environment variables that are injected into the
command environment of **every** operation. Values are literal — everything
after the first `=`, so they may contain `:`, `=`, spaces, or `#`:

```toml
[env]
NODE_ENV=production
API_URL=https://api.example.com/v1
GREETING=hello world
```

Precedence: `[env]` values **override** the caller's shell environment, but the
reserved `FARM_*` variables farm injects per run (e.g. `FARM_TARGET`,
`FARM_VARIANT`) always win. `[env]` values do **not** affect cache keys — only
env declared via an operation's `env:` line or `[cache] env_key` does.

## Local Development Flow

### Simple Build (Most Common)
```bash
cd myproject
farm run build                    # Execute build, creates logs and manifest
```
No parcels created, no context needed. Fast iteration.

### Build Artifacts Location

Every run writes its own directory under `.farm/run/<build-id>/`, so running a
goal twice no longer destroys the first run's log. The build-id is generated per
run, or set explicitly with `--build-id` / `FARM_BUILD_ID`:

```
.farm/run/<build-id>/
├── log/
│   └── <target>_<variant>.log   # streamed build log (combined, default)
├── manifest.json                # build metadata (git commit, timing, stages)
└── work.json                    # used by `farm replay`
```

Output is **streamed into the log file line-by-line** as it arrives, so a build
in progress can be followed live. The on-disk log is plain text (ANSI colors
stripped); colors still show live on the console.

Run history is bounded automatically: the last 5 runs per goal and variant are
kept, anything older than 7 days is discarded, and the most recent *failure*
always survives — the run you most likely want to read is the one plain
keep-the-last-5 would throw away.

These paths are internal and change between versions. Read them through
`farm log` rather than constructing them.

### Reading Run Logs

```bash
farm log                          # log of the most recent run, whatever the goal
farm log build                    # log of the most recent `build` run
farm log build --list             # recent `build` runs, newest first
farm log build --diff             # compare the last two runs
farm log build --diff --against 3 # compare against 3 runs back
farm log build --follow           # follow a run as it executes
```

Diffs are normalized first: timestamps, durations, run directory names and
progress-bar redraws are stripped, so what remains is what actually changed
between two runs. `--raw` shows the log exactly as stored, and `--only-output`
drops farm's own `[farm]` framing lines.

### Replay a Build

```bash
farm replay <goal>                # Re-run the most recent run of a goal
farm replay <build-id>            # Re-run a specific run
farm replay --dry-run <build-id>  # Show what would run
```

The argument may be a goal name, a build id, or a path to a run directory. A
replay is a new attempt: it gets its own run directory rather than overwriting
the run it replays, with `FARM_REPLAY_OF` recording where it came from.

### Contexts

Contexts isolate build state (e.g. one per git branch) so parallel builds on
the same machine don't collide.

```bash
farm ctx                       # print the current context name (like `git branch`)
farm ctx init                  # set up a context for the current branch (one-shot)
farm ctx branch list           # list branch contexts
farm ctx branch use <name>     # switch the active branch context
farm ctx run list              # list runs in the workspace
eval "$(farm ctx env)"         # export this context's FARM_* vars into your shell
```

`farm ctx env [name]` supports `--format export|json|fish`. Run
`farm ctx --help` (and `farm ctx branch --help`, `farm ctx run --help`) for the
full surface.

## Environment Variables

farm reads these from the environment:

| Variable | Description |
|----------|-------------|
| `FARM_FARMFILE` | Default Farmfile path (overridden by `-f`) |
| `FARM_BUILD_ID` | Build ID for run isolation (default: goal name) |
| `FARM_CTX` | Active context name |
| `FARM_VARIANT` / `FARM_TARGET` | Default variant / target |
| `NO_COLOR` | Disable colored output (e.g. the `farm version` banner) |
| `RUST_LOG` | Logging level (e.g. `farm=debug`, `farm::cache=debug`) |

During a run, farm also **sets** these for each operation's command:
`FARM_TARGET`, `FARM_VARIANT`, `FARM_WORKSPACE`, `FARM_DIR`, `FARM_LOG_DIR`,
`FARM_EVENT` (plus anything from the `[env]` section). Operations run
**non-interactively** — their stdin is `/dev/null`, so a process that prompts
for input gets EOF and fails fast rather than hanging.

## CLI Options

(Global options; also accepted by `farm run`.)

| Option | Description |
|--------|-------------|
| `-f, --farmfile` | Path to Farmfile (default: `Farmfile`) |
| `--variant` | Variant to use for execution (default: `default`) |
| `--target` | Target operation to reach |
| `--farm-dir` | Directory for runtime files (logs, cache) |
| `--build-id` | Build ID for run isolation (default: goal name) |
| `--silent` | Suppress real-time task output |
| `--log-output` | `file` (default), `file-split`, `stdout`, or `none` |
| `--split-streams` | Keep stdout/stderr separate (may reorder output) |
| `-1, --only` | Run only the target, skip dependencies |
| `-n, --no-cache` | Bypass cache, always re-execute |
| `--verbose` | Enable debug-level logging |
| `-V, --version` | Print version (rustc-style) |
| `--only, -1` | Run only the target, skip dependencies |
| `--no-cache, -n` | Bypass cache, always re-execute |

## Subcommands

| Command | Description |
|---------|-------------|
| `farm run <target>` | Run a target and its dependencies |
| `farm plan` | Display the execution plan |
| `farm plan --format dot` | Output plan in DOT graph format |
| `farm ctx` | Manage contexts (build-state isolation) — see [Contexts](#contexts) |
| `farm log [goal]` | Show, list or diff the logs of previous runs |
| `farm replay <goal\|build-id>` | Re-run a build from a preserved run directory |
| `farm cache list` | List cached targets |
| `farm cache get <key>` | Show a cache entry by key prefix |
| `farm cache stats` | Show cache statistics |
| `farm cache clear` | Clear all cache entries |
| `farm cache remove <id>` | Remove entries by operation name or key prefix |
| `farm cache check <ops…>` | Check input/output declarations between dependent ops |
| `farm version` | Show the version banner |

## Target Cache System

Farm includes an cache system that **skips re-execution of stages when inputs haven't changed**. This  speeds up builds by avoiding redundant work.

### Purpose

The cache answers one question: **"Have the inputs to this stage changed since the last successful execution?"**

If not, execution is skipped entirely. The cache is local (per-workspace) and purely based on content hashing—it doesn't track timestamps, only actual file contents.

### Why Content Hashes, Not Timestamps?

Many build tools (like `make`) use file modification timestamps to detect changes. Farm uses **content hashing** instead. Here's why:

| Aspect | Timestamps | Content Hashes |
|--------|-----------|----------------|
| **Accuracy** | Can miss changes (same timestamp, different content) or false positive (touched but unchanged) | Always accurate—only actual content changes trigger rebuilds |
| **Git operations** | `git checkout`, `git rebase` change timestamps without changing content → unnecessary rebuilds | Content unchanged = no rebuild, regardless of timestamp |
| **Clock issues** | Network filesystems, VMs, containers can have skewed clocks | Clock-independent |
| **Build machines** | Different machines have different clocks → inconsistent behavior | Identical content = identical hash, anywhere |
| **Copying files** | Copy preserves content but may change timestamp | Hash stays the same |

**Trade-off:** Hashing requires reading file contents (I/O), while timestamps are just metadata. Farm mitigates this with:
- Parallel file hashing (using rayon for 10+ files)
- Only hashing files that match declared patterns

**Bottom line:** Content hashing is slower but **correct**. Timestamp-based caching is faster but prone to false positives and negatives that waste developer time debugging "why didn't it rebuild?" or "why did it rebuild everything?"

### What Makes a Stage Cacheable

A stage is cacheable **only if it declares both `input:` and `output:`**.
`input:` tells Farm which files affect the result; `output:` is what Farm
re-hashes to confirm a cache hit is still valid.

```toml
[operation.build]
input: src/**/*.rs, Cargo.toml, Cargo.lock
output: target/release/myapp
work: cargo build --release
```

**Without `input:`, the stage is uncacheable and always executes** — Farm can't
know what your command depends on unless you declare it. **Without `output:`,
caching is also disabled**, because Farm has nothing to verify against (it can't
tell whether a previous result is still on disk).

### The `output` Declaration

The `output:` field declares the files a stage produces. It is used to:
- **Gate caching** — a stage is only cacheable when `output:` is present.
- **Validate hits** — on a key match, Farm re-hashes the `output:` files and
  compares them to the hash stored at write time (see [Output Validation](#output-validation)).
- **Document** the data flow between stages.

```toml
[operation.build]
input: src/**/*.rs, Cargo.toml, Cargo.lock
output: target/release/myapp
work: cargo build --release
```

**Note:** `output:` is **not** part of the cache *key* — the key is
`SHA256(input + command + env + variant)`. But the output **hash** is stored
with the entry and checked on every lookup, so a deleted/changed artifact
correctly forces a rebuild.

### ⚠️ WARNING: Incomplete Input Declarations Cause False Cache Hits

**The cache is only as correct as your `input` declaration.** If you fail to declare a file that affects your build output, Farm will not detect changes to that file, leading to **false cache hits** (skipping execution when it should have run).

```toml
# ❌ DANGEROUS - Missing Cargo.lock means dependency updates are ignored!
[operation.build]
input: src/**/*.rs, Cargo.toml
work: cargo build --release

# ✅ CORRECT - All files that affect the build are declared
[operation.build]
input: src/**/*.rs, Cargo.toml, Cargo.lock, build.rs
work: cargo build --release
```

**Common mistakes that cause false cache hits:**
- Missing lock files (`Cargo.lock`, `package-lock.json`, `yarn.lock`)
- Missing build scripts (`build.rs`, `Makefile`, `webpack.config.js`)
- Missing generated files from previous stages
- Missing header files for C/C++ builds
- Environment variables that affect output but aren't declared with `env:`

**If builds behave incorrectly after cache hits, your `input` declaration is likely incomplete.** Use `farm run --no-cache` to verify the build works without caching, then fix your `input` patterns.

### Comparison to Other Build Systems

| Build System | Change Detection | Strengths | Weaknesses |
|--------------|------------------|-----------|------------|
| **make** | Timestamps | Fast, widely understood | False rebuilds on `touch`, `git checkout`; misses changes with same timestamp |
| **ninja** | Timestamps | Extremely fast, designed for generated build files | Same timestamp issues as `make` |
| **cmake** | Timestamps (via ninja/make) | High-level, cross-platform | Inherits timestamp issues from backend |
| **Bazel** | Content hashes | Correct, hermetic, remote caching | Complex, steep learning curve |
| **Buck2** | Content hashes | Fast, correct, distributed | Meta-specific, complex setup |
| **Farm** | Content hashes | Correct, simple Farmfile syntax, explicit inputs | Requires manual input declaration |

#### The "touch" Problem (Ninja, Make)

```bash
# Ninja/Make: Touch triggers rebuild even though content unchanged
touch src/main.rs
ninja build        # Rebuilds everything that depends on main.rs (unnecessary!)

# Farm: Touch does NOT trigger rebuild
touch src/main.rs
farm run build     # Cache hit! Content hash unchanged, no rebuild.
```

**This is a feature, not a bug.** Farm only rebuilds when actual content changes, saving time on:
- `git checkout` / `git rebase` (which update timestamps)
- CI systems that clone fresh repos
- Developers who `touch` files accidentally

#### Ninja/Make as Downstream Build Tools

Farm and ninja/make serve different purposes and work well **together**:

```toml
[operation.build]
input: src/**/*.rs, Cargo.toml, Cargo.lock
work: cargo build --release   # Cargo internally uses incremental compilation
```

Here, Farm determines "should we run `cargo build` at all?" based on content hashes. If the answer is yes, Cargo/rustc then uses its own incremental compilation (timestamp-based or hash-based) for fine-grained artifact reuse.

**Farm's job:** Skip entire stages when nothing changed.  
**Ninja/Make's job:** Within a stage, do minimal incremental work.

### Target Key Computation

A **target key** uniquely identifies a build configuration. It is computed as:

```
target_key = SHA256(inputs_hash + command_hash + env_hash + variant_hash)
```

The `variant_hash` ensures different `--variant` builds never share a key.

#### 1. `inputs_hash` — Hash of Input File Contents

Farm expands all glob patterns and hashes each matching **file** (not directories):

1. Expand each glob pattern relative to the workspace root
2. Collect all matching **files** (directories are ignored)
3. Sort files alphabetically by path (for determinism)
4. For each file: hash `relative_path + \0 + file_content + \0`
5. Combine all individual hashes into final `inputs_hash`

**Critical:** Both the file path AND content contribute to the hash. Renaming a file changes the hash even if content is identical.

#### 2. `command_hash` — Hash of the Command String

```
command_hash = SHA256(work_command)
```

Changing the command (even whitespace) invalidates the cache.

#### 3. `env_hash` — Hash of Declared Environment Variables

Only environment variables **declared in the Farmfile** are included:

```toml
[operation.build]
env: RUSTFLAGS="-C target-cpu=native", CC
input: src/**/*.rs
work: cargo build
```

- `RUSTFLAGS="-C target-cpu=native"` — literal value is hashed
- `CC` — **current value** of `$CC` at execution time is hashed (bare key = capture)

Environment variables are sorted by name before hashing for determinism.

### Cache Lookup Flow

```
┌──────────────────────────────────────────────────────────────────────────┐
│  Stage has `input` declared?                                             │
│    NO  ──────────────────────────────────────────► EXECUTE (uncacheable) │
│    YES                                                                   │
│     │                                                                    │
│     ▼                                                                    │
│  Expand glob patterns, find matching FILES                               │
│     │                                                                    │
│     ▼                                                                    │
│  Any files matched?                                                      │
│    NO  ───────────────────────────────────────────► EXECUTE (uncacheable)│
│                                                    ⚠️  Warning logged    │
│    YES                                                                   │
│     │                                                                    │
│     ▼                                                                    │
│  Compute target_key = SHA256(inputs + command + env)                     │
│     │                                                                    │
│     ▼                                                                    │
│  Lookup key in .farm/cache/goal-cache.json                               │
│     │                                                                    │
│     ├── HIT  ────────────────────────────────────► SKIP (⚡ cached)       │
│     │                                                                    │
│     └── MISS ────────────────────────────────────► EXECUTE               │
│                                                         │                │
│                                     On success: store cache entry        │
└──────────────────────────────────────────────────────────────────────────┘
```

### Glob Pattern Syntax (Important!)

Farm uses standard glob patterns. **Common mistake:** using `**` alone.

| Pattern | Matches |
|---------|---------|
| `src/**/*.rs` | All `.rs` files recursively under `src/` |
| `src/**/*` | All files recursively under `src/` |
| `src/**` | ⚠️ **Directories only!** Not files. |
| `*.rs` | `.rs` files in workspace root only |
| `Cargo.toml` | Single file |

**The `**` pattern matches directories, not files.** To match all files recursively, use `**/*`.

```toml
# ❌ WRONG - matches 0 files (only directories)
input: ./scripts/**

# ✅ CORRECT - matches all files in scripts/
input: ./scripts/**/*

# ✅ CORRECT - matches only .sh files
input: ./scripts/**/*.sh
```

If your input patterns match 0 files, Farm logs a warning and **disables caching for that stage** (it will always execute).

### Cache Invalidation

The cache is invalidated (stage re-executes) when **any** of these change:

1. **File content** — Any input file is modified
2. **File added/removed** — New file matches pattern, or existing file deleted
3. **File renamed** — Path is part of the hash
4. **Command changed** — Any change to the work command
5. **Environment changed** — Declared env vars have different values
6. **Input patterns changed** — You changed the `input` field itself

**The cache is NOT invalidated by:**

- `touch file` — Timestamp changes don't matter, only content
- `git checkout` / `git rebase` — If content is the same, hash is the same
- File permission changes — Only content is hashed
- File copied from another location — Same content = same hash
- Different machine / CI runner — Content hashes are location-independent

### Dependencies (`after:`) and Cache

**Important:** The `after:` keyword controls **execution order only**, not cache invalidation.

```toml
[operation.config]
input: myfile.txt
output: setup.in
work: cat myfile.txt > setup.in

[operation.build]
after: config
input: my_other_file.txt    # ❌ WRONG - missing setup.in!
output: out.bin
work: process setup.in my_other_file.txt > out.bin
```

**Problem:** If `myfile.txt` changes:
1. `config` re-runs → produces new `setup.in` ✓
2. `build` checks its inputs (`my_other_file.txt`) — unchanged
3. `build` gets **false cache hit** — uses stale `setup.in` ❌

**Solution:** Include upstream outputs that you read in your `input` declaration:

```toml
[operation.build]
after: config
input: my_other_file.txt, setup.in   # ✅ Include the file you actually read
output: out.bin
work: process setup.in my_other_file.txt > out.bin
```

Now changes propagate correctly:
```
myfile.txt → [config] → setup.in → [build] → out.bin
     ↑                       ↑
 config input           build input (includes intermediate)
```

#### The Chain Rule for Multi-Stage Pipelines

**Each stage declares files it DIRECTLY reads, not what upstream stages read.**

```toml
[operation.fetch]
input: urls.txt
output: data/raw/**/*
work: ./fetch_data.sh

[operation.transform]
after: fetch
input: data/raw/**/*           # ✓ What this stage reads
output: data/processed/**/*
work: ./transform.sh

[operation.analyze]
after: transform
input: data/processed/**/*     # ✓ What this stage reads (NOT data/raw/**)
output: reports/**/*
work: ./analyze.sh
```

**You don't need to declare `urls.txt` in `transform`** because:
- `transform` doesn't read `urls.txt` directly
- Changes to `urls.txt` propagate through the artifact chain:
  `urls.txt` changes → `fetch` reruns → `data/raw` changes → `transform` reruns

#### Why Not Auto-Include Upstream Outputs?

Farm could theoretically auto-include upstream `output:` declarations in downstream cache keys. We chose explicit declaration because:

| Aspect | Explicit (Farm's Design) | Implicit/Automatic |
|--------|--------------------------|-------------------|
| **Predictability** | You know exactly what's hashed | Hidden behavior, harder to debug |
| **Control** | Skip outputs you don't use | Forced to include everything |
| **`after:` semantics** | Clear: ordering only | Ambiguous: ordering + data flow? |
| **Debugging** | `RUST_LOG=debug` shows exact files | Magic inclusions confuse users |

The `after:` relationship is ambiguous by design—it might mean:
- "Build reads config's output" (data dependency)
- "Don't run simultaneously" (resource contention)
- "Order for side effects" (deploy after build, but deploy doesn't read build output)

Making `after:` automatically imply "include all outputs" would be incorrect in some cases.

### Cache Storage

| Item | Details |
|------|---------|
| Location | `.farm/cache/goal-cache.json` |
| Format | JSON map of `target_key → CacheEntry` |
| CacheEntry | `{ target_key, content_hash, stage_name, stage_variant, parcel_ref, vcs, created_at }` |
| Scope | Per-workspace (not shared between machines) |

### Cache Commands

```bash
farm cache list              # List all cached targets
farm cache stats             # Show cache statistics  
farm cache clear             # Clear all entries
farm cache clear -y          # Clear without confirmation
farm cache remove <id>       # Remove by stage name or key prefix
farm cache remove build      # Remove cache entry for 'build' stage
farm cache remove 34f8       # Remove by partial target key match
```

### Debugging Cache Issues

Enable debug logging to see cache computations:

```bash
RUST_LOG=farm::cache=debug farm run build
```

Example output:
```
[build] Computing target key for 2 input patterns
[build] Hashed 19 files, 33549 total bytes in 2.2ms (parallel)
[build] Computed target key: 34f874672a303c72
⚡ Cache hit for stage: build (key: 34f874672a30)
```

**If you see "0 files matched"**, your glob patterns are wrong (see [Glob Pattern Syntax](#glob-pattern-syntax-important)).

### Bypassing Cache

```bash
farm run build --no-cache    # Force re-execution, ignore cache
farm run build -n            # Short form of --no-cache
```

### `[cache]` Configuration Section

Configure cache behavior at the Farmfile level with a `[cache]` section:

```toml
[cache]
# Maximum local cache size (triggers LRU eviction when exceeded)
max_size: 10GB

# Environment variables that always influence cache key
# (cross-platform builds need different cache entries)
env_key:
    FARM_PLATFORM      # e.g., "linux-x86_64", "darwin-arm64"
    RUST_TARGET        # e.g., "x86_64-unknown-linux-musl"
    CC                 # Compiler matters for native builds

# Command outputs that influence cache key
# (for detecting toolchain versions dynamically)
env_command:
    RUSTC_VERSION: rustc --version
    NODE_VERSION: node --version
    GCC_VERSION: gcc --version 2>&1 | head -1

# TTL for local cache entries (optional, default: no expiry)
ttl: 30d
```

#### Platform-Specific Cache Keys

When building for multiple platforms, cache keys must differ to avoid serving the wrong artifacts:

- **`env_key`**: Named env vars that must match for cache hit
- **`env_command`**: Commands whose output is hashed into cache key
  - Enables automatic platform detection: `uname -m`, `rustc --version`
  - Output is trimmed and hashed, not stored verbatim

**Example cross-platform scenario:**
```
# Machine A (x86_64):
$ RUSTC_VERSION=$(rustc --version) → "rustc 1.92.0 (ded5c06cf 2025-12-08)"
$ cache_key = hash(inputs + command + env + "rustc 1.92.0...")

# Machine B (arm64):  
$ FARM_PLATFORM=darwin-arm64
$ cache_key = hash(inputs + command + env + "rustc 1.92.0..." + "darwin-arm64")
# → Different key, won't use x86_64 parcel on arm64
```

#### Output Validation

**Problem:** the cache key matches, but the outputs were deleted or modified
since (e.g. `cargo clean`, a stray edit).

**Solution:** a key match alone is not a hit. On a key match, farm **re-hashes
the declared `output:` files** and compares them to the content hash recorded
when the entry was written:

```
On cache lookup (only when both `input:` and `output:` are declared):
1. Compute target_key = SHA256(inputs + command + env + variant)
2. Find the cache entry for that key
3. Re-hash the current `output:` files:
     • hash == stored content_hash       → cache HIT  (skip execution)
     • outputs changed / missing / errored → cache MISS
4. On a MISS the operation re-executes; on success the entry is re-stored
```

So a deleted or altered output simply turns into a rebuild — there is no
"stuck" state where the cache claims a hit but the artifact is gone.

#### Hash Collision Safety

**Question:** Can hash collisions cause wrong cached outputs to be served?

**Answer:** Astronomically unlikely. Farm uses SHA-256 for target key computation.

**Target key composition:**
```
target_key = SHA256(inputs_hash || command_hash || env_hash || variant_hash)
```

**Collision probability:**

| Metric | Value |
|--------|-------|
| SHA-256 output space | 2^256 possible keys |
| Birthday attack threshold | ~2^128 operations for 50% collision chance |
| Time at 1 trillion hashes/sec | ~10^22 years |
| Universe age | ~10^10 years |

For a collision to cause cache poisoning, **all of these** must collide simultaneously:
- Different `inputs` content → same hash
- Different `command` → same hash  
- Different `env` → same hash
- Different `variant` → same hash
- Combined hash also collides

**Real risks to focus on instead:**

| Risk | Probability | Mitigation |
|------|------------|------------|
| Input pattern too broad | High | Be specific with `inputs:` globs |
| Missing env var | Medium | Use `env_key:` in `[cache]` for platform-dependent builds |
| Forgot dependency file | Common | Include `Cargo.toml`, `package.json` in inputs |

**Bottom line:** Don't worry about SHA-256 collisions. Focus on complete `inputs:` and `env:` declarations.

### Guide: Finding the Right Input Declaration

Properly declaring inputs is **critical** for correct caching. Here's how to determine what files affect your build.

#### Step 1: Start with the Obvious Source Files

```toml
# For a Rust project
input: src/**/*.rs

# For a TypeScript project  
input: src/**/*.ts, src/**/*.tsx

# For a C/C++ project
input: src/**/*.c, src/**/*.cpp, src/**/*.h, include/**/*.h
```

#### Step 2: Add Configuration and Build Files

Ask: **"What files would require a rebuild if changed?"**

| Language/Tool | Files to Include |
|---------------|------------------|
| **Rust/Cargo** | `Cargo.toml`, `Cargo.lock`, `build.rs`, `.cargo/config.toml` |
| **Node/npm** | `package.json`, `package-lock.json`, `tsconfig.json`, `webpack.config.js`, `.babelrc` |
| **Python** | `requirements.txt`, `pyproject.toml`, `setup.py`, `setup.cfg` |
| **Go** | `go.mod`, `go.sum` |
| **C/C++ (Make)** | `Makefile`, `CMakeLists.txt`, `*.cmake` |
| **Docker** | `Dockerfile`, `.dockerignore` |
| **General** | `.env`, `config/**/*` |

#### Step 3: Include Lock Files (Critical!)

Lock files pin exact dependency versions. **Missing lock files = silent dependency upgrades = false cache hits.**

```toml
# ❌ WRONG - Dependency updates won't trigger rebuild!
input: src/**/*.rs, Cargo.toml

# ✅ CORRECT - Lock file included
input: src/**/*.rs, Cargo.toml, Cargo.lock
```

#### Step 4: Check for Generated/Output Dependencies

**Critical:** The `after:` keyword controls execution order, NOT cache invalidation. If Stage B depends on Stage A's output, Stage B **must explicitly include those files** in its `input`:

```toml
[operation.codegen]
input: schema/**/*.graphql
output: generated/**/*.rs
work: ./generate-code.sh

[operation.build]
after: codegen
input: src/**/*.rs, generated/**/*.rs, Cargo.toml, Cargo.lock
#                   ^^^^^^^^^^^^^^^^^ Must include! after: doesn't auto-include.
work: cargo build
```

Without `generated/**/*.rs` in build's input, changes to GraphQL schema would:
1. Trigger `codegen` to regenerate files ✓
2. But `build` would get a **false cache hit** ❌ (it doesn't know `generated/*` changed)

See [Dependencies (`after:`) and Cache](#dependencies-after-and-cache) for detailed explanation.

#### Step 5: Identify Environment Variables

Some builds are affected by environment variables:

```toml
[operation.build]
input: src/**/*.c, Makefile
env: CC, CFLAGS, TARGET_ARCH
work: make build
```

**Common env vars that affect builds:**
- `CC`, `CXX`, `CFLAGS`, `CXXFLAGS`, `LDFLAGS` — C/C++ compiler settings
- `RUSTFLAGS`, `CARGO_TARGET_DIR` — Rust settings
- `NODE_ENV`, `NODE_OPTIONS` — Node.js settings
- `GOOS`, `GOARCH` — Go cross-compilation
- `PATH` — If you use tools from specific locations

#### Step 6: Test Your Declaration

1. **Run with caching disabled to establish baseline:**
   ```bash
   farm run build --no-cache   # Should succeed
   ```

2. **Run again to verify cache hit:**
   ```bash
   farm run build              # Should say "Cache hit"
   ```

3. **Modify a file you declared and verify cache miss:**
   ```bash
   echo "// comment" >> src/main.rs
   farm run build              # Should rebuild (cache miss)
   ```

4. **Modify a file you DIDN'T declare and check:**
   ```bash
   echo "# comment" >> some_config.txt
   farm run build              # Cache hit? If build should have changed, you're missing an input!
   ```

#### Step 7: Use Debug Logging to Verify

```bash
RUST_LOG=farm::cache=debug farm run build
```

Check the output:
- **"Hashing N input files"** — Are all expected files included?
- **"Computed target key: XXX"** — Does it change when you expect it to?

#### Quick Reference: Common Project Types

**Rust Project:**
```toml
[operation.build]
input: src/**/*.rs, Cargo.toml, Cargo.lock, build.rs, .cargo/**/*
env: RUSTFLAGS
work: cargo build --release
```

**Node.js/TypeScript Project:**
```toml
[operation.build]
input: src/**/*.ts, src/**/*.tsx, package.json, package-lock.json, tsconfig.json, webpack.config.js
env: NODE_ENV
work: npm run build
```

**Python Project:**
```toml
[operation.build]
input: src/**/*.py, requirements.txt, pyproject.toml, setup.py
work: pip install -e . && python -m build
```

**C/C++ with Make:**
```toml
[operation.build]
input: src/**/*.c, src/**/*.cpp, src/**/*.h, include/**/*.h, Makefile
env: CC, CFLAGS
work: make build
```

**Docker Build:**
```toml
[operation.docker]
input: Dockerfile, src/**/*.*, package.json, package-lock.json
work: docker build -t myapp .
```

#### Red Flags: Signs Your Input Declaration is Incomplete

| Symptom | Likely Cause |
|---------|--------------|
| Build succeeds after cache hit but output is wrong | Missing input file |
| Upstream stage changed but downstream didn't rebuild | Missing upstream `output` in downstream `input` |
| `cargo update` doesn't trigger rebuild | Missing `Cargo.lock` |
| New dependency doesn't trigger rebuild | Missing lock file |
| Environment change doesn't trigger rebuild | Missing `env:` declaration |
| Header file change doesn't trigger rebuild | Missing `**/*.h` pattern |
| Config change doesn't trigger rebuild | Missing config file in input |
| Build works locally but fails in CI after cache hit | Env var differs, not captured |

### Best Practices

1. **Declare all inputs** — Include every file that affects the output
   ```toml
   input: src/**/*.rs, Cargo.toml, Cargo.lock, build.rs
   ```

2. **Be specific** — Over-broad patterns slow down hashing and may cause false cache misses
   ```toml
   # ❌ Too broad - includes test files, docs, etc.
   input: ./**/*
   
   # ✅ Specific - only source files
   input: src/**/*.rs, Cargo.toml
   ```

3. **Include lock files** — Dependency changes should invalidate cache
   ```toml
   input: package.json, package-lock.json, src/**/*
   ```

4. **Capture env vars that matter** — If `$CC` affects your build, declare it
   ```toml
   env: CC, CFLAGS
   ```

5. **Don't forget generated files** — If a stage depends on output from another stage, include it
   ```toml
   [operation.codegen]
   input: schema/**/*.graphql
   work: ./codegen.sh
   
   [operation.build]
   after: codegen
   input: src/**/*.rs, generated/**/*.rs
   work: cargo build
   ```

## API / Hosted Service Disclaimer

This repository contains the **runner** only. The runner is open-source under **MIT OR Apache-2.0**.  

**Important:**  

- The runner executes commands locally and does **not include any backend services**.
- You may **not** use the runner to bypass the intended workflow of the client or any proprietary services.
- You may **not** fork the runner to provide a competing SaaS using our backend.
- Contributions to this repository are subject to the same MIT/Apache license.

## License

This project is open-source and dual-licensed under **MIT OR Apache-2.0**.  

- See [`LICENSE-MIT`](LICENSE-MIT) for the MIT License text.  
- See [`LICENSE-APACHE`](LICENSE-APACHE) for the Apache 2.0 License text.  

Copyright (c) 2026 Christian Staffa

SPDX Identifier for source files: `MIT OR Apache-2.0`

## Contributing

By submitting a pull request or contribution, you agree that your work will be licensed under **MIT OR Apache-2.0**.  

## Trademark Notice

Farmland and the Farmland logo are registered trademarks of Christian Staffa.

The license does **not** grant any rights to use the trademarks,
service marks, or logos.

If you fork this project, you must remove all references to our brand.
