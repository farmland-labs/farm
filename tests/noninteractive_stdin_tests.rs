//! SPDX-License-Identifier: MIT OR Apache-2.0

//! Operations run non-interactively: the child's stdin is `/dev/null`, so a
//! process that reads input gets EOF immediately (it fails fast instead of
//! deadlocking on an invisible prompt), and farm's own stdin is never
//! forwarded into the build.

use std::fs;
use std::io::Write;
use std::process::{Command, Stdio};
use tempfile::TempDir;

/// Clear leaked `FARM_*` env vars so the spawned `farm` operates against our
/// `TempDir` and the run directory is keyed on the goal name.
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
fn test_parent_stdin_is_not_forwarded_to_child() {
    isolate_test_env();
    let temp = TempDir::new().unwrap();
    let ws = temp.path();

    // `cat` echoes its stdin to stdout. With a null child stdin it reads EOF
    // and prints nothing; if farm had inherited the parent's stdin, the
    // sentinel below would show up in the operation's output (the log).
    let farmfile = "version: 1\n\n[operation.echo_in]\nwork: cat\n";
    fs::write(ws.join("Farmfile"), farmfile).unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_farm"))
        .current_dir(ws)
        .args(["run", "echo_in", "--no-cache"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn farm");

    // Feed farm's *own* stdin some data, then close it. farm must not relay
    // this to the build's `cat`.
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"SHOULD_NOT_APPEAR\n")
        .unwrap();

    let output = child.wait_with_output().expect("farm should finish (not hang)");
    assert!(
        output.status.success(),
        "farm run should succeed without hanging. stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let log = fs::read_to_string(ws.join(".farm/run/echo_in/log/echo_in_default.log")).unwrap();
    assert!(
        !log.contains("SHOULD_NOT_APPEAR"),
        "parent stdin must not be forwarded into the child:\n{log}"
    );
}

#[test]
fn test_stdin_reader_fails_fast_instead_of_hanging() {
    isolate_test_env();
    let temp = TempDir::new().unwrap();
    let ws = temp.path();

    // `read` blocks for a line on a TTY; with a null stdin it gets EOF and
    // returns non-zero immediately. The operation fails fast — it must not
    // hang waiting for input.
    let farmfile = "version: 1\n\n[operation.prompt]\nwork: read answer\n";
    fs::write(ws.join("Farmfile"), farmfile).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_farm"))
        .current_dir(ws)
        .args(["run", "prompt", "--no-cache"])
        .output()
        .expect("farm should finish (not hang)");

    // The read hits EOF -> non-zero -> the operation (and the run) fail fast.
    assert!(
        !output.status.success(),
        "an input-reading op should fail fast on EOF, not succeed/hang"
    );
}
