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
}

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

/// Resolve a user-supplied run specifier.
///
/// An exact build ID wins; otherwise the newest run of a goal by that name is
/// returned. This is what keeps `farm replay test` working now that the local
/// build ID is no longer the bare goal name — before ADR 0001 the two were the
/// same string, and users typed the goal.
pub fn resolve(farm_dir: &Path, spec: &str, variant: Option<&str>) -> Option<RunEntry> {
    let runs = list_runs(farm_dir);

    if let Some(exact) = runs.iter().find(|run| run.build_id == spec) {
        return Some(exact.clone());
    }

    runs.into_iter()
        .filter(|run| run.goal() == Some(spec))
        .find(|run| variant.is_none_or(|v| run.variant() == Some(v)))
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

    #[test]
    fn missing_run_root_is_empty_not_an_error() {
        let farm_dir = temp_farm_dir();
        assert!(list_runs(&farm_dir).is_empty());
        assert!(latest(&farm_dir).is_none());
    }
}
