//! SPDX-License-Identifier: MIT OR Apache-2.0

//! Integration tests for the `farm ctx run` commands.
//!
//! Tests the CLI interface for pipeline run management that external
//! orchestrators rely on.

use std::fs;
use std::process::Command;
use tempfile::TempDir;

/// Helper to create test workspace and run farm commands
struct TestWorkspace {
    dir: TempDir,
}

impl TestWorkspace {
    fn new() -> Self {
        let dir = TempDir::new().expect("Failed to create temp dir");
        // Initialize .farm directory (required for ctx commands)
        fs::create_dir_all(dir.path().join(".farm")).expect("Failed to create .farm");
        Self { dir }
    }

    /// Run farm ctx run command
    fn ctx_run(&self, args: &[&str]) -> FarmResult {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_farm"));
        cmd.current_dir(self.dir.path())
            .args(["ctx", "run"]);

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

    fn path(&self) -> &std::path::Path {
        self.dir.path()
    }
}

struct FarmResult {
    success: bool,
    stdout: String,
    stderr: String,
}

// =============================================================================
// Init Tests
// =============================================================================

#[test]
fn test_ctx_run_init_creates_directories() {
    let ws = TestWorkspace::new();

    let result = ws.ctx_run(&["init", "--build-id", "test-build-001"]);

    assert!(result.success, "init should succeed: {}", result.stderr);
    assert!(
        result.stdout.contains("Initialized pipeline run"),
        "should show success message"
    );

    // Verify directories were created
    let run_dir = ws.path().join(".farm/run/test-build-001");
    assert!(run_dir.exists(), "run dir should exist");
    assert!(run_dir.join("in").exists(), "in dir should exist");
    assert!(run_dir.join("out").exists(), "out dir should exist");
}

#[test]
fn test_ctx_run_init_with_custom_context() {
    let ws = TestWorkspace::new();

    let result = ws.ctx_run(&["init", "--build-id", "build-002", "--ctx", "ci"]);

    assert!(result.success, "init should succeed: {}", result.stderr);
    assert!(result.stdout.contains("Context:   ci"), "should show ci context");

    // Verify mount dir is in the right context
    let mnt_dir = ws.path().join(".farm/ctx/ci/mnt");
    assert!(mnt_dir.exists(), "mount dir should exist in ci context");
}

#[test]
fn test_ctx_run_init_format_json() {
    let ws = TestWorkspace::new();

    let result = ws.ctx_run(&["init", "--build-id", "build-json", "--format", "json"]);

    assert!(
        result.success,
        "init with json format should succeed: {}",
        result.stderr
    );

    // Parse JSON output
    let json: serde_json::Value =
        serde_json::from_str(&result.stdout).expect("should output valid JSON");

    // Verify required fields for scripting
    assert_eq!(json["build_id"], "build-json");
    assert!(json["run_dir"].is_string(), "should have run_dir");
    assert!(json["in_dir"].is_string(), "should have in_dir");
    assert!(json["out_dir"].is_string(), "should have out_dir");
    assert!(json["mnt_dir"].is_string(), "should have mnt_dir");
    assert!(json["ctx_name"].is_string(), "should have ctx_name");
}

#[test]
fn test_ctx_run_init_shows_env_vars() {
    let ws = TestWorkspace::new();

    let result = ws.ctx_run(&["init", "--build-id", "build-env"]);

    assert!(result.success);
    assert!(
        result.stdout.contains("FARM_BUILD_ID=build-env"),
        "should show FARM_BUILD_ID"
    );
    assert!(
        result.stdout.contains("FARM_CTX="),
        "should show FARM_CTX"
    );
}

// =============================================================================
// Show Tests
// =============================================================================

#[test]
fn test_ctx_run_show_existing_run() {
    let ws = TestWorkspace::new();

    // First init
    let init_result = ws.ctx_run(&["init", "--build-id", "show-test"]);
    assert!(init_result.success);

    // Then show
    let result = ws.ctx_run(&["show", "--build-id", "show-test"]);

    assert!(result.success, "show should succeed: {}", result.stderr);
    assert!(
        result.stdout.contains("Pipeline Run: show-test"),
        "should show build id"
    );
}

#[test]
fn test_ctx_run_show_nonexistent_fails() {
    let ws = TestWorkspace::new();

    let result = ws.ctx_run(&["show", "--build-id", "does-not-exist"]);

    assert!(!result.success, "show should fail for nonexistent run");
    assert!(
        result.stderr.contains("not found"),
        "should mention not found"
    );
}

#[test]
fn test_ctx_run_show_format_json() {
    let ws = TestWorkspace::new();

    // First init
    ws.ctx_run(&["init", "--build-id", "show-json"]);

    // Then show with json
    let result = ws.ctx_run(&["show", "--build-id", "show-json", "--format", "json"]);

    assert!(
        result.success,
        "show json should succeed: {}",
        result.stderr
    );

    let json: serde_json::Value =
        serde_json::from_str(&result.stdout).expect("should output valid JSON");

    // Same structure as init JSON
    assert_eq!(json["build_id"], "show-json");
    assert!(json["run_dir"].is_string());
    assert!(json["in_dir"].is_string());
    assert!(json["out_dir"].is_string());
}

// =============================================================================
// List Tests
// =============================================================================

#[test]
fn test_ctx_run_list_empty() {
    let ws = TestWorkspace::new();

    let result = ws.ctx_run(&["list"]);

    assert!(result.success);
    assert!(
        result.stdout.contains("No runs found"),
        "should say no runs: {}",
        result.stdout
    );
}

#[test]
fn test_ctx_run_list_multiple() {
    let ws = TestWorkspace::new();

    // Create multiple runs
    ws.ctx_run(&["init", "--build-id", "run-a"]);
    ws.ctx_run(&["init", "--build-id", "run-b"]);
    ws.ctx_run(&["init", "--build-id", "run-c"]);

    let result = ws.ctx_run(&["list"]);

    assert!(result.success);
    assert!(result.stdout.contains("run-a"), "should list run-a");
    assert!(result.stdout.contains("run-b"), "should list run-b");
    assert!(result.stdout.contains("run-c"), "should list run-c");
}

// =============================================================================
// Clean Tests
// =============================================================================

#[test]
fn test_ctx_run_clean_requires_confirmation() {
    let ws = TestWorkspace::new();

    // Create some runs
    ws.ctx_run(&["init", "--build-id", "clean-test"]);

    // Try to clean without -y
    let result = ws.ctx_run(&["clean"]);

    assert!(!result.success, "clean should require confirmation");
    assert!(
        result.stderr.contains("-y") || result.stderr.contains("confirm"),
        "should mention -y flag"
    );
}

#[test]
fn test_ctx_run_clean_keeps_recent() {
    let ws = TestWorkspace::new();

    // Create 5 runs
    for i in 1..=5 {
        ws.ctx_run(&["init", "--build-id", &format!("clean-{}", i)]);
    }

    // Clean, keeping 2
    let result = ws.ctx_run(&["clean", "--keep", "2", "-y"]);

    assert!(result.success, "clean should succeed: {}", result.stderr);
    assert!(
        result.stdout.contains("Cleaned up"),
        "should confirm cleanup"
    );

    // Verify runs directory
    let run_dir = ws.path().join(".farm/run");
    let runs: Vec<_> = fs::read_dir(&run_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();

    assert_eq!(runs.len(), 2, "should keep 2 most recent runs");
}

// =============================================================================
// JSON Stability Tests (for tooling)
// =============================================================================

#[test]
fn test_ctx_run_json_has_stable_fields() {
    let ws = TestWorkspace::new();

    let result = ws.ctx_run(&["init", "--build-id", "stable-fields", "--format", "json"]);
    assert!(result.success);

    let json: serde_json::Value = serde_json::from_str(&result.stdout).unwrap();

    // These fields MUST exist for external tooling
    assert!(json.get("build_id").is_some(), "must have 'build_id'");
    assert!(json.get("ctx_name").is_some(), "must have 'ctx_name'");
    assert!(json.get("run_dir").is_some(), "must have 'run_dir'");
    assert!(json.get("in_dir").is_some(), "must have 'in_dir'");
    assert!(json.get("out_dir").is_some(), "must have 'out_dir'");
    assert!(json.get("mnt_dir").is_some(), "must have 'mnt_dir'");
}

#[test]
fn test_ctx_run_json_paths_are_absolute() {
    let ws = TestWorkspace::new();

    let result = ws.ctx_run(&["init", "--build-id", "absolute-paths", "--format", "json"]);
    assert!(result.success);

    let json: serde_json::Value = serde_json::from_str(&result.stdout).unwrap();

    // All paths should be absolute
    let run_dir = json["run_dir"].as_str().unwrap();
    let in_dir = json["in_dir"].as_str().unwrap();
    let out_dir = json["out_dir"].as_str().unwrap();
    let mnt_dir = json["mnt_dir"].as_str().unwrap();

    assert!(
        run_dir.starts_with('/'),
        "run_dir should be absolute: {}",
        run_dir
    );
    assert!(
        in_dir.starts_with('/'),
        "in_dir should be absolute: {}",
        in_dir
    );
    assert!(
        out_dir.starts_with('/'),
        "out_dir should be absolute: {}",
        out_dir
    );
    assert!(
        mnt_dir.starts_with('/'),
        "mnt_dir should be absolute: {}",
        mnt_dir
    );
}

// =============================================================================
// Integration with FARM_ env vars
// =============================================================================

#[test]
fn test_ctx_run_respects_farm_ctx_env() {
    let ws = TestWorkspace::new();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_farm"));
    cmd.current_dir(ws.path())
        .env("FARM_CTX", "production")
        .args(["ctx", "run", "init", "--build-id", "env-test", "--format", "json"]);

    let output = cmd.output().expect("Failed to execute farm");
    assert!(output.status.success());

    let json: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&output.stdout)).unwrap();

    assert_eq!(
        json["ctx_name"], "production",
        "should use FARM_CTX from environment"
    );
}

#[test]
fn test_ctx_run_cli_overrides_env() {
    let ws = TestWorkspace::new();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_farm"));
    cmd.current_dir(ws.path())
        .env("FARM_CTX", "from-env")
        .args([
            "ctx", "run", "init",
            "--build-id", "override-test",
            "--ctx", "from-cli",
            "--format", "json",
        ]);

    let output = cmd.output().expect("Failed to execute farm");
    assert!(output.status.success());

    let json: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&output.stdout)).unwrap();

    assert_eq!(
        json["ctx_name"], "from-cli",
        "CLI --ctx should override FARM_CTX env"
    );
}
