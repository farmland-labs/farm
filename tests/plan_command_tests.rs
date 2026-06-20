//! SPDX-License-Identifier: MIT OR Apache-2.0

//! Integration tests for the `farm plan` command
//!
//! Tests the CLI interface that users rely on for tooling integration.
//! Focus: format outputs (especially --format json for scripting)

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
        Self { dir }
    }

    fn write_farmfile(&self, content: &str) {
        fs::write(self.dir.path().join("Farmfile"), content)
            .expect("Failed to write Farmfile");
    }

    /// Run farm plan command
    fn plan(&self, args: &[&str]) -> FarmResult {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_farm"));
        cmd.current_dir(self.dir.path()).arg("plan");

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
// Basic Plan Tests
// =============================================================================

#[test]
fn test_plan_basic_output() {
    let ws = TestWorkspace::new();

    ws.write_farmfile(
        r#"
version: 1

[operation.build]
work: echo "building"
"#,
    );

    let result = ws.plan(&[]);

    assert!(result.success, "plan command should succeed: {}", result.stderr);
    assert!(
        result.stdout.contains("Farmland Plan"),
        "should show plan header"
    );
    assert!(
        result.stdout.contains("build"),
        "should list build operation"
    );
}

#[test]
fn test_plan_with_dependencies() {
    let ws = TestWorkspace::new();

    ws.write_farmfile(
        r#"
version: 1

[operation.prepare]
work: echo "preparing"

[operation.build]
after: prepare
work: echo "building"

[operation.test]
after: build
work: echo "testing"
"#,
    );

    let result = ws.plan(&[]);

    assert!(result.success);
    assert!(result.stdout.contains("prepare"));
    assert!(result.stdout.contains("build"));
    assert!(result.stdout.contains("test"));
}

// =============================================================================
// Format Tests
// =============================================================================

#[test]
fn test_plan_format_tree() {
    let ws = TestWorkspace::new();

    ws.write_farmfile(
        r#"
version: 1

[operation.build]
work: echo "building"
"#,
    );

    let result = ws.plan(&["--format", "tree"]);

    assert!(result.success);
    assert!(result.stdout.contains("Farmland Plan"));
}

#[test]
fn test_plan_format_list() {
    let ws = TestWorkspace::new();

    ws.write_farmfile(
        r#"
version: 1

[operation.prepare]
work: echo "preparing"

[operation.build]
after: prepare
work: echo "building"
"#,
    );

    let result = ws.plan(&["--format", "list"]);

    assert!(result.success);
    // List format shows numbered operations with dependencies
    assert!(
        result.stdout.contains("depends:"),
        "list format should show dependencies"
    );
}

#[test]
fn test_plan_format_dot() {
    let ws = TestWorkspace::new();

    ws.write_farmfile(
        r#"
version: 1

[operation.prepare]
work: echo "preparing"

[operation.build]
after: prepare
work: echo "building"
"#,
    );

    let result = ws.plan(&["--format", "dot"]);

    assert!(result.success);
    // DOT format is graphviz
    assert!(
        result.stdout.contains("digraph"),
        "dot format should output graphviz"
    );
}

#[test]
fn test_plan_format_json() {
    let ws = TestWorkspace::new();

    ws.write_farmfile(
        r#"
version: 1

[variant]
debug
release

[operation.build]
work: echo "building"

[operation.test]
after: build
work: echo "testing"
"#,
    );

    let result = ws.plan(&["--format", "json"]);

    assert!(result.success, "json format should succeed: {}", result.stderr);

    // Parse the JSON to verify structure
    let json: serde_json::Value =
        serde_json::from_str(&result.stdout).expect("should output valid JSON");

    // Verify expected fields exist
    assert!(json.get("operations").is_some(), "should have operations");
    assert!(json.get("variants").is_some(), "should have variants");

    // Verify variants are present
    let variants = json["variants"].as_array().expect("variants should be array");
    assert!(
        variants.iter().any(|v| v == "debug"),
        "should have debug variant"
    );
    assert!(
        variants.iter().any(|v| v == "release"),
        "should have release variant"
    );

    // Verify operations structure
    let ops = json["operations"].as_array().expect("operations should be array");
    assert_eq!(ops.len(), 2, "should have 2 operations");
}

#[test]
fn test_plan_format_structure() {
    let ws = TestWorkspace::new();

    ws.write_farmfile(
        r#"
version: 1

[variant]
debug

[goal]
all: test

[operation.build]
work: echo "building"

[operation.test]
after: build
work: echo "testing"
"#,
    );

    let result = ws.plan(&["--format", "structure"]);

    assert!(
        result.success,
        "structure format should succeed: {}",
        result.stderr
    );

    // Parse the JSON to verify structure
    let json: serde_json::Value =
        serde_json::from_str(&result.stdout).expect("should output valid JSON");

    // Structure format is designed for tooling - safe output without secrets
    assert!(json.get("operations").is_some(), "should have operations");
    assert!(json.get("variants").is_some(), "should have variants");

    // Operations should have label, depends, task_count, goal
    let ops = json["operations"].as_array().expect("operations should be array");
    for op in ops {
        assert!(op.get("label").is_some(), "operation should have label");
        assert!(op.get("depends").is_some(), "operation should have depends");
        assert!(
            op.get("task_count").is_some(),
            "operation should have task_count"
        );
        // goal may be null
    }

    // Verify the goal is properly mapped
    let test_op = ops.iter().find(|op| op["label"] == "test").unwrap();
    assert_eq!(
        test_op["goal"], "all",
        "test operation should have goal 'all'"
    );
}

#[test]
fn test_plan_format_invalid() {
    let ws = TestWorkspace::new();

    ws.write_farmfile(
        r#"
version: 1

[operation.build]
work: echo "building"
"#,
    );

    let result = ws.plan(&["--format", "invalid"]);

    assert!(!result.success, "invalid format should fail");
    assert!(
        result.stderr.contains("Unknown format") || result.stdout.contains("Unknown format"),
        "should mention unknown format"
    );
}

// =============================================================================
// Verbose Flag Tests
// =============================================================================

#[test]
fn test_plan_verbose_flag() {
    let ws = TestWorkspace::new();

    ws.write_farmfile(
        r#"
version: 1

[operation.build]
input: src/**/*.rs
output: target/release/app
work: echo "building"
"#,
    );

    let normal = ws.plan(&[]);
    let verbose = ws.plan(&["--verbose"]);

    assert!(normal.success);
    assert!(verbose.success);

    // Verbose output should be longer or contain more detail
    // At minimum, both should work
}

// =============================================================================
// Error Cases
// =============================================================================

#[test]
fn test_plan_missing_farmfile() {
    let ws = TestWorkspace::new();
    // Don't write a Farmfile

    let result = ws.plan(&[]);

    assert!(!result.success, "should fail without Farmfile");
}

#[test]
fn test_plan_invalid_farmfile() {
    let ws = TestWorkspace::new();

    ws.write_farmfile("this is not valid syntax {{{}}}");

    let result = ws.plan(&[]);

    assert!(!result.success, "should fail with invalid Farmfile");
}

#[test]
fn test_plan_custom_farmfile_path() {
    let ws = TestWorkspace::new();

    // Write to custom path
    fs::write(
        ws.dir.path().join("custom.farm"),
        r#"
version: 1

[operation.build]
work: echo "building"
"#,
    )
    .unwrap();

    let result = ws.plan(&["--farmfile", "custom.farm"]);

    assert!(
        result.success,
        "should work with custom Farmfile: {}",
        result.stderr
    );
    assert!(result.stdout.contains("build"));
}

// =============================================================================
// JSON Format Stability Tests (for tooling)
// =============================================================================

#[test]
fn test_plan_json_has_stable_fields() {
    let ws = TestWorkspace::new();

    ws.write_farmfile(
        r#"
version: 1

[operation.build]
input: src/**/*.rs
output: target/app
work: echo "building"
"#,
    );

    let result = ws.plan(&["--format", "json"]);
    assert!(result.success);

    let json: serde_json::Value = serde_json::from_str(&result.stdout).unwrap();

    // These fields MUST exist for tooling stability
    assert!(json.get("operations").is_some(), "must have 'operations'");
    assert!(json.get("variants").is_some(), "must have 'variants'");

    // Operation structure
    let ops = json["operations"].as_array().unwrap();
    let build = &ops[0];
    assert!(build.get("label").is_some(), "operation must have 'label'");
    assert!(build.get("depends").is_some(), "operation must have 'depends'");
    assert!(build.get("tasks").is_some(), "operation must have 'tasks'");
}

#[test]
fn test_plan_structure_omits_task_details() {
    let ws = TestWorkspace::new();

    ws.write_farmfile(
        r#"
version: 1

[operation.build]
work: ```
SECRET_KEY=supersecret
echo $SECRET_KEY
```
"#,
    );

    let result = ws.plan(&["--format", "structure"]);
    assert!(result.success);

    // Structure format should NOT contain the secret
    assert!(
        !result.stdout.contains("supersecret"),
        "structure format should not expose task content"
    );
    assert!(
        !result.stdout.contains("SECRET_KEY"),
        "structure format should not expose env vars"
    );

    // But task_count should be there
    let json: serde_json::Value = serde_json::from_str(&result.stdout).unwrap();
    let ops = json["operations"].as_array().unwrap();
    assert_eq!(ops[0]["task_count"], 1, "should show task count");
}
