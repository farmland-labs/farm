//! SPDX-License-Identifier: MIT OR Apache-2.0

//! Run directory discovery.
//!
//! Since ADR 0001 every invocation gets its own `.farm/run/{build_id}/`, so
//! "the log for goal X" is no longer a fixed path — it has to be resolved.
//! This module is the single place that does that resolving, shared by
//! retention, `farm log`, and `farm replay`.
//!
//! **Ordering and identity come from `manifest.json`, never from the directory
//! name.** The name carries a goal and a timestamp purely so the directory is
//! recognisable when poking around by hand; parsing it would freeze a format the
//! ADR deliberately left free to change. Directories without a readable manifest
//! (killed before the stub landed, or corrupted) still surface — they just fall
//! back to mtime for ordering and carry no goal or variant, so callers filtering
//! by goal will not match them.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::manifest::{ReplayManifest, RunStatus};

/// One discovered run directory.
#[derive(Debug, Clone)]
pub struct RunEntry {
    /// Build ID — the run directory's name.
    pub build_id: String,
    /// Absolute path to `.farm/run/{build_id}/`.
    pub dir: PathBuf,
    /// Parsed `manifest.json`. `None` when the run died before its stub was
    /// written, or the file is unreadable/corrupt.
    pub manifest: Option<ReplayManifest>,
    /// Directory mtime, used to order manifest-less runs.
    pub mtime: SystemTime,
}

impl RunEntry {
    /// Goal this run executed, when known.
    pub fn goal(&self) -> Option<&str> {
        self.manifest.as_ref().map(|m| m.goal.as_str())
    }

    /// Variant this run executed, when known.
    pub fn variant(&self) -> Option<&str> {
        self.manifest.as_ref().map(|m| m.variant.as_str())
    }

    /// Lifecycle state. A run with no manifest at all crashed before it could
    /// record anything, which is reported as [`RunStatus::Running`] — the state
    /// it was in when it died. Callers that must not disturb a live run treat
    /// both the same way, and deliberately so.
    pub fn status(&self) -> RunStatus {
        self.manifest
            .as_ref()
            .map(|m| m.run_status())
            .unwrap_or(RunStatus::Running)
    }

    /// Positive evidence, from a manifest, that this run is executing.
    ///
    /// Deliberately narrower than [`RunEntry::status`], which reports
    /// [`RunStatus::Running`] for a directory with no manifest at all. Both
    /// answers are useful and neither is wrong: automatic pruning wants the
    /// conservative reading (unknown might be live, leave it alone), while an
    /// explicitly requested cleanup has to be able to remove directories that
    /// never carried a manifest — `farm ctx run init` creates exactly those.
    pub fn is_running(&self) -> bool {
        self.manifest
            .as_ref()
            .is_some_and(|m| m.run_status() == RunStatus::Running)
    }

    /// True when this run executed through a PTY.
    pub fn interactive(&self) -> bool {
        self.manifest.as_ref().is_some_and(|m| m.interactive)
    }

    /// Run start from the manifest, falling back to directory mtime. Used both
    /// for ordering and as the age input for retention.
    pub fn started_at(&self) -> SystemTime {
        self.manifest
            .as_ref()
            .and_then(|m| chrono::DateTime::parse_from_rfc3339(&m.created_at).ok())
            .map(SystemTime::from)
            .unwrap_or(self.mtime)
    }

    /// Path to the combined log file for this run, if it exists.
    ///
    /// Split-stream runs have no combined log; those callers want
    /// [`RunEntry::log_dir`] and the `_stdout` / `_stderr` pair instead.
    pub fn combined_log(&self) -> Option<PathBuf> {
        let manifest = self.manifest.as_ref()?;
        let path = self
            .log_dir()
            .join(format!("{}_{}.log", manifest.goal, manifest.variant));
        path.exists().then_some(path)
    }

    /// Path to this run's `log/` directory.
    pub fn log_dir(&self) -> PathBuf {
        self.dir.join("log")
    }

    /// The last [`SHORT_ID_LEN`] characters of the build ID: what `farm log
    /// --list` highlights and completion offers. It resolves through the
    /// unique-suffix rule of [`select`], so it needs no knowledge of the
    /// directory name format (generated IDs end in random hex, which makes the
    /// tail the distinctive part). IDs no longer than that are returned whole.
    pub fn short_id(&self) -> &str {
        let id = self.build_id.as_str();
        match id.char_indices().rev().nth(SHORT_ID_LEN - 1) {
            Some((start, _)) => &id[start..],
            None => id,
        }
    }
}

/// Length of [`RunEntry::short_id`].
pub const SHORT_ID_LEN: usize = 6;

/// Shortest partial build ID [`select`] matches, so a stray short word is not
/// silently taken for an ID.
pub const MIN_PARTIAL_ID: usize = 4;

/// List every run under `{farm_dir}/run/`, newest first.
///
/// Unreadable entries are skipped rather than erroring: a half-written run
/// directory should not stop the caller from finding the others.
pub fn list_runs(farm_dir: &Path) -> Vec<RunEntry> {
    let run_root = farm_dir.join("run");
    let Ok(entries) = std::fs::read_dir(&run_root) else {
        return Vec::new();
    };

    let mut runs: Vec<RunEntry> = entries
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let dir = entry.path();
            if !dir.is_dir() {
                return None;
            }
            let mtime = entry
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);

            Some(RunEntry {
                build_id: entry.file_name().to_string_lossy().into_owned(),
                manifest: ReplayManifest::read_from(&dir).ok(),
                dir,
                mtime,
            })
        })
        .collect();

    runs.sort_by_key(|run| std::cmp::Reverse(run.started_at()));
    runs
}

/// List runs for a goal (and optionally a variant), newest first.
///
/// Runs with no readable manifest carry no goal, so they never match here.
pub fn find_runs(farm_dir: &Path, goal: &str, variant: Option<&str>) -> Vec<RunEntry> {
    list_runs(farm_dir)
        .into_iter()
        .filter(|run| run.goal() == Some(goal))
        .filter(|run| variant.is_none_or(|v| run.variant() == Some(v)))
        .collect()
}

/// A run picked by a user-supplied specifier, together with the list it was
/// picked from: `@N` counts within [`Selection::scope`], and "the previous run"
/// is `scope[index + 1]`.
#[derive(Debug, Clone)]
pub struct Selection {
    /// Runs the selection is relative to, newest first: the selected run's goal
    /// (or every run, for a bare `@N` or no specifier).
    pub scope: Vec<RunEntry>,
    /// Position of the selected run in `scope`.
    pub index: usize,
}

impl Selection {
    /// The selected run.
    pub fn run(&self) -> &RunEntry {
        &self.scope[self.index]
    }
}

/// Resolve a user-supplied run specifier, tried in this order:
///
/// 1. an exact build ID;
/// 2. `goal@N` / `@N`: N runs back within that goal / within all runs
///    (`@0` is the newest), as numbered by `farm log --list`;
/// 3. a goal name: its newest run;
/// 4. a unique prefix or suffix of a build ID, at least [`MIN_PARTIAL_ID`]
///    characters (the [`RunEntry::short_id`] shown by `--list`).
///
/// No specifier selects the newest run of any goal. `variant` restricts every
/// form. Errors are ready-to-print and name what *is* available.
pub fn select(farm_dir: &Path, spec: Option<&str>, variant: Option<&str>) -> Result<Selection, String> {
    let all: Vec<RunEntry> = list_runs(farm_dir)
        .into_iter()
        .filter(|run| variant.is_none_or(|v| run.variant() == Some(v)))
        .collect();

    if all.is_empty() {
        return Err(match variant {
            Some(v) if !list_runs(farm_dir).is_empty() => format!("No runs recorded for variant '{v}'."),
            _ => "No runs recorded yet. Run a goal first.".to_string(),
        });
    }

    let Some(spec) = spec else {
        return Ok(Selection { scope: all, index: 0 });
    };

    // 1. Exact build ID. Checked first so an ID can never be shadowed — CI
    //    build IDs may well coincide with a goal name.
    if let Some(pos) = all.iter().position(|run| run.build_id == spec) {
        return Ok(scoped_to_goal(all, pos));
    }

    // 2. `goal@N` / `@N`.
    if let Some((goal, n)) = spec.rsplit_once('@') {
        let n: usize = n
            .parse()
            .map_err(|_| format!("Invalid run index '{spec}': expected goal@N or @N, e.g. @0 for the newest run"))?;
        let scope = if goal.is_empty() {
            all
        } else {
            let runs = goal_runs(&all, goal);
            if runs.is_empty() {
                return Err(no_match(&all, goal));
            }
            runs
        };
        if n >= scope.len() {
            return Err(format!(
                "'{spec}' is out of range: {} run(s) recorded{} (@0 to @{})",
                scope.len(),
                if goal.is_empty() { String::new() } else { format!(" for '{goal}'") },
                scope.len() - 1
            ));
        }
        return Ok(Selection { scope, index: n });
    }

    // 3. Goal name.
    let scope = goal_runs(&all, spec);
    if !scope.is_empty() {
        return Ok(Selection { scope, index: 0 });
    }

    // 4. Unique part of a build ID.
    if spec.chars().count() >= MIN_PARTIAL_ID {
        let hits: Vec<usize> = all
            .iter()
            .enumerate()
            .filter(|(_, run)| run.build_id.starts_with(spec) || run.build_id.ends_with(spec))
            .map(|(i, _)| i)
            .collect();
        match hits.as_slice() {
            [] => {}
            [pos] => return Ok(scoped_to_goal(all, *pos)),
            many => {
                let ids: Vec<&str> = many.iter().take(5).map(|&i| all[i].build_id.as_str()).collect();
                return Err(format!(
                    "'{spec}' matches {} runs: {}{}. Use more characters.",
                    many.len(),
                    ids.join(", "),
                    if many.len() > ids.len() { ", …" } else { "" }
                ));
            }
        }
    }

    Err(no_match(&all, spec))
}

/// Resolve a user-supplied run specifier to a single run; see [`select`].
///
/// This is what keeps `farm replay test` working now that the local build ID
/// is no longer the bare goal name — before ADR 0001 the two were the same
/// string, and users typed the goal.
pub fn resolve(farm_dir: &Path, spec: &str, variant: Option<&str>) -> Option<RunEntry> {
    select(farm_dir, Some(spec), variant).ok().map(|sel| sel.run().clone())
}

/// Runs of `goal`, newest first (`all` is already in that order).
fn goal_runs(all: &[RunEntry], goal: &str) -> Vec<RunEntry> {
    all.iter().filter(|run| run.goal() == Some(goal)).cloned().collect()
}

/// Select `all[pos]`, scoped to the runs of its goal so `@N` and "previous run"
/// stay within that goal. A run with no manifest has no goal: it is its own scope.
fn scoped_to_goal(all: Vec<RunEntry>, pos: usize) -> Selection {
    let picked = &all[pos];
    let Some(goal) = picked.goal() else {
        return Selection { scope: vec![picked.clone()], index: 0 };
    };
    let id = picked.build_id.clone();
    let scope = goal_runs(&all, goal);
    let index = scope.iter().position(|run| run.build_id == id).unwrap_or(0);
    Selection { scope, index }
}

fn no_match(all: &[RunEntry], spec: &str) -> String {
    let mut goals: Vec<&str> = all.iter().filter_map(|r| r.goal()).collect();
    goals.sort_unstable();
    goals.dedup();
    format!(
        "No run matches '{spec}' (not a goal, run index or build ID). Recorded goals: {}",
        goals.join(", ")
    )
}

/// The most recent run of any goal, newest first. Backs `farm log` with no
/// arguments.
pub fn latest(farm_dir: &Path) -> Option<RunEntry> {
    list_runs(farm_dir).into_iter().next()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Write a run directory with a manifest, as the engine would.
    fn write_run(farm_dir: &Path, build_id: &str, goal: &str, variant: &str, created_at: &str) {
        let dir = farm_dir.join("run").join(build_id);
        std::fs::create_dir_all(dir.join("log")).unwrap();

        let mut manifest =
            ReplayManifest::running(build_id, goal, variant, false, SystemTime::UNIX_EPOCH);
        manifest.created_at = created_at.to_string();
        manifest.write_to(&dir).unwrap();

        std::fs::write(
            dir.join("log").join(format!("{}_{}.log", goal, variant)),
            "output\n",
        )
        .unwrap();
    }

    fn temp_farm_dir() -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "farm-runs-test-{}",
            uuid::Uuid::now_v7().simple()
        ));
        std::fs::create_dir_all(&base).unwrap();
        base
    }

    #[test]
    fn lists_runs_newest_first() {
        let farm_dir = temp_farm_dir();
        write_run(&farm_dir, "b-old", "test", "default", "2026-01-01T10:00:00+00:00");
        write_run(&farm_dir, "b-new", "test", "default", "2026-01-01T12:00:00+00:00");
        write_run(&farm_dir, "b-mid", "test", "default", "2026-01-01T11:00:00+00:00");

        let ids: Vec<_> = list_runs(&farm_dir)
            .into_iter()
            .map(|r| r.build_id)
            .collect();
        assert_eq!(ids, vec!["b-new", "b-mid", "b-old"]);
    }

    /// Ordering must come from the manifest, not the directory name — the name
    /// format is explicitly not load-bearing.
    #[test]
    fn ordering_ignores_directory_names() {
        let farm_dir = temp_farm_dir();
        write_run(&farm_dir, "zzz", "test", "default", "2026-01-01T12:00:00+00:00");
        write_run(&farm_dir, "aaa", "test", "default", "2026-01-01T10:00:00+00:00");

        let ids: Vec<_> = list_runs(&farm_dir)
            .into_iter()
            .map(|r| r.build_id)
            .collect();
        assert_eq!(ids, vec!["zzz", "aaa"]);
    }

    #[test]
    fn filters_by_goal_and_variant() {
        let farm_dir = temp_farm_dir();
        write_run(&farm_dir, "b-1", "build", "default", "2026-01-01T10:00:00+00:00");
        write_run(&farm_dir, "b-2", "test", "default", "2026-01-01T11:00:00+00:00");
        write_run(&farm_dir, "b-3", "test", "release", "2026-01-01T12:00:00+00:00");

        let all_test: Vec<_> = find_runs(&farm_dir, "test", None)
            .into_iter()
            .map(|r| r.build_id)
            .collect();
        assert_eq!(all_test, vec!["b-3", "b-2"]);

        let release: Vec<_> = find_runs(&farm_dir, "test", Some("release"))
            .into_iter()
            .map(|r| r.build_id)
            .collect();
        assert_eq!(release, vec!["b-3"]);
    }

    #[test]
    fn resolves_exact_build_id_before_goal_name() {
        let farm_dir = temp_farm_dir();
        // A build ID that collides with another run's goal name. The exact
        // match must win, otherwise CI build IDs become unaddressable.
        write_run(&farm_dir, "test", "build", "default", "2026-01-01T10:00:00+00:00");
        write_run(&farm_dir, "b-2", "test", "default", "2026-01-01T11:00:00+00:00");

        let resolved = resolve(&farm_dir, "test", None).unwrap();
        assert_eq!(resolved.build_id, "test");
        assert_eq!(resolved.goal(), Some("build"));
    }

    #[test]
    fn resolves_goal_name_to_newest_run() {
        let farm_dir = temp_farm_dir();
        write_run(&farm_dir, "b-1", "test", "default", "2026-01-01T10:00:00+00:00");
        write_run(&farm_dir, "b-2", "test", "default", "2026-01-01T11:00:00+00:00");

        assert_eq!(resolve(&farm_dir, "test", None).unwrap().build_id, "b-2");
        assert!(resolve(&farm_dir, "nonexistent", None).is_none());
    }

    /// A run killed before its stub landed has no manifest. It must still be
    /// listed (retention has to be able to see it) but must not match a goal
    /// filter, since its goal is unknown.
    #[test]
    fn manifest_less_run_is_listed_but_not_matched_by_goal() {
        let farm_dir = temp_farm_dir();
        write_run(&farm_dir, "b-1", "test", "default", "2026-01-01T10:00:00+00:00");
        std::fs::create_dir_all(farm_dir.join("run").join("orphan")).unwrap();

        let all = list_runs(&farm_dir);
        assert_eq!(all.len(), 2);

        let orphan = all.iter().find(|r| r.build_id == "orphan").unwrap();
        assert_eq!(orphan.goal(), None);
        assert_eq!(orphan.status(), RunStatus::Running);

        let by_goal = find_runs(&farm_dir, "test", None);
        assert_eq!(by_goal.len(), 1);
    }

    fn select_id(farm_dir: &Path, spec: &str) -> Result<String, String> {
        select(farm_dir, Some(spec), None).map(|sel| sel.run().build_id.clone())
    }

    /// Three `build` runs and one `test` run, IDs shaped like generated ones.
    fn history() -> PathBuf {
        let farm_dir = temp_farm_dir();
        write_run(&farm_dir, "build-20260101T100000.000Z-aaa111", "build", "default", "2026-01-01T10:00:00+00:00");
        write_run(&farm_dir, "build-20260101T110000.000Z-bbb222", "build", "default", "2026-01-01T11:00:00+00:00");
        write_run(&farm_dir, "test-20260101T113000.000Z-ccc333", "test", "default", "2026-01-01T11:30:00+00:00");
        write_run(&farm_dir, "build-20260101T120000.000Z-ddd444", "build", "release", "2026-01-01T12:00:00+00:00");
        farm_dir
    }

    #[test]
    fn select_by_index() {
        let farm_dir = history();
        assert_eq!(select_id(&farm_dir, "@0").unwrap(), "build-20260101T120000.000Z-ddd444");
        assert_eq!(select_id(&farm_dir, "@1").unwrap(), "test-20260101T113000.000Z-ccc333");
        assert_eq!(select_id(&farm_dir, "build@0").unwrap(), "build-20260101T120000.000Z-ddd444");
        assert_eq!(select_id(&farm_dir, "build@2").unwrap(), "build-20260101T100000.000Z-aaa111");

        let err = select_id(&farm_dir, "build@3").unwrap_err();
        assert!(err.contains("out of range") && err.contains("@0 to @2"), "{err}");
        let err = select_id(&farm_dir, "@x").unwrap_err();
        assert!(err.contains("Invalid run index"), "{err}");
        let err = select_id(&farm_dir, "nope@0").unwrap_err();
        assert!(err.contains("Recorded goals: build, test"), "{err}");
    }

    #[test]
    fn select_by_partial_id() {
        let farm_dir = history();
        // Suffix (the short ID), prefix, and the exact ID.
        assert_eq!(select_id(&farm_dir, "bbb222").unwrap(), "build-20260101T110000.000Z-bbb222");
        assert_eq!(select_id(&farm_dir, "test-2026").unwrap(), "test-20260101T113000.000Z-ccc333");
        assert_eq!(
            select_id(&farm_dir, "build-20260101T100000.000Z-aaa111").unwrap(),
            "build-20260101T100000.000Z-aaa111"
        );

        // Too short to be taken for an ID, and ambiguous prefixes are refused.
        assert!(select_id(&farm_dir, "222").unwrap_err().contains("No run matches"));
        let err = select_id(&farm_dir, "build-2026").unwrap_err();
        assert!(err.contains("matches 3 runs"), "{err}");
    }

    /// Selecting a run by ID scopes it to its goal, so "previous run" means
    /// the previous run of the *same goal*.
    #[test]
    fn id_selection_is_scoped_to_its_goal() {
        let farm_dir = history();
        let sel = select(&farm_dir, Some("bbb222"), None).unwrap();
        assert_eq!(sel.index, 1);
        assert!(sel.scope.iter().all(|r| r.goal() == Some("build")));
        assert_eq!(sel.scope[sel.index + 1].build_id, "build-20260101T100000.000Z-aaa111");
    }

    #[test]
    fn select_respects_variant() {
        let farm_dir = history();
        let sel = select(&farm_dir, Some("build"), Some("default")).unwrap();
        assert_eq!(sel.run().build_id, "build-20260101T110000.000Z-bbb222");
        assert_eq!(sel.scope.len(), 2);
        // A run of another variant is not reachable by ID under --variant.
        assert!(select(&farm_dir, Some("ddd444"), Some("default")).is_err());
        assert!(select(&farm_dir, None, Some("debug")).unwrap_err().contains("variant 'debug'"));
    }

    #[test]
    fn short_id_is_the_tail() {
        let farm_dir = history();
        let run = select(&farm_dir, Some("@0"), None).unwrap().run().clone();
        assert_eq!(run.short_id(), "ddd444");
        let farm_dir = temp_farm_dir();
        write_run(&farm_dir, "ci-7", "build", "default", "2026-01-01T10:00:00+00:00");
        assert_eq!(list_runs(&farm_dir)[0].short_id(), "ci-7");
    }

    #[test]
    fn missing_run_root_is_empty_not_an_error() {
        let farm_dir = temp_farm_dir();
        assert!(list_runs(&farm_dir).is_empty());
        assert!(latest(&farm_dir).is_none());
    }
}
