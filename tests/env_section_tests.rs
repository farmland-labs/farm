//! SPDX-License-Identifier: MIT OR Apache-2.0

//! Integration tests for the Farmfile `[env]` section: environment variables
//! declared there must be injected into every operation's command.

use std::fs;
use std::process::Command;
use tempfile::TempDir;

/// Clear leaked `FARM_*` env vars that an external launcher may have set, so
/// the spawned `farm` subprocess operates against our `TempDir`.
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
fn test_env_section_value_reaches_operation() {
    isolate_test_env();
    let temp_dir = TempDir::new().unwrap();
    let workspace = temp_dir.path();

    let farmfile = r#"version: 1

[env]
GREETING=hello_from_farm_env

[operation.greet]
work: echo "msg=$GREETING"
"#;
    fs::write(workspace.join("Farmfile"), farmfile).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_farm"))
        .current_dir(workspace)
        .args(["run", "greet", "--no-cache"])
        .output()
        .expect("Failed to execute farm");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "farm run should succeed. stdout={stdout}\nstderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains("msg=hello_from_farm_env"),
        "operation should see the [env] value. stdout={stdout}"
    );
}

#[test]
fn test_env_section_overrides_ambient_shell() {
    isolate_test_env();
    let temp_dir = TempDir::new().unwrap();
    let workspace = temp_dir.path();

    let farmfile = r#"version: 1

[env]
GREETING=from_farmfile

[operation.greet]
work: echo "msg=$GREETING"
"#;
    fs::write(workspace.join("Farmfile"), farmfile).unwrap();

    // The caller's shell sets GREETING to something else; the Farmfile must win.
    let output = Command::new(env!("CARGO_BIN_EXE_farm"))
        .current_dir(workspace)
        .env("GREETING", "from_shell")
        .args(["run", "greet", "--no-cache"])
        .output()
        .expect("Failed to execute farm");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "farm run should succeed. stdout={stdout}");
    assert!(
        stdout.contains("msg=from_farmfile"),
        "[env] should override the ambient shell value. stdout={stdout}"
    );
    assert!(
        !stdout.contains("msg=from_shell"),
        "ambient shell value should not win. stdout={stdout}"
    );
}

#[test]
fn test_env_section_does_not_override_reserved_farm_vars() {
    isolate_test_env();
    let temp_dir = TempDir::new().unwrap();
    let workspace = temp_dir.path();

    // A user trying to set FARM_TARGET via [env] must not clobber the
    // reserved value farm injects for the running operation.
    let farmfile = r#"version: 1

[env]
FARM_TARGET=hacked

[operation.show]
work: echo "target=$FARM_TARGET"
"#;
    fs::write(workspace.join("Farmfile"), farmfile).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_farm"))
        .current_dir(workspace)
        .args(["run", "show", "--no-cache"])
        .output()
        .expect("Failed to execute farm");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "farm run should succeed. stdout={stdout}");
    assert!(
        stdout.contains("target=show"),
        "reserved FARM_TARGET must reflect the running operation, not [env]. stdout={stdout}"
    );
}
