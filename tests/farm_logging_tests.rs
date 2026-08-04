//! SPDX-License-Identifier: MIT OR Apache-2.0

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

/// Log directory of the most recent run of `goal`.
///
/// Run directories are per-invocation since ADR 0001, so their names are
/// generated and tests resolve them through the manifest instead of assuming
/// `.farm/run/{goal}/`.
fn latest_log_dir(workspace: &Path, goal: &str) -> Option<PathBuf> {
    farm::runs::find_runs(&workspace.join(".farm"), goal, None)
        .first()
        .map(|run| run.log_dir())
}

/// Clear leaked `FARM_*` env vars once per test process. The
/// spawned `farm` subprocess inherits the developer's environment;
/// `FARM_WORKSPACE` from an external launcher would otherwise redirect
/// it away from our `TempDir`, so the run would be recorded in the wrong
/// workspace and `latest_log_dir` would find nothing. See
/// `farm/tests/context_integration_tests.rs::isolate_test_env`
/// for the full rationale.
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

#[test]
fn test_log_output_file_mode() {
    isolate_test_env();
    let temp_dir = TempDir::new().unwrap();
    let workspace = temp_dir.path();
    
        // Create a simple Farmfile
    let farmfile_content = r#"
version: 1

[variant]
debug

[operation.test]
work: ```
echo "Default mode test"
```
"#;
    
    fs::write(workspace.join("Farmfile"), farmfile_content).unwrap();
    
    // Run farm with --log-output file (default)
    let output = Command::new(env!("CARGO_BIN_EXE_farm"))
        .current_dir(workspace)
        .args(["run", "test", "--variant", "debug", "--log-output", "file"])
        .output()
        .expect("Failed to execute farm");
    
    // Should succeed
    assert!(output.status.success(), "Farm execution failed: {}", String::from_utf8_lossy(&output.stderr));
    
    // Check that log files were created
    let log_dir = latest_log_dir(workspace, "test").expect("run should be recorded");
    assert!(log_dir.exists(), "Log directory should exist");
    
    // Default file mode now creates combined log (not split stdout/stderr)
    let combined_log = log_dir.join("test_debug.log");
    
    assert!(combined_log.exists(), "Combined log file should exist");
    
    // Check log content contains stage information
    let log_content = fs::read_to_string(&combined_log).unwrap();
    // Log format uses key=value frames, not the old `=== Stage: ... ===` banner.
    assert!(
        log_content.contains("stage=test") && log_content.contains("variant=debug"),
        "Log should contain stage header: {log_content}"
    );
    assert!(log_content.contains("Default mode test"), "Log should contain task output");
}

#[test]
fn test_log_output_file_split_mode() {
    isolate_test_env();
    let temp_dir = TempDir::new().unwrap();
    let workspace = temp_dir.path();
    
    // Create a simple Farmfile
    let farmfile_content = r#"
version: 1

[variant]
debug

[operation.test]
work: ```
echo "Split mode test"
```
"#;
    
    fs::write(workspace.join("Farmfile"), farmfile_content).unwrap();
    
    // Run farm with --log-output file-split
    let output = Command::new(env!("CARGO_BIN_EXE_farm"))
        .current_dir(workspace)
        .args(["run", "test", "--variant", "debug", "--log-output", "file-split"])
        .output()
        .expect("Failed to execute farm");
    
    // Should succeed
    assert!(output.status.success(), "Farm execution failed: {}", String::from_utf8_lossy(&output.stderr));
    
    // Check that separate log files were created
    let log_dir = latest_log_dir(workspace, "test").expect("run should be recorded");
    assert!(log_dir.exists(), "Log directory should exist");
    
    let stdout_log = log_dir.join("test_debug_stdout.log");
    let stderr_log = log_dir.join("test_debug_stderr.log");
    
    assert!(stdout_log.exists(), "Stdout log file should exist in split mode");
    assert!(stderr_log.exists(), "Stderr log file should exist in split mode");
    
    // Check log content contains stage information
    let log_content = fs::read_to_string(&stdout_log).unwrap();
    // Log format uses key=value frames, not the old `=== Stage: ... ===` banner.
    assert!(
        log_content.contains("stage=test") && log_content.contains("variant=debug"),
        "Log should contain stage header: {log_content}"
    );
    assert!(log_content.contains("Split mode test"), "Log should contain task output");
}

#[test]
fn test_log_output_stdout_mode() {
    isolate_test_env();
    let temp_dir = TempDir::new().unwrap();
    let workspace = temp_dir.path();
    
    // Create a simple Farmfile
    let farmfile_content = r#"
version: 1

[variant]
debug

[operation.test]
work: ```
echo "Hello from stdout test"
```
"#;
    
    fs::write(workspace.join("Farmfile"), farmfile_content).unwrap();
    
    // Run farm with --log-output stdout
    let output = Command::new(env!("CARGO_BIN_EXE_farm"))
        .current_dir(workspace)
        .args(["run", "test", "--variant", "debug", "--log-output", "stdout"])
        .output()
        .expect("Failed to execute farm");
    
    // Should succeed
    assert!(output.status.success(), "Farm execution failed: {}", String::from_utf8_lossy(&output.stderr));
    
    // Check that NO log files were created
    let log_dir = latest_log_dir(workspace, "test").unwrap_or_default();
    if log_dir.exists() {
        let stdout_log = log_dir.join("test_debug_stdout.log");

        // If files exist, they should be empty or minimal (just directory structure)
        if stdout_log.exists() {
            let log_content = fs::read_to_string(&stdout_log).unwrap();
            assert!(log_content.is_empty() || log_content.trim().is_empty(), 
                   "Stdout log should be empty in stdout mode, but contained: {}", log_content);
        }
    }
    
    // The output should contain the task output (since it goes to stdout)
    let stdout_str = String::from_utf8_lossy(&output.stdout);
    assert!(stdout_str.contains("Hello from stdout test"), "Task output should go to stdout");
}

#[test]
fn test_log_output_none_mode() {
    isolate_test_env();
    let temp_dir = TempDir::new().unwrap();
    let workspace = temp_dir.path();
    
    // Create a simple Farmfile
    let farmfile_content = r#"
version: 1

[variant]
debug

[operation.test]
work: ```
echo "Hello from none test"
```
"#;
    
    fs::write(workspace.join("Farmfile"), farmfile_content).unwrap();
    
    // Run farm with --log-output none
    let output = Command::new(env!("CARGO_BIN_EXE_farm"))
        .current_dir(workspace)
        .args(["run", "test", "--variant", "debug", "--log-output", "none"])
        .output()
        .expect("Failed to execute farm");
    
    // Should succeed
    assert!(output.status.success(), "Farm execution failed: {}", String::from_utf8_lossy(&output.stderr));
    
    // Check that NO meaningful log files were created
    let log_dir = latest_log_dir(workspace, "test").unwrap_or_default();
    if log_dir.exists() {
        let stdout_log = log_dir.join("test_debug_stdout.log");
        if stdout_log.exists() {
            let log_content = fs::read_to_string(&stdout_log).unwrap();
            assert!(log_content.is_empty() || log_content.trim().is_empty(), 
                   "Stdout log should be empty in none mode");
        }
    }
}

#[test]
fn test_invalid_log_output_mode() {
    isolate_test_env();
    let temp_dir = TempDir::new().unwrap();
    let workspace = temp_dir.path();
    
    // Create a simple Farmfile
    let farmfile_content = r#"
version: 1

[variant]
debug

[operation.test]
work: ```
echo "Hello"
```
"#;
    
    fs::write(workspace.join("Farmfile"), farmfile_content).unwrap();
    
    // Run farm with invalid --log-output
    let output = Command::new(env!("CARGO_BIN_EXE_farm"))
        .current_dir(workspace)
        .args(["run", "test", "--variant", "debug", "--log-output", "invalid"])
        .output()
        .expect("Failed to execute farm");
    
    // Should fail
    assert!(!output.status.success(), "Farm should fail with invalid log-output");
    
    let stderr_str = String::from_utf8_lossy(&output.stderr);
    assert!(stderr_str.contains("Invalid --log-output value"), "Should show invalid log-output error");
}

#[test]
fn test_default_log_output_is_file() {
    isolate_test_env();
    let temp_dir = TempDir::new().unwrap();
    let workspace = temp_dir.path();
    
    // Create a simple Farmfile
    let farmfile_content = r#"
version: 1

[variant]
debug

[operation.test]
work: ```
echo "Default mode test"
```
"#;
    
    fs::write(workspace.join("Farmfile"), farmfile_content).unwrap();
    
    // Run farm without --log-output (should default to file)
    let output = Command::new(env!("CARGO_BIN_EXE_farm"))
        .current_dir(workspace)
        .args(["run", "test", "--variant", "debug"])
        .output()
        .expect("Failed to execute farm");
    
    // Should succeed
    assert!(output.status.success(), "Farm execution failed: {}", String::from_utf8_lossy(&output.stderr));
    
    // Check that log files were created (default behavior is combined log)
    let log_dir = latest_log_dir(workspace, "test").expect("run should be recorded");
    assert!(log_dir.exists(), "Log directory should exist");
    
    let combined_log = log_dir.join("test_debug.log");
    assert!(combined_log.exists(), "Combined log file should exist by default");
    
    // Check log content contains stage information
    let log_content = fs::read_to_string(&combined_log).unwrap();
    // Log format uses key=value frames, not the old `=== Stage: ... ===` banner.
    assert!(
        log_content.contains("stage=test") && log_content.contains("variant=debug"),
        "Log should contain stage header: {log_content}"
    );
}