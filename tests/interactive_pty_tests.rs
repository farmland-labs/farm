//! SPDX-License-Identifier: MIT OR Apache-2.0

//! Interactive execution (ADR-081): when `farm` is attached to a controlling
//! TTY and `--non-interactive` is not passed, operations run through a PTY so
//! the child sees a real terminal, farm relays keystrokes into it, and a clean
//! (ANSI-stripped) copy of the output is still tee'd to the build log.
//!
//! These tests drive `farm` itself behind a PTY so its stdin/stdout are ttys —
//! the same condition a developer's terminal provides — and are Unix-only
//! (the Windows ConPTY path is validated separately, per ADR-081).

#![cfg(unix)]

use std::fs;
use std::io::{Read, Write};
use std::time::{Duration, Instant};

use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use tempfile::TempDir;

/// Clear leaked `FARM_*` env vars so the spawned `farm` operates against our
/// `TempDir` and the run directory is keyed on the goal name. The child
/// `CommandBuilder` inherits this process's environment at spawn.
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

/// Read from `reader` until EOF (child exit closes the PTY), collecting all
/// bytes. Runs on its own thread so the child never blocks on a full PTY.
fn drain(mut reader: Box<dyn Read + Send>) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut out = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => out.extend_from_slice(&buf[..n]),
                Err(_) => break,
            }
        }
        out
    })
}

/// With a controlling TTY, an operation that reads stdin receives the developer's
/// input — proving the PTY relay works — and the input round-trips into the
/// tee'd log. `cat` echoes its stdin until EOF (Ctrl-D).
#[test]
fn interactive_run_forwards_input_to_child_and_logs_it() {
    isolate_test_env();
    let temp = TempDir::new().unwrap();
    let ws = temp.path();

    let farmfile = "version: 1\n\n[operation.echo_in]\nwork: cat\n";
    fs::write(ws.join("Farmfile"), farmfile).unwrap();

    let pair = native_pty_system()
        .openpty(PtySize { rows: 24, cols: 80, pixel_width: 0, pixel_height: 0 })
        .expect("openpty");

    let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_farm"));
    cmd.cwd(ws);
    cmd.args(["run", "echo_in", "--no-cache"]);

    let mut child = pair.slave.spawn_command(cmd).expect("spawn farm in pty");
    drop(pair.slave);

    let reader = pair.master.try_clone_reader().expect("reader");
    let mut writer = pair.master.take_writer().expect("writer");
    let drainer = drain(reader);

    // Send a line of "keystrokes", then Ctrl-D (0x04) so `cat` sees EOF and
    // exits. The inner PTY buffers this until `cat` reads it.
    writer.write_all(b"hello-from-tty\n").unwrap();
    writer.write_all(&[0x04]).unwrap();
    writer.flush().unwrap();

    // Bound the wait so a regression hangs the test (with a message) instead of
    // wedging CI forever.
    let deadline = Instant::now() + Duration::from_secs(20);
    let status = loop {
        if let Some(s) = child.try_wait().expect("try_wait") {
            break s;
        }
        assert!(Instant::now() < deadline, "farm did not exit — interactive run hung");
        std::thread::sleep(Duration::from_millis(50));
    };
    let console = String::from_utf8_lossy(&drainer.join().unwrap()).to_string();

    assert!(status.success(), "interactive farm run should succeed. console:\n{console}");
    assert!(
        console.contains("hello-from-tty"),
        "child should echo our keystrokes back to the terminal. console:\n{console}"
    );

    let log = fs::read_to_string(ws.join(".farm/run/echo_in/log/echo_in_default.log")).unwrap();
    assert!(
        log.contains("hello-from-tty"),
        "the developer's input must round-trip into the tee'd log:\n{log}"
    );
}

/// Ctrl-C at an interactive prompt interrupts the *task*, not farm. farm keeps
/// its own terminal in raw mode (ISIG off), so the `^C` byte is not turned into
/// a signal for farm — it is relayed into the child's PTY, whose line discipline
/// (ISIG on) raises SIGINT in the child. The task dies, the operation fails, and
/// farm returns control cleanly instead of running the op to completion.
#[test]
fn ctrl_c_interrupts_the_running_task_not_farm() {
    isolate_test_env();
    let temp = TempDir::new().unwrap();
    let ws = temp.path();

    // Without interruption this op would pin the run for 30s.
    let farmfile = "version: 1\n\n[operation.nap]\nwork: sleep 30\n";
    fs::write(ws.join("Farmfile"), farmfile).unwrap();

    let pair = native_pty_system()
        .openpty(PtySize { rows: 24, cols: 80, pixel_width: 0, pixel_height: 0 })
        .expect("openpty");

    let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_farm"));
    cmd.cwd(ws);
    cmd.args(["run", "nap", "--no-cache"]);

    let mut child = pair.slave.spawn_command(cmd).expect("spawn farm in pty");
    drop(pair.slave);

    let reader = pair.master.try_clone_reader().expect("reader");
    let mut writer = pair.master.take_writer().expect("writer");
    let drainer = drain(reader);

    // Let the child reach `sleep`, then press Ctrl-C (0x03).
    std::thread::sleep(Duration::from_millis(700));
    let start = Instant::now();
    writer.write_all(&[0x03]).unwrap();
    writer.flush().unwrap();

    let deadline = start + Duration::from_secs(15);
    let status = loop {
        if let Some(s) = child.try_wait().expect("try_wait") {
            break s;
        }
        assert!(
            Instant::now() < deadline,
            "Ctrl-C did not interrupt the task — farm ran the sleep to completion"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    let elapsed = start.elapsed();
    let _ = drainer.join();

    assert!(
        elapsed < Duration::from_secs(10),
        "Ctrl-C should interrupt promptly, but the run took {elapsed:?} after ^C"
    );
    assert!(
        !status.success(),
        "an interrupted operation must fail the run, not report success"
    );
}

/// A task that traps SIGINT survives the first Ctrl-C; a second consecutive
/// Ctrl-C escalates to a farm-issued force-kill (SIGKILL of the child's process
/// group). This is the ADR-081 "press Ctrl-C again to force-kill" escape hatch.
#[test]
fn double_ctrl_c_force_kills_a_signal_trapping_task() {
    isolate_test_env();
    let temp = TempDir::new().unwrap();
    let ws = temp.path();

    // Ignore INT/TERM and loop forever: only SIGKILL (the group force-kill) can
    // stop this. Run via a script file to avoid Farmfile quoting concerns.
    let script = ws.join("trapper.sh");
    fs::write(&script, "#!/bin/sh\ntrap '' INT TERM\nwhile true; do sleep 1; done\n").unwrap();
    let farmfile = format!("version: 1\n\n[operation.trapper]\nwork: sh {}\n", script.display());
    fs::write(ws.join("Farmfile"), farmfile).unwrap();

    let pair = native_pty_system()
        .openpty(PtySize { rows: 24, cols: 80, pixel_width: 0, pixel_height: 0 })
        .expect("openpty");

    let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_farm"));
    cmd.cwd(ws);
    cmd.args(["run", "trapper", "--no-cache"]);

    let mut child = pair.slave.spawn_command(cmd).expect("spawn farm in pty");
    drop(pair.slave);

    let reader = pair.master.try_clone_reader().expect("reader");
    let mut writer = pair.master.take_writer().expect("writer");
    let drainer = drain(reader);

    // First Ctrl-C: absorbed by the trapping task.
    std::thread::sleep(Duration::from_millis(800));
    writer.write_all(&[0x03]).unwrap();
    writer.flush().unwrap();

    // The run must NOT end on the first Ctrl-C — proving escalation, not the
    // first press, is what kills it.
    std::thread::sleep(Duration::from_millis(1200));
    assert!(
        child.try_wait().expect("try_wait").is_none(),
        "a SIGINT-trapping task should survive the first Ctrl-C"
    );

    // Second consecutive Ctrl-C: farm force-kills.
    let start = Instant::now();
    writer.write_all(&[0x03]).unwrap();
    writer.flush().unwrap();

    let deadline = start + Duration::from_secs(15);
    let status = loop {
        if let Some(s) = child.try_wait().expect("try_wait") {
            break s;
        }
        assert!(Instant::now() < deadline, "second Ctrl-C did not force-kill the task");
        std::thread::sleep(Duration::from_millis(50));
    };
    let elapsed = start.elapsed();
    let _ = drainer.join();

    assert!(
        elapsed < Duration::from_secs(10),
        "force-kill should be prompt, took {elapsed:?} after the second ^C"
    );
    assert!(!status.success(), "a force-killed run must fail");
}

/// `--non-interactive` forces the closed-stdin path even behind a real TTY:
/// `cat` reads EOF immediately and echoes nothing, so our sentinel never
/// reaches the child or the log. This is the behaviour a tool/buddy opts into.
#[test]
fn non_interactive_flag_closes_stdin_even_on_a_tty() {
    isolate_test_env();
    let temp = TempDir::new().unwrap();
    let ws = temp.path();

    let farmfile = "version: 1\n\n[operation.echo_in]\nwork: cat\n";
    fs::write(ws.join("Farmfile"), farmfile).unwrap();

    let pair = native_pty_system()
        .openpty(PtySize { rows: 24, cols: 80, pixel_width: 0, pixel_height: 0 })
        .expect("openpty");

    let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_farm"));
    cmd.cwd(ws);
    cmd.args(["run", "echo_in", "--no-cache", "--non-interactive"]);

    let mut child = pair.slave.spawn_command(cmd).expect("spawn farm in pty");
    drop(pair.slave);

    let reader = pair.master.try_clone_reader().expect("reader");
    let mut writer = pair.master.take_writer().expect("writer");
    let drainer = drain(reader);

    // Even though we send input, --non-interactive means the child's stdin is
    // /dev/null: `cat` gets EOF and exits without echoing anything.
    let _ = writer.write_all(b"SHOULD_NOT_APPEAR\n");
    let _ = writer.flush();

    let deadline = Instant::now() + Duration::from_secs(20);
    let status = loop {
        if let Some(s) = child.try_wait().expect("try_wait") {
            break s;
        }
        assert!(Instant::now() < deadline, "farm did not exit");
        std::thread::sleep(Duration::from_millis(50));
    };
    let console = String::from_utf8_lossy(&drainer.join().unwrap()).to_string();

    assert!(status.success(), "run should succeed (cat on empty stdin). console:\n{console}");

    let log = fs::read_to_string(ws.join(".farm/run/echo_in/log/echo_in_default.log")).unwrap();
    assert!(
        !log.contains("SHOULD_NOT_APPEAR"),
        "--non-interactive must not forward stdin to the child:\n{log}"
    );
}
