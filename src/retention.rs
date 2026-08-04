//! SPDX-License-Identifier: MIT OR Apache-2.0

//! Run directory retention (ADR 0001).
//!
//! Every invocation now gets its own `.farm/run/{build_id}/`, so something has
//! to bound the growth. Pruning happens at the start of a build, scoped to the
//! goal and variant about to run: one directory listing, no global sweep, no
//! background process.
//!
//! The rules, in priority order:
//!
//! 1. A run that is still executing is never removed.
//! 2. The newest failed run is never removed, even when the keep count would
//!    evict it. The common pattern is one failure followed by several
//!    exploratory reruns, and plain keep-N discards precisely the run worth
//!    reading.
//! 3. Runs beyond the keep count are removed, newest first.
//! 4. Runs older than the maximum age are removed.
//!
//! Explicit `--build-id` runs prune under the same rules; exempting them would
//! leave CI unbounded, which is one of the problems this exists to fix.

use std::path::Path;
use std::time::{Duration, SystemTime};

use crate::manifest::RunStatus;
use crate::runs::{list_runs, find_runs};

/// How much run history to keep.
#[derive(Debug, Clone, Copy)]
pub struct RetentionPolicy {
    /// Number of runs to keep per goal + variant.
    pub keep: usize,
    /// Runs older than this are removed regardless of the keep count.
    pub max_age: Duration,
}

impl Default for RetentionPolicy {
    fn default() -> Self {
        Self {
            keep: 5,
            max_age: Duration::from_secs(7 * 24 * 60 * 60),
        }
    }
}

/// What a prune pass did. Callers log this; nothing depends on it.
#[derive(Debug, Default)]
pub struct PruneOutcome {
    /// Build IDs whose directories were removed.
    pub removed: Vec<String>,
    /// Runs left in place.
    pub kept: usize,
    /// Directories that could not be removed, with the reason. Never fatal —
    /// a build must not fail because housekeeping did.
    pub errors: Vec<String>,
}

/// Prune old runs of `goal` + `variant`.
///
/// Call this *after* the current run's stub manifest exists, so rule 1 protects
/// the run that is about to execute.
pub fn prune(
    farm_dir: &Path,
    goal: &str,
    variant: &str,
    policy: &RetentionPolicy,
) -> PruneOutcome {
    let runs = find_runs(farm_dir, goal, Some(variant));
    let now = SystemTime::now();
    let mut outcome = PruneOutcome::default();

    // Index of the newest failure, protected by rule 2.
    let newest_failure = runs.iter().position(|r| r.status() == RunStatus::Fail);

    for (index, run) in runs.iter().enumerate() {
        let protected =
            run.status() == RunStatus::Running || Some(index) == newest_failure;

        if protected {
            outcome.kept += 1;
            continue;
        }

        let too_old = now
            .duration_since(run.started_at())
            .map(|age| age > policy.max_age)
            .unwrap_or(false);
        let beyond_keep = outcome.kept >= policy.keep;

        if !too_old && !beyond_keep {
            outcome.kept += 1;
            continue;
        }

        match std::fs::remove_dir_all(&run.dir) {
            Ok(()) => outcome.removed.push(run.build_id.clone()),
            Err(e) => outcome
                .errors
                .push(format!("{}: {}", run.dir.display(), e)),
        }
    }

    outcome
}

/// Keep the newest `keep` runs across every goal, removing the rest.
///
/// This backs the manual `farm ctx run clean -k N`, whose contract is a global
/// count rather than the per-goal budget [`prune`] applies. The two safety rules
/// still hold: a run that is still executing is never removed, and the newest
/// failure of each goal survives. Without the first rule a cleanup issued while
/// a build is running would delete that build's directory out from under it.
pub fn prune_all(farm_dir: &Path, keep: usize) -> PruneOutcome {
    let runs = list_runs(farm_dir);
    let mut outcome = PruneOutcome::default();

    // Newest failure per goal, by build ID.
    let mut protected_failures: Vec<&str> = Vec::new();
    let mut seen_goals: Vec<&str> = Vec::new();
    for run in runs.iter().filter(|r| r.status() == RunStatus::Fail) {
        if let Some(goal) = run.goal() {
            if !seen_goals.contains(&goal) {
                seen_goals.push(goal);
                protected_failures.push(&run.build_id);
            }
        }
    }

    for run in runs.iter() {
        // `is_running` rather than `status`: a directory with no manifest — as
        // `farm ctx run init` leaves behind — must stay removable here, or an
        // explicitly requested cleanup could never clear it.
        let protected =
            run.is_running() || protected_failures.contains(&run.build_id.as_str());

        if protected || outcome.kept < keep {
            outcome.kept += 1;
            continue;
        }

        match std::fs::remove_dir_all(&run.dir) {
            Ok(()) => outcome.removed.push(run.build_id.clone()),
            Err(e) => outcome
                .errors
                .push(format!("{}: {}", run.dir.display(), e)),
        }
    }

    outcome
}

/// Remove run directories with no readable manifest once they are past
/// `max_age`.
///
/// These are runs killed before their stub landed, or directories created by
/// `farm ctx run init` for a build that never happened. They carry no goal, so
/// no goal-scoped prune will ever see them — without this pass they leak
/// forever. The age threshold is what makes it safe: a live run always has a
/// stub, so anything manifest-less and days old is certainly not running.
pub fn prune_orphans(farm_dir: &Path, policy: &RetentionPolicy) -> PruneOutcome {
    let now = SystemTime::now();
    let mut outcome = PruneOutcome::default();

    for run in list_runs(farm_dir).iter().filter(|r| r.manifest.is_none()) {
        let too_old = now
            .duration_since(run.started_at())
            .map(|age| age > policy.max_age)
            .unwrap_or(false);

        if !too_old {
            outcome.kept += 1;
            continue;
        }

        match std::fs::remove_dir_all(&run.dir) {
            Ok(()) => outcome.removed.push(run.build_id.clone()),
            Err(e) => outcome
                .errors
                .push(format!("{}: {}", run.dir.display(), e)),
        }
    }

    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::ReplayManifest;
    use std::path::PathBuf;

    fn temp_farm_dir() -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "farm-retention-test-{}",
            uuid::Uuid::now_v7().simple()
        ));
        std::fs::create_dir_all(&base).unwrap();
        base
    }

    /// Write a run directory `age` old with the given status.
    fn write_run(farm_dir: &Path, build_id: &str, status: RunStatus, age: Duration) {
        let dir = farm_dir.join("run").join(build_id);
        std::fs::create_dir_all(dir.join("log")).unwrap();

        let started = SystemTime::now() - age;
        let mut manifest =
            ReplayManifest::running(build_id, "test", "default", false, started);
        manifest.status = Some(status);
        manifest.success = status == RunStatus::Ok;
        manifest.write_to(&dir).unwrap();
    }

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    fn remaining(farm_dir: &Path) -> Vec<String> {
        let mut ids: Vec<_> = list_runs(farm_dir)
            .into_iter()
            .map(|r| r.build_id)
            .collect();
        ids.sort();
        ids
    }

    #[test]
    fn keeps_the_newest_n_runs() {
        let farm_dir = temp_farm_dir();
        for i in 0..8 {
            write_run(&farm_dir, &format!("b-{}", i), RunStatus::Ok, secs(800 - i * 100));
        }

        let policy = RetentionPolicy { keep: 3, ..Default::default() };
        let outcome = prune(&farm_dir, "test", "default", &policy);

        assert_eq!(outcome.kept, 3);
        assert_eq!(outcome.removed.len(), 5);
        // b-7 is newest (smallest age), b-0 oldest.
        assert_eq!(remaining(&farm_dir), vec!["b-5", "b-6", "b-7"]);
    }

    /// Rule 2: the failure survives even though five successes are newer.
    #[test]
    fn newest_failure_survives_the_keep_count() {
        let farm_dir = temp_farm_dir();
        write_run(&farm_dir, "the-failure", RunStatus::Fail, secs(1000));
        for i in 0..6 {
            write_run(&farm_dir, &format!("ok-{}", i), RunStatus::Ok, secs(500 - i * 50));
        }

        let policy = RetentionPolicy { keep: 3, ..Default::default() };
        prune(&farm_dir, "test", "default", &policy);

        let left = remaining(&farm_dir);
        assert!(left.contains(&"the-failure".to_string()), "got {:?}", left);
        assert_eq!(left.len(), 4, "3 kept + the protected failure");
    }

    /// Only the *newest* failure is protected; older ones are ordinary.
    #[test]
    fn older_failures_are_not_protected() {
        let farm_dir = temp_farm_dir();
        write_run(&farm_dir, "old-failure", RunStatus::Fail, secs(2000));
        write_run(&farm_dir, "new-failure", RunStatus::Fail, secs(1000));
        for i in 0..4 {
            write_run(&farm_dir, &format!("ok-{}", i), RunStatus::Ok, secs(500 - i * 50));
        }

        let policy = RetentionPolicy { keep: 2, ..Default::default() };
        prune(&farm_dir, "test", "default", &policy);

        let left = remaining(&farm_dir);
        assert!(left.contains(&"new-failure".to_string()), "got {:?}", left);
        assert!(!left.contains(&"old-failure".to_string()), "got {:?}", left);
    }

    /// Rule 1: the stub written moments ago by the run that is about to execute
    /// must survive its own prune pass.
    #[test]
    fn running_run_is_never_pruned() {
        let farm_dir = temp_farm_dir();
        write_run(&farm_dir, "in-flight", RunStatus::Running, secs(0));
        for i in 0..6 {
            write_run(&farm_dir, &format!("ok-{}", i), RunStatus::Ok, secs(500 - i * 50));
        }

        let policy = RetentionPolicy { keep: 1, ..Default::default() };
        prune(&farm_dir, "test", "default", &policy);

        assert!(remaining(&farm_dir).contains(&"in-flight".to_string()));
    }

    /// An old run still executing is protected: age must not override rule 1.
    #[test]
    fn age_does_not_evict_a_running_run() {
        let farm_dir = temp_farm_dir();
        write_run(&farm_dir, "long-runner", RunStatus::Running, secs(30 * 24 * 3600));

        let outcome = prune(&farm_dir, "test", "default", &RetentionPolicy::default());

        assert!(outcome.removed.is_empty());
        assert_eq!(remaining(&farm_dir), vec!["long-runner"]);
    }

    #[test]
    fn removes_runs_past_max_age_even_within_keep_count() {
        let farm_dir = temp_farm_dir();
        write_run(&farm_dir, "ancient", RunStatus::Ok, secs(30 * 24 * 3600));
        write_run(&farm_dir, "recent", RunStatus::Ok, secs(60));

        let policy = RetentionPolicy { keep: 10, max_age: secs(24 * 3600) };
        let outcome = prune(&farm_dir, "test", "default", &policy);

        assert_eq!(outcome.removed, vec!["ancient"]);
        assert_eq!(remaining(&farm_dir), vec!["recent"]);
    }

    #[test]
    fn other_goals_are_untouched() {
        let farm_dir = temp_farm_dir();
        for i in 0..4 {
            write_run(&farm_dir, &format!("t-{}", i), RunStatus::Ok, secs(400 - i * 50));
        }
        // A run of a different goal, well past the keep count for "test".
        let dir = farm_dir.join("run").join("other-goal");
        std::fs::create_dir_all(&dir).unwrap();
        ReplayManifest::running("other-goal", "build", "default", false, SystemTime::now())
            .write_to(&dir)
            .unwrap();

        let policy = RetentionPolicy { keep: 1, ..Default::default() };
        prune(&farm_dir, "test", "default", &policy);

        assert!(remaining(&farm_dir).contains(&"other-goal".to_string()));
    }

    #[test]
    fn orphans_are_removed_only_once_stale() {
        let farm_dir = temp_farm_dir();
        let fresh = farm_dir.join("run").join("fresh-orphan");
        let stale = farm_dir.join("run").join("stale-orphan");
        std::fs::create_dir_all(&fresh).unwrap();
        std::fs::create_dir_all(&stale).unwrap();

        // A fresh orphan could be a run that just started; only age makes it safe.
        let policy = RetentionPolicy { keep: 5, max_age: secs(3600) };
        let outcome = prune_orphans(&farm_dir, &policy);
        assert!(outcome.removed.is_empty());
        assert!(fresh.exists() && stale.exists());

        // With no age tolerance both are stale, so both go.
        let policy = RetentionPolicy { keep: 5, max_age: Duration::ZERO };
        let outcome = prune_orphans(&farm_dir, &policy);
        assert_eq!(outcome.removed.len(), 2);
        assert!(!fresh.exists() && !stale.exists());
    }

    #[test]
    fn global_prune_keeps_newest_across_goals() {
        let farm_dir = temp_farm_dir();
        for i in 0..5 {
            write_run(&farm_dir, &format!("t-{}", i), RunStatus::Ok, secs(500 - i * 50));
        }

        let outcome = prune_all(&farm_dir, 2);

        assert_eq!(outcome.kept, 2);
        assert_eq!(remaining(&farm_dir), vec!["t-3", "t-4"]);
    }

    /// The hazard per-invocation run directories introduce: a manual cleanup
    /// issued while a build is running must not delete that build's directory.
    #[test]
    fn global_prune_never_removes_a_running_build() {
        let farm_dir = temp_farm_dir();
        write_run(&farm_dir, "in-flight", RunStatus::Running, secs(1));
        for i in 0..5 {
            write_run(&farm_dir, &format!("t-{}", i), RunStatus::Ok, secs(500 - i * 50));
        }

        prune_all(&farm_dir, 1);

        assert!(remaining(&farm_dir).contains(&"in-flight".to_string()));
    }

    #[test]
    fn global_prune_protects_the_newest_failure_of_each_goal() {
        let farm_dir = temp_farm_dir();
        write_run(&farm_dir, "test-failure", RunStatus::Fail, secs(900));
        for i in 0..4 {
            write_run(&farm_dir, &format!("t-{}", i), RunStatus::Ok, secs(400 - i * 50));
        }

        // A failure belonging to a different goal must be protected separately.
        let dir = farm_dir.join("run").join("build-failure");
        std::fs::create_dir_all(&dir).unwrap();
        let mut other =
            ReplayManifest::running("build-failure", "build", "default", false, SystemTime::now() - secs(950));
        other.status = Some(RunStatus::Fail);
        other.write_to(&dir).unwrap();

        prune_all(&farm_dir, 1);

        let left = remaining(&farm_dir);
        assert!(left.contains(&"test-failure".to_string()), "got {:?}", left);
        assert!(left.contains(&"build-failure".to_string()), "got {:?}", left);
    }

    /// A run with a manifest is not an orphan, however old.
    #[test]
    fn orphan_sweep_ignores_runs_with_manifests() {
        let farm_dir = temp_farm_dir();
        write_run(&farm_dir, "has-manifest", RunStatus::Ok, secs(30 * 24 * 3600));

        let policy = RetentionPolicy { keep: 5, max_age: Duration::ZERO };
        let outcome = prune_orphans(&farm_dir, &policy);

        assert!(outcome.removed.is_empty());
        assert_eq!(remaining(&farm_dir), vec!["has-manifest"]);
    }
}
