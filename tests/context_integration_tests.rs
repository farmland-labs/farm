//! SPDX-License-Identifier: MIT OR Apache-2.0

//! Integration tests for farm context system
//!
//! Tests cover:
//! - Context creation, switching, listing, deletion
//! - Goal and State management
//! - Operation initialization and directory structure
//! - Path mapping for parcels
//! - Multi-context workflows
//! - CLI command integration

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

use farm::context::{
    self, ContextPaths, Goal, State, ExecutionStatus, VcsHint,
};

// =============================================================================
// Test Helpers
// =============================================================================

/// Helper to create test workspace
struct TestWorkspace {
    dir: TempDir,
}

/// Clear leaked `farm` env vars once per test process.
///
/// A developer's shell or an external launcher exports several
/// `FARM_*` vars that would otherwise override the workspace state
/// these tests carefully construct in a `TempDir`. Two were observed
/// in practice (`FARM_CTX`, `FARM_WORKSPACE`) and the others are
/// listed for completeness — anything the `farm` CLI reads from env
/// (`grep -rn 'env::var("FARM_'` in farm/src) is a potential leaker.
///
/// - `FARM_WORKSPACE` short-circuits cwd-based workspace detection
///   in the spawned `farm` subprocess; without removing it the
///   subprocess operates against the developer's workspace instead
///   of our tempdir, and assertions like
///   `ws.dir_exists(".farm/ctx/cli-ctx")` fail because the dir was
///   created, just not where we look.
/// - `FARM_CTX` shadows the `.farm/ctx/current` file these tests
///   write — `current_context_name` checks the env var first.
/// - `FARM_OPS_DIR` / `FARM_BUILD_ID` / `FARM_FARMFILE` /
///   `FARM_VARIANT` / `FARM_TARGET` could similarly redirect the
///   CLI to surfaces the tests don't control.
///
/// Removing once at startup is safe because no test in this crate
/// *sets* any of these, so there is no concurrent writer to race
/// with. Subprocesses spawned after the removal inherit the cleaned
/// env.
fn isolate_test_env() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        for var in [
            "FARM_CTX",
            "FARM_WORKSPACE",
            "FARM_OPS_DIR",
            "FARM_BUILD_ID",
            "FARM_FARMFILE",
            "FARM_VARIANT",
            "FARM_TARGET",
        ] {
            std::env::remove_var(var);
        }
    });
}

impl TestWorkspace {
    fn new() -> Self {
        isolate_test_env();
        let dir = TempDir::new().expect("Failed to create temp dir");
        Self { dir }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn read_file(&self, name: &str) -> String {
        fs::read_to_string(self.dir.path().join(name)).unwrap_or_default()
    }

    fn file_exists(&self, name: &str) -> bool {
        self.dir.path().join(name).exists()
    }

    fn dir_exists(&self, name: &str) -> bool {
        self.dir.path().join(name).is_dir()
    }

    /// Initialize .farm directory structure
    fn init_farm(&self) {
        fs::create_dir_all(self.dir.path().join(".farm")).expect("Failed to create .farm");
    }

    /// Run farm CLI command
    fn run_farm(&self, args: &[&str]) -> FarmResult {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_farm"));
        cmd.current_dir(self.dir.path());
        for arg in args {
            cmd.arg(arg);
        }
        let output = cmd.output().expect("Failed to execute farm");
        FarmResult {
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        }
    }
}

struct FarmResult {
    success: bool,
    stdout: String,
    stderr: String,
}

// =============================================================================
// Context Creation Tests
// =============================================================================

#[test]
fn test_create_context_creates_directories() {
    let ws = TestWorkspace::new();
    ws.init_farm();

    let ctx = context::create_context(ws.path(), "feature-x").unwrap();
    let op_paths = context::init_run(ws.path(), context::DEFAULT_BUILD_ID, &ctx.name).unwrap();

    assert!(ctx.exists(), "Context should exist after creation");
    assert!(ws.dir_exists(".farm/ctx/feature-x"), "Context directory should exist");
    // Per-build state lives under run/{build_id}/, not under ctx/.
    assert!(op_paths.run_dir.exists(), "run dir should exist after init_run");
    assert!(op_paths.in_dir.exists(), "in dir should exist (under run/)");
    assert!(op_paths.out_dir.exists(), "out dir should exist (under run/)");
    assert!(ws.dir_exists(".farm/ctx/feature-x/mnt"), "mnt directory should exist (ctx-shared cache)");
}

#[test]
fn test_create_context_no_goal_initially() {
    let ws = TestWorkspace::new();
    ws.init_farm();

    let ctx = context::create_context(ws.path(), "test-ctx").unwrap();
    let _op_paths = context::init_run(ws.path(), context::DEFAULT_BUILD_ID, &ctx.name).unwrap();

    // goal.json should NOT exist until farm run is invoked
    assert!(!ws.file_exists(".farm/ctx/test-ctx/goal.json"),
            "goal.json should not exist until farm run");

    let goal = context::read_goal(&ctx).unwrap();
    assert!(goal.is_none(), "Goal should be None initially");
}

#[test]
fn test_create_context_duplicate_fails() {
    let ws = TestWorkspace::new();
    ws.init_farm();

    context::create_context(ws.path(), "dup-ctx").unwrap();
    let result = context::create_context(ws.path(), "dup-ctx");

    assert!(result.is_err(), "Creating duplicate context should fail");
}

// =============================================================================
// Context Switching Tests
// =============================================================================

#[test]
fn test_switch_context_updates_current() {
    let ws = TestWorkspace::new();
    ws.init_farm();

    context::create_context(ws.path(), "ctx-a").unwrap();
    context::create_context(ws.path(), "ctx-b").unwrap();

    context::switch_context(ws.path(), "ctx-a").unwrap();
    assert_eq!(context::current_context_name(ws.path()), "ctx-a");

    context::switch_context(ws.path(), "ctx-b").unwrap();
    assert_eq!(context::current_context_name(ws.path()), "ctx-b");
}

#[test]
fn test_switch_nonexistent_context_fails() {
    let ws = TestWorkspace::new();
    ws.init_farm();

    let result = context::switch_context(ws.path(), "nonexistent");
    assert!(result.is_err(), "Switching to nonexistent context should fail");
}

#[test]
fn test_current_context_default_without_farm() {
    let ws = TestWorkspace::new();
    // Without .farm and without a git repo, the resolver falls
    // through to FARM_DEFAULT_CTX_NAME ("_main"). Picked
    // deliberately so the fallback can never be confused with a
    // real `main` branch — see the constant's comment.
    assert_eq!(
        context::current_context_name(ws.path()),
        context::FARM_DEFAULT_CTX_NAME,
    );
}

// =============================================================================
// Context Listing Tests
// =============================================================================

#[test]
fn test_list_contexts_empty() {
    let ws = TestWorkspace::new();
    ws.init_farm();

    let contexts = context::list_contexts(ws.path()).unwrap();
    assert!(contexts.is_empty(), "Should have no contexts initially");
}

#[test]
fn test_list_contexts_multiple() {
    let ws = TestWorkspace::new();
    ws.init_farm();

    context::create_context(ws.path(), "alpha").unwrap();
    context::create_context(ws.path(), "beta").unwrap();
    context::create_context(ws.path(), "gamma").unwrap();

    let contexts = context::list_contexts(ws.path()).unwrap();
    assert_eq!(contexts.len(), 3);

    // Should be sorted alphabetically
    assert_eq!(contexts[0].0, "alpha");
    assert_eq!(contexts[1].0, "beta");
    assert_eq!(contexts[2].0, "gamma");

    // No goals set yet
    assert!(contexts[0].1.is_none());
    assert!(contexts[1].1.is_none());
    assert!(contexts[2].1.is_none());
}

#[test]
fn test_list_contexts_with_goals() {
    let ws = TestWorkspace::new();
    ws.init_farm();

    let ctx_a = context::create_context(ws.path(), "with-goal").unwrap();
    context::create_context(ws.path(), "without-goal").unwrap();

    // Write a goal to first context
    let goal = Goal::new("build", Some("debug"), "Farmfile", None);
    context::write_goal(&ctx_a, &goal).unwrap();

    let contexts = context::list_contexts(ws.path()).unwrap();
    assert_eq!(contexts.len(), 2);

    // with-goal should have a goal
    let (name, goal_opt) = &contexts[0];
    assert_eq!(name, "with-goal");
    assert!(goal_opt.is_some());
    let g = goal_opt.as_ref().unwrap();
    assert_eq!(g.target, "build");
    assert_eq!(g.variant.as_deref(), Some("debug"));

    // without-goal should have None
    assert!(contexts[1].1.is_none());
}

// =============================================================================
// Context Deletion Tests
// =============================================================================

#[test]
fn test_delete_context() {
    let ws = TestWorkspace::new();
    ws.init_farm();

    context::create_context(ws.path(), "to-delete").unwrap();
    assert!(ws.dir_exists(".farm/ctx/to-delete"));

    context::delete_context(ws.path(), "to-delete").unwrap();
    assert!(!ws.dir_exists(".farm/ctx/to-delete"));
}

#[test]
fn test_delete_current_context_fails() {
    let ws = TestWorkspace::new();
    ws.init_farm();

    context::create_context(ws.path(), "current-ctx").unwrap();
    context::switch_context(ws.path(), "current-ctx").unwrap();

    let result = context::delete_context(ws.path(), "current-ctx");
    assert!(result.is_err(), "Deleting current context should fail");
}

#[test]
fn test_delete_nonexistent_context_fails() {
    let ws = TestWorkspace::new();
    ws.init_farm();

    let result = context::delete_context(ws.path(), "ghost");
    assert!(result.is_err(), "Deleting nonexistent context should fail");
}

// =============================================================================
// Goal Management Tests
// =============================================================================

#[test]
fn test_goal_write_and_read() {
    let ws = TestWorkspace::new();
    ws.init_farm();

    let ctx = context::create_context(ws.path(), "goal-test").unwrap();
    let _op_paths = context::init_run(ws.path(), context::DEFAULT_BUILD_ID, &ctx.name).unwrap();

    let goal = Goal::new("test", Some("release"), "Farmfile.release", None);
    context::write_goal(&ctx, &goal).unwrap();

    assert!(ws.file_exists(".farm/ctx/goal-test/goal.json"));

    let read_goal = context::read_goal(&ctx).unwrap().unwrap();
    assert_eq!(read_goal.target, "test");
    assert_eq!(read_goal.variant.as_deref(), Some("release"));
    assert_eq!(read_goal.farmfile, "Farmfile.release");
}

#[test]
fn test_goal_with_vcs_hint() {
    let ws = TestWorkspace::new();
    ws.init_farm();

    let ctx = context::create_context(ws.path(), "vcs-goal").unwrap();
    let _op_paths = context::init_run(ws.path(), context::DEFAULT_BUILD_ID, &ctx.name).unwrap();

    let vcs = VcsHint {
        vcs_type: "git".to_string(),
        branch: Some("feature-xyz".to_string()),
        commit: Some("abc123def".to_string()),
    };
    let goal = Goal::new("build", None, "Farmfile", Some(vcs));
    context::write_goal(&ctx, &goal).unwrap();

    let read_goal = context::read_goal(&ctx).unwrap().unwrap();
    let vcs_hint = read_goal.vcs_hint.unwrap();
    assert_eq!(vcs_hint.vcs_type, "git");
    assert_eq!(vcs_hint.branch.as_deref(), Some("feature-xyz"));
    assert_eq!(vcs_hint.commit.as_deref(), Some("abc123def"));
}

#[test]
fn test_goal_json_format() {
    let ws = TestWorkspace::new();
    ws.init_farm();

    let ctx = context::create_context(ws.path(), "json-test").unwrap();
    let _op_paths = context::init_run(ws.path(), context::DEFAULT_BUILD_ID, &ctx.name).unwrap();
    let goal = Goal::new("deploy", Some("prod"), "Farmfile.prod", None);
    context::write_goal(&ctx, &goal).unwrap();

    let content = ws.read_file(".farm/ctx/json-test/goal.json");
    assert!(content.contains("\"target\": \"deploy\""));
    assert!(content.contains("\"variant\": \"prod\""));
    assert!(content.contains("\"farmfile\": \"Farmfile.prod\""));
    assert!(content.contains("\"invoked_at\""));
}

// =============================================================================
// State Management Tests
// =============================================================================

#[test]
fn test_current_state_write_and_read() {
    let ws = TestWorkspace::new();
    ws.init_farm();

    let ctx = context::create_context(ws.path(), "state-test").unwrap();
    let _op_paths = context::init_run(ws.path(), context::DEFAULT_BUILD_ID, &ctx.name).unwrap();

    let state = State::running("compile");
    context::write_state(&ctx, &state).unwrap();

    assert!(ws.file_exists(".farm/ctx/state-test/state.json"));

    let read_state = context::read_state(&ctx).unwrap().unwrap();
    assert_eq!(read_state.operation, "compile");
    assert_eq!(read_state.status, ExecutionStatus::Running);
}

#[test]
fn test_current_state_transitions() {
    let ws = TestWorkspace::new();
    ws.init_farm();

    let ctx = context::create_context(ws.path(), "transition").unwrap();
    let _op_paths = context::init_run(ws.path(), context::DEFAULT_BUILD_ID, &ctx.name).unwrap();

    // Start running
    context::write_state(&ctx, &State::running("prepare")).unwrap();
    let state = context::read_state(&ctx).unwrap().unwrap();
    assert_eq!(state.status, ExecutionStatus::Running);

    // Move to next operation
    context::write_state(&ctx, &State::running("build")).unwrap();
    let state = context::read_state(&ctx).unwrap().unwrap();
    assert_eq!(state.operation, "build");

    // Complete
    context::write_state(&ctx, &State::completed("build")).unwrap();
    let state = context::read_state(&ctx).unwrap().unwrap();
    assert_eq!(state.status, ExecutionStatus::Completed);

    // Or fail
    context::write_state(&ctx, &State::failed("test")).unwrap();
    let state = context::read_state(&ctx).unwrap().unwrap();
    assert_eq!(state.status, ExecutionStatus::Failed);
}

#[test]
fn test_current_state_json_format() {
    let ws = TestWorkspace::new();
    ws.init_farm();

    let ctx = context::create_context(ws.path(), "json-state").unwrap();
    let _op_paths = context::init_run(ws.path(), context::DEFAULT_BUILD_ID, &ctx.name).unwrap();
    context::write_state(&ctx, &State::completed("final")).unwrap();

    let content = ws.read_file(".farm/ctx/json-state/state.json");
    assert!(content.contains("\"operation\": \"final\""));
    assert!(content.contains("\"status\": \"completed\""));
    assert!(content.contains("\"updated_at\""));
}

// =============================================================================
// Build Initialization Tests
// =============================================================================

#[test]
fn test_init_run_creates_structure() {
    let ws = TestWorkspace::new();
    ws.init_farm();

    let ctx = context::create_context(ws.path(), "build-test").unwrap();
    let paths = context::init_run(ws.path(), context::DEFAULT_BUILD_ID, &ctx.name).unwrap();

    // Build state lives under .farm/run/{build_id}/.
    assert!(paths.run_dir.exists());
    assert!(paths.in_dir.exists());
    assert!(paths.out_dir.exists());
    assert!(paths.log_dir.exists());
    assert!(paths.external_dir().exists());

    // Shared ctx-level mnt/ also created (cross-build parcel cache).
    assert!(paths.mnt_dir.exists());
    assert!(ws.dir_exists(".farm/ctx/build-test/mnt"));
}

#[test]
fn test_init_run_writes_no_state_or_context_json() {
    // init_run is purely directory-prep — state.json is written by
    // the build engine when execution begins, and context.json is
    // written by the orchestrator. Locking this in so the
    // separation of concerns doesn't drift back.
    let ws = TestWorkspace::new();
    ws.init_farm();

    let ctx = context::create_context(ws.path(), "no-side-effects").unwrap();
    let _paths = context::init_run(ws.path(), context::DEFAULT_BUILD_ID, &ctx.name).unwrap();

    assert!(!ws.file_exists(&format!(
        ".farm/run/{}/context.json",
        context::DEFAULT_BUILD_ID
    )));
    assert!(!ws.file_exists(".farm/ctx/no-side-effects/state.json"));
}

#[test]
fn test_init_run_auto_creates_default_context() {
    let ws = TestWorkspace::new();
    ws.init_farm();

    let op_paths = context::init_run(ws.path(), context::DEFAULT_BUILD_ID, context::FARM_DEFAULT_CTX_NAME).unwrap();

    // Default context falls through to FARM_DEFAULT_CTX_NAME
    // ("_main"). The shared ctx dir (mnt cache) is auto-
    // created; the build run dir is also created.
    assert_eq!(op_paths.ctx_name, context::FARM_DEFAULT_CTX_NAME);
    assert!(ws.dir_exists(&format!(
        ".farm/ctx/{}/mnt",
        context::FARM_DEFAULT_CTX_NAME
    )));
    assert!(ws.dir_exists(&format!(
        ".farm/run/{}",
        context::DEFAULT_BUILD_ID
    )));
}

// =============================================================================
// Path Mapping Tests
// =============================================================================

// ContextPaths-shape tests below mirror the path conventions
// exercised more thoroughly by the lib unit-tests in
// `farm/src/context/mod.rs` (see `test_context_paths`,
// `test_mount_path`). Kept here as a thin integration-side smoke check.

#[test]
fn test_context_paths_structure() {
    let ctx = ContextPaths::new(Path::new("/workspace"), "myctx");

    assert_eq!(ctx.name, "myctx");
    assert_eq!(ctx.ctx_dir, PathBuf::from("/workspace/.farm/ctx/myctx"));
    assert_eq!(ctx.mnt_dir, PathBuf::from("/workspace/.farm/ctx/myctx/mnt"));
}

#[test]
fn test_mount_path_without_variant() {
    let ctx = ContextPaths::new(Path::new("/ws"), "test");

    let path = ctx.mount_path("build", "model", None);
    assert_eq!(path, PathBuf::from("/ws/.farm/ctx/test/mnt/build_model"));
}

#[test]
fn test_mount_path_with_variant() {
    let ctx = ContextPaths::new(Path::new("/ws"), "test");

    let path = ctx.mount_path("build", "model", Some("debug"));
    assert_eq!(path, PathBuf::from("/ws/.farm/ctx/test/mnt/build_model_debug"));
}

// =============================================================================
// Multi-Context Workflow Tests
// =============================================================================

#[test]
fn test_goal_per_context() {
    let ws = TestWorkspace::new();
    ws.init_farm();

    let ctx_a = context::create_context(ws.path(), "goal-a").unwrap();
    let ctx_b = context::create_context(ws.path(), "goal-b").unwrap();

    context::write_goal(&ctx_a, &Goal::new("build", Some("debug"), "f1", None)).unwrap();
    context::write_goal(&ctx_b, &Goal::new("test", Some("release"), "f2", None)).unwrap();

    let goal_a = context::read_goal(&ctx_a).unwrap().unwrap();
    let goal_b = context::read_goal(&ctx_b).unwrap().unwrap();

    assert_eq!(goal_a.target, "build");
    assert_eq!(goal_a.variant.as_deref(), Some("debug"));
    assert_eq!(goal_b.target, "test");
    assert_eq!(goal_b.variant.as_deref(), Some("release"));
}

#[test]
fn test_switch_preserves_state() {
    let ws = TestWorkspace::new();
    ws.init_farm();

    let ctx_a = context::create_context(ws.path(), "preserve-a").unwrap();
    let _ctx_b = context::create_context(ws.path(), "preserve-b").unwrap();

    // Set up state in ctx-a
    context::write_goal(&ctx_a, &Goal::new("build", None, "f", None)).unwrap();
    context::write_state(&ctx_a, &State::completed("build")).unwrap();
    context::switch_context(ws.path(), "preserve-a").unwrap();

    // Switch to b
    context::switch_context(ws.path(), "preserve-b").unwrap();
    assert_eq!(context::current_context_name(ws.path()), "preserve-b");

    // Switch back to a - state should still be there
    context::switch_context(ws.path(), "preserve-a").unwrap();
    let goal = context::read_goal(&ctx_a).unwrap().unwrap();
    let state = context::read_state(&ctx_a).unwrap().unwrap();

    assert_eq!(goal.target, "build");
    assert_eq!(state.status, ExecutionStatus::Completed);
}

// =============================================================================
// VCS Detection Tests
// =============================================================================

#[test]
fn test_vcs_mismatch_detection() {
    let ws = TestWorkspace::new();
    ws.init_farm();

    let ctx = context::create_context(ws.path(), "vcs-test").unwrap();
    let _op_paths = context::init_run(ws.path(), context::DEFAULT_BUILD_ID, &ctx.name).unwrap();

    // Create goal with VCS hint
    let vcs = VcsHint {
        vcs_type: "git".to_string(),
        branch: Some("old-branch".to_string()),
        commit: Some("old-commit".to_string()),
    };
    let goal = Goal::new("build", None, "f", Some(vcs));
    context::write_goal(&ctx, &goal).unwrap();

    // Simulate different current VCS (since we don't have real git)
    // The function needs current VCS to compare, which won't exist in temp dir
    let result = context::check_vcs_mismatch(ws.path(), &goal);
    // Without git in temp dir, detect_vcs returns None, so no mismatch
    assert!(result.is_none(), "No mismatch when current VCS not detected");
}

// =============================================================================
// CLI Integration Tests
// =============================================================================

#[test]
fn test_cli_ctx_create() {
    let ws = TestWorkspace::new();
    ws.init_farm();

    let result = ws.run_farm(&["ctx", "branch", "init", "--name", "cli-ctx"]);
    assert!(result.success, "ctx branch init should succeed: {}", result.stderr);
    assert!(ws.dir_exists(".farm/ctx/cli-ctx"));
}

#[test]
fn test_cli_ctx_list_empty() {
    let ws = TestWorkspace::new();
    ws.init_farm();

    let result = ws.run_farm(&["ctx", "branch", "list"]);
    assert!(result.success);
    // Empty-state copy is: "No branch contexts found. ..."
    assert!(
        result.stdout.contains("No branch contexts found")
            || result.stderr.contains("No branch contexts found"),
        "expected empty-state hint, got stdout={:?} stderr={:?}",
        result.stdout,
        result.stderr,
    );
}

#[test]
fn test_cli_ctx_list_multiple() {
    let ws = TestWorkspace::new();
    ws.init_farm();

    context::create_context(ws.path(), "alpha").unwrap();
    context::create_context(ws.path(), "beta").unwrap();

    let result = ws.run_farm(&["ctx", "branch", "list"]);
    assert!(result.success);
    assert!(result.stdout.contains("alpha"));
    assert!(result.stdout.contains("beta"));
}

#[test]
fn test_cli_ctx_switch() {
    let ws = TestWorkspace::new();
    ws.init_farm();

    context::create_context(ws.path(), "target-ctx").unwrap();

    let result = ws.run_farm(&["ctx", "branch", "use", "target-ctx"]);
    assert!(result.success, "ctx branch use should succeed: {}", result.stderr);

    assert_eq!(context::current_context_name(ws.path()), "target-ctx");
}

#[test]
fn test_cli_ctx_current() {
    let ws = TestWorkspace::new();
    ws.init_farm();

    context::create_context(ws.path(), "current-test").unwrap();
    context::switch_context(ws.path(), "current-test").unwrap();

    let result = ws.run_farm(&["ctx", "branch", "show", "--format", "name"]);
    assert!(result.success);
    assert!(result.stdout.trim() == "current-test");
}

#[test]
fn test_cli_ctx_delete() {
    let ws = TestWorkspace::new();
    ws.init_farm();

    context::create_context(ws.path(), "to-del").unwrap();
    // Make sure we're not on that context
    context::create_context(ws.path(), "other").unwrap();
    context::switch_context(ws.path(), "other").unwrap();

    let result = ws.run_farm(&["ctx", "branch", "delete", "to-del", "-y"]);
    assert!(result.success, "ctx branch delete should succeed: {}", result.stderr);
    assert!(!ws.dir_exists(".farm/ctx/to-del"));
}

// =============================================================================
// Edge Cases and Error Handling
// =============================================================================

#[test]
fn test_special_characters_in_names() {
    let ws = TestWorkspace::new();
    ws.init_farm();

    // Context names with hyphens and underscores should work
    let ctx = context::create_context(ws.path(), "my-feature_v2").unwrap();
    let _op_paths = context::init_run(ws.path(), context::DEFAULT_BUILD_ID, &ctx.name).unwrap();
    assert!(ctx.exists());

    // Operation names with hyphens
    let op = context::init_run(ws.path(), context::DEFAULT_BUILD_ID, &ctx.name).unwrap();
    assert!(op.run_dir.exists());
}

// =============================================================================
// Concurrent Access Simulation
// =============================================================================

#[test]
fn test_multiple_builds_in_context() {
    // Multiple builds (different build_ids) coexist under the
    // same ctx with isolated run/{build_id}/ directories.
    let ws = TestWorkspace::new();
    ws.init_farm();

    let ctx = context::create_context(ws.path(), "multi-build").unwrap();

    context::init_run(ws.path(), "prepare-build", &ctx.name).unwrap();
    context::init_run(ws.path(), "main-build", &ctx.name).unwrap();
    context::init_run(ws.path(), "test-build", &ctx.name).unwrap();

    // Each build has its own isolated run dir.
    assert!(ws.dir_exists(".farm/run/prepare-build"));
    assert!(ws.dir_exists(".farm/run/main-build"));
    assert!(ws.dir_exists(".farm/run/test-build"));

    // mnt cache is ctx-shared across all builds.
    assert!(ws.dir_exists(".farm/ctx/multi-build/mnt"));
}

// =============================================================================
// CLI Format Tests (Public Interface Stability for Tooling)
// =============================================================================

#[test]
fn test_cli_ctx_list_with_long_format() {
    let ws = TestWorkspace::new();
    ws.init_farm();

    context::create_context(ws.path(), "feature-x").unwrap();

    let result = ws.run_farm(&["ctx", "branch", "list", "--long"]);

    assert!(result.success, "ctx list --long should succeed: {}", result.stderr);
    // Long format should show more details
    assert!(result.stdout.contains("feature-x"));
}

#[test]
fn test_cli_ctx_show_json_format() {
    let ws = TestWorkspace::new();
    ws.init_farm();

    context::create_context(ws.path(), "json-test").unwrap();
    context::switch_context(ws.path(), "json-test").unwrap();

    let result = ws.run_farm(&["ctx", "branch", "show", "--format", "json"]);

    assert!(
        result.success,
        "ctx show --format json should succeed: {}",
        result.stderr
    );

    // Parse as JSON to verify structure
    let json: serde_json::Value =
        serde_json::from_str(&result.stdout).expect("should output valid JSON");

    assert!(json.get("name").is_some(), "should have 'name' field");
}

#[test]
fn test_cli_ctx_env_export_format() {
    let ws = TestWorkspace::new();
    ws.init_farm();

    context::create_context(ws.path(), "env-test").unwrap();
    context::switch_context(ws.path(), "env-test").unwrap();

    let result = ws.run_farm(&["ctx", "env", "--format", "export"]);

    assert!(result.success, "ctx env should succeed: {}", result.stderr);
    // Export format should have shell export statements
    assert!(
        result.stdout.contains("export") || result.stdout.contains("FARM_CTX"),
        "export format should have shell exports: {}",
        result.stdout
    );
}

#[test]
fn test_cli_ctx_env_json_format() {
    let ws = TestWorkspace::new();
    ws.init_farm();

    context::create_context(ws.path(), "env-json").unwrap();
    context::switch_context(ws.path(), "env-json").unwrap();

    let result = ws.run_farm(&["ctx", "env", "--format", "json"]);

    assert!(result.success, "ctx env --format json should succeed: {}", result.stderr);

    // Parse as JSON
    let json: serde_json::Value =
        serde_json::from_str(&result.stdout).expect("should output valid JSON");

    assert!(json.is_object(), "should be a JSON object");
}
