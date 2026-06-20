//! SPDX-License-Identifier: MIT OR Apache-2.0

//! Integration tests for the `farm version` command
//!
//! Simple sanity test to ensure version output works.

use std::process::Command;

#[test]
fn test_version_command_succeeds() {
    // Note: `farm version` only outputs to terminal (TTY check)
    // When not a TTY, it just exits successfully
    let output = Command::new(env!("CARGO_BIN_EXE_farm"))
        .arg("version")
        .output()
        .expect("Failed to execute farm");

    assert!(output.status.success(), "version command should succeed");
}

#[test]
fn test_version_flag() {
    // --version is handled by clap and always outputs
    let output = Command::new(env!("CARGO_BIN_EXE_farm"))
        .arg("--version")
        .output()
        .expect("Failed to execute farm");

    assert!(output.status.success(), "--version should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("farm"),
        "should mention 'farm': {}",
        stdout
    );
}
