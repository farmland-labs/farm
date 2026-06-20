//! SPDX-License-Identifier: MIT OR Apache-2.0

//! Integration tests: command output is streamed line-by-line into the build
//! log file (merged combined log, and separated streams in split mode).

use std::fs;
use std::process::Command;
use tempfile::TempDir;

/// Clear leaked `FARM_*` env vars that an external launcher may have set, so
/// the spawned `farm` subprocess operates against our `TempDir` and the run
/// directory is keyed on the goal name (`greet`) as expected.
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
fn test_output_streams_into_combined_log_in_order() {
    isolate_test_env();
    let temp = TempDir::new().unwrap();
    let ws = temp.path();

    let farmfile = r#"version: 1

[operation.greet]
work: printf 'line1\nline2\nline3\n'
"#;
    fs::write(ws.join("Farmfile"), farmfile).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_farm"))
        .current_dir(ws)
        .args(["run", "greet", "--no-cache"])
        .output()
        .expect("run farm");
    assert!(
        output.status.success(),
        "farm run should succeed. stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Default build_id is the goal name; combined log is {goal}_{variant}.log.
    let log = fs::read_to_string(ws.join(".farm/run/greet/log/greet_default.log"))
        .expect("combined log file should exist");

    for line in ["line1", "line2", "line3"] {
        assert!(log.contains(line), "log missing {line}:\n{log}");
    }
    let (p1, p2, p3) = (
        log.find("line1").unwrap(),
        log.find("line2").unwrap(),
        log.find("line3").unwrap(),
    );
    assert!(p1 < p2 && p2 < p3, "streamed lines out of order:\n{log}");

    // Streamed merged output carries no per-stream framing, but keeps the
    // surrounding [log] header/footer.
    assert!(!log.contains("[stdout]"), "merged log must not be framed:\n{log}");
    assert!(log.contains("[log] stage=greet"), "missing stage header:\n{log}");
    assert!(log.contains("exit_code=0 success=true"), "missing footer:\n{log}");
}

#[test]
fn test_split_mode_separates_streams_into_files() {
    isolate_test_env();
    let temp = TempDir::new().unwrap();
    let ws = temp.path();

    let farmfile = r#"version: 1

[operation.greet]
work: echo OUTLINE; echo ERRLINE 1>&2
"#;
    fs::write(ws.join("Farmfile"), farmfile).unwrap();

    // file-split picks the two-file layout; split-streams keeps stdout/stderr
    // on separate capture threads so each lands in its own file.
    let output = Command::new(env!("CARGO_BIN_EXE_farm"))
        .current_dir(ws)
        .args([
            "run",
            "greet",
            "--no-cache",
            "--log-output",
            "file-split",
            "--split-streams",
        ])
        .output()
        .expect("run farm");
    assert!(
        output.status.success(),
        "farm run should succeed. stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout_log = fs::read_to_string(ws.join(".farm/run/greet/log/greet_default_stdout.log"))
        .expect("stdout log should exist");
    let stderr_log = fs::read_to_string(ws.join(".farm/run/greet/log/greet_default_stderr.log"))
        .expect("stderr log should exist");

    // The cmd header (written to both files) echoes the full command, so both
    // markers appear once there. Correct separation means the *output* line
    // adds exactly one more occurrence to its own file: OUTLINE → 2 in stdout
    // (header + output), 1 in stderr (header only); ERRLINE the mirror.
    assert_eq!(stdout_log.matches("OUTLINE").count(), 2, "OUTLINE should appear in stdout header+output:\n{stdout_log}");
    assert_eq!(stdout_log.matches("ERRLINE").count(), 1, "ERRLINE must not stream into the stdout file:\n{stdout_log}");
    assert_eq!(stderr_log.matches("ERRLINE").count(), 2, "ERRLINE should appear in stderr header+output:\n{stderr_log}");
    assert_eq!(stderr_log.matches("OUTLINE").count(), 1, "OUTLINE must not stream into the stderr file:\n{stderr_log}");
    assert!(!stdout_log.contains("[stdout]"), "no framing markers in streamed files:\n{stdout_log}");
}
