//! SPDX-License-Identifier: MIT OR Apache-2.0

use std::process::{Command, Stdio};
use std::io::{BufRead, BufReader};
use std::borrow::Cow;
use std::thread;
use std::sync::mpsc;
use crate::vendor::log::debug;

use anyhow::Result;

use crate::task::Context;

/// Stream a child's output line by line, tolerant of non-UTF-8 bytes.
///
/// [`BufRead::lines`] yields `Err` on the first non-UTF-8 byte or read
/// error; the old `lines().map_while(Result::ok)` turned that into loop
/// termination, *silently truncating the rest of the stream* with no
/// signal on farm's stdout or in the log. This reads raw bytes instead
/// and decodes each line with [`String::from_utf8_lossy`], so invalid
/// UTF-8 becomes `U+FFFD` and output is preserved rather than lost.
///
/// The first lossy line emits a one-time `[farm] warning:` notice, and a
/// genuine read error emits a notice before stopping — both through the
/// same `emit` sink as normal output, so they land wherever the affected
/// output would have (console + log, on the right stream). `stream` is
/// `"stdout"` or `"stderr"`, used only in the notice text.
///
/// Newline handling matches `lines()`: a trailing `\n` (and a `\r`
/// directly before it) is stripped; a final unterminated line is kept.
fn stream_lines(mut reader: impl BufRead, stream: &str, mut emit: impl FnMut(String)) {
  let mut buf = Vec::new();
  let mut warned_lossy = false;
  loop {
    buf.clear();
    match reader.read_until(b'\n', &mut buf) {
      Ok(0) => break, // EOF
      Ok(_) => {
        if buf.last() == Some(&b'\n') {
          buf.pop();
          if buf.last() == Some(&b'\r') {
            buf.pop();
          }
        }
        match String::from_utf8_lossy(&buf) {
          Cow::Borrowed(s) => emit(s.to_owned()),
          Cow::Owned(s) => {
            if !warned_lossy {
              warned_lossy = true;
              emit(format!(
                "[farm] warning: non-UTF-8 bytes on {stream}; decoded lossily (output preserved, not truncated)"
              ));
            }
            emit(s);
          }
        }
      }
      Err(e) => {
        emit(format!(
          "[farm] warning: read error on {stream}: {e}; output may be incomplete"
        ));
        break;
      }
    }
  }
}

#[derive(Debug, Clone)]
pub struct CommandResult {
    pub success: bool,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}



pub struct WrappedCommand<'i> {
  parts: Vec<&'i str>
}

impl<'i> WrappedCommand<'i> {

  pub fn new(args: &[&'i str]) -> WrappedCommand<'i> {
    WrappedCommand {
      parts: args.to_vec(),
    }
  }

  pub fn run(&self, ctx: &Context) -> Result<CommandResult> {
    if let Some((bin, args)) = self.parts.split_first() {
      // Interactive (PTY) execution: the developer is at a real terminal and
      // may need to answer a prompt. Diverges entirely from the piped path
      // below — the child gets a real tty, not /dev/null on stdin. See ADR-081.
      if ctx.opt_interactive {
        return self.run_interactive(bin, args, ctx);
      }

      // Check if we're running interactively (connected to a terminal)
      let is_terminal = atty::is(atty::Stream::Stdout);
      
      let mut cmd = Command::new(bin);
      cmd.args(args)
        .envs(ctx.env.map.iter())
        // Operations run non-interactively. The child's stdin is /dev/null so
        // a process that tries to read input (e.g. a config prompt) gets EOF
        // immediately and fails fast, instead of blocking forever on a prompt
        // farm can't display (stdout is piped) and the user can't answer.
        // Inheriting farm's stdin here would silently deadlock under capture —
        // and always hangs when run head-less (e.g. by a CI runner).
        // Interactive execution (a real PTY behind an opt-in flag) is deferred.
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
      
      // If running in a terminal, tell child processes to enable color output
      if is_terminal {
        cmd.env("FORCE_COLOR", "1")        // Generic
           .env("CLICOLOR_FORCE", "1")      // Generic CLI tools
           .env("CARGO_TERM_COLOR", "always"); // Cargo specifically
      }
      
      // Spawn failures (e.g. the shell is missing) become a failed result, not
      // a panic — a build runner must report the error, not abort.
      let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
          let msg = format!("failed to start command {:?}: {}", self.parts, e);
          if !ctx.silent {
            eprintln!("❌ {}", msg);
          }
          if let Some(ref l) = ctx.log {
            l.stderr_line(&format!("[farm] {}", msg));
          }
          return Ok(CommandResult {
            success: false,
            exit_code: None,
            stdout: String::new(),
            stderr: msg,
          });
        }
      };

      debug!(ppid = std::process::id(), pid = ?child.id(), cmd = ?self.parts, "spawn");

      // Optional sink that streams each output line into the build log file
      // as it arrives (line-by-line, flushed). Cloned into the reader threads.
      // Output is NOT accumulated in memory — it is streamed and discarded.
      let log = ctx.log.clone();
      let mut handles = vec![];

      if ctx.split_streams {
        // Split mode: stdout and stderr handled on separate threads, each
        // streamed to its own sink (may interleave on the console).
        if let Some(stdout_handle) = child.stdout.take() {
          let silent = ctx.silent;
          let log = log.clone();
          let handle = thread::spawn(move || {
            let reader = BufReader::new(stdout_handle);
            stream_lines(reader, "stdout", |line| {
              // Print to the console first so the live stdout stream (which
              // a parent process may capture) is never delayed by the
              // per-line log flush below.
              if !silent {
                println!("{}", line);
              }
              if let Some(ref l) = log {
                l.stdout_line(&line);
              }
            });
          });
          handles.push(handle);
        }

        if let Some(stderr_handle) = child.stderr.take() {
          let silent = ctx.silent;
          let log = log.clone();
          let handle = thread::spawn(move || {
            let reader = BufReader::new(stderr_handle);
            stream_lines(reader, "stderr", |line| {
              if !silent {
                eprintln!("{}", line);
              }
              if let Some(ref l) = log {
                l.stderr_line(&line);
              }
            });
          });
          handles.push(handle);
        }

        for handle in handles {
          let _ = handle.join();
        }
      } else {
        // Merge mode (default): use a channel to serialize output in arrival order.
        let (tx, rx) = mpsc::channel::<String>();

        if let Some(stdout_handle) = child.stdout.take() {
          let tx_stdout = tx.clone();
          let handle = thread::spawn(move || {
            let reader = BufReader::new(stdout_handle);
            stream_lines(reader, "stdout", |line| {
              let _ = tx_stdout.send(line);
            });
          });
          handles.push(handle);
        }

        if let Some(stderr_handle) = child.stderr.take() {
          let tx_stderr = tx;
          let handle = thread::spawn(move || {
            let reader = BufReader::new(stderr_handle);
            stream_lines(reader, "stderr", |line| {
              let _ = tx_stderr.send(line);
            });
          });
          handles.push(handle);
        }

        // Receiver thread: prints (console first) then streams to the log in
        // arrival order. Merged streams land in the single combined writer.
        let silent = ctx.silent;
        let log_recv = log.clone();
        let receiver_handle = thread::spawn(move || {
          for line in rx {
            if !silent {
              println!("{}", line);
            }
            if let Some(ref l) = log_recv {
              l.stdout_line(&line);
            }
          }
        });

        // Wait for reader threads to complete (this closes the channel)
        for handle in handles {
          let _ = handle.join();
        }
        // Then wait for receiver to finish
        let _ = receiver_handle.join();
      }

      let status = match child.wait() {
        Ok(status) => status,
        Err(e) => {
          return Ok(CommandResult {
            success: false,
            exit_code: None,
            stdout: String::new(),
            stderr: format!("failed to wait on child process: {}", e),
          });
        }
      };
      let exit_code = status.code();
      let success = status.success();

      debug!(ppid = std::process::id(), pid = child.id(), code = ?exit_code, success = success, "spawn complete");

      // Output was streamed to the log + console as it arrived; nothing is
      // buffered here (no consumer reads these strings — see plan).
      return Ok(CommandResult {
        success,
        exit_code,
        stdout: String::new(),
        stderr: String::new(),
      });
    }
    
    // Return failure if no command parts
    Ok(CommandResult {
      success: false,
      exit_code: None,
      stdout: String::new(),
      stderr: "No command to execute".to_string(),
    })
  }

  /// Run the command through a PTY so the child sees a real terminal and the
  /// developer can answer prompts. farm relays keystrokes (stdin → child) and
  /// the child's output (child → console) while tee-ing a clean, ANSI-stripped
  /// copy into the build log. A PTY exposes a single stream, so stdout/stderr
  /// are merged here (the CLI rejects `--split-streams` + interactive). Only
  /// reached when `ctx.opt_interactive` is true — i.e. a real controlling TTY
  /// with no `--non-interactive`. See ADR-081.
  fn run_interactive(&self, bin: &str, args: &[&str], ctx: &Context) -> Result<CommandResult> {
    use portable_pty::{native_pty_system, CommandBuilder, PtySize};
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    // Size the PTY to the real terminal (sane fallback if we can't query it).
    let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));

    let pty_system = native_pty_system();
    let pair = pty_system
      .openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
      .map_err(|e| anyhow::anyhow!("failed to allocate pty: {e}"))?;

    // portable-pty's CommandBuilder inherits the parent environment; ctx.env
    // already carries that plus farm's FARM_* injections, so overlaying it
    // matches the piped path. cwd is inherited (as with std::process::Command).
    let mut builder = CommandBuilder::new(bin);
    builder.args(args);
    for (k, v) in ctx.env.map.iter() {
      builder.env(k, v);
    }
    if let Ok(cwd) = std::env::current_dir() {
      builder.cwd(cwd);
    }

    let mut child = match pair.slave.spawn_command(builder) {
      Ok(child) => child,
      Err(e) => {
        let msg = format!("failed to start command {:?}: {}", self.parts, e);
        if !ctx.silent {
          eprintln!("❌ {}", msg);
        }
        if let Some(ref l) = ctx.log {
          l.stderr_line(&format!("[farm] {}", msg));
        }
        return Ok(CommandResult { success: false, exit_code: None, stdout: String::new(), stderr: msg });
      }
    };
    // A cloneable killer + the child's pid let the stdin forwarder force-kill
    // the task on a double Ctrl-C without sharing the `&mut child` the main
    // thread needs for `wait()`.
    let killer = child.clone_killer();
    let child_pid = child.process_id();
    // Drop our slave handle so the master reads EOF once the child (now the
    // only slave holder) exits.
    drop(pair.slave);

    let mut reader = pair
      .master
      .try_clone_reader()
      .map_err(|e| anyhow::anyhow!("pty reader: {e}"))?;
    let writer = pair
      .master
      .take_writer()
      .map_err(|e| anyhow::anyhow!("pty writer: {e}"))?;

    debug!(ppid = std::process::id(), cmd = ?self.parts, "spawn (pty)");

    // Defensive safety net: restore the terminal if farm itself is signalled
    // (e.g. SIGTERM/SIGHUP from another shell, or SIGINT in the rare case raw
    // mode failed to engage). Installed once for the process. See ADR-081.
    #[cfg(unix)]
    install_terminal_restore_handler();

    // Put our own terminal into raw mode so keystrokes pass to the child
    // char-by-char. Scoped to the relay only — restored on every exit path by
    // the guard's Drop, so farm's surrounding progress output stays cooked.
    let _raw = RawModeGuard::enable();

    let master = Arc::new(Mutex::new(pair.master));
    let stop = Arc::new(AtomicBool::new(false));

    // Thread: real stdin → PTY master (keystrokes reach the child). Also
    // forwards terminal resizes on Unix and force-kills the task on a double
    // Ctrl-C. Torn down via `stop` after the child exits so it never lingers
    // into the next serial operation.
    let stdin_handle = spawn_stdin_forwarder(writer, stop.clone(), master.clone(), killer, child_pid);

    // This thread: PTY master → console + log tee. Runs until the child closes
    // the slave (EOF on exit). The slave's ONLCR already turned the child's
    // "\n" into "\r\n", so console bytes need no translation.
    let log = ctx.log.clone();
    let silent = ctx.silent;
    let mut stdout = std::io::stdout();
    let mut pending: Vec<u8> = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
      match reader.read(&mut buf) {
        Ok(0) => break,
        Ok(n) => {
          let chunk = &buf[..n];
          if !silent {
            let _ = stdout.write_all(chunk);
            let _ = stdout.flush();
          }
          // Log tee: reassemble lines, strip ANSI + trailing CR per line
          // (LogStream::stdout_line strips ANSI). A newline-less prompt stays
          // buffered until its line completes — the console already showed it.
          if let Some(ref l) = log {
            pending.extend_from_slice(chunk);
            while let Some(pos) = pending.iter().position(|&b| b == b'\n') {
              let mut line: Vec<u8> = pending.drain(..=pos).collect();
              line.pop(); // drop '\n'
              if line.last() == Some(&b'\r') { line.pop(); }
              l.stdout_line(&String::from_utf8_lossy(&line));
            }
          }
        }
        Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
        Err(_) => break,
      }
    }
    // Flush any trailing partial line (no terminating newline) into the log.
    if let Some(ref l) = log {
      if !pending.is_empty() {
        if pending.last() == Some(&b'\r') { pending.pop(); }
        l.stdout_line(&String::from_utf8_lossy(&pending));
      }
    }

    let status = child
      .wait()
      .map_err(|e| anyhow::anyhow!("failed to wait on child process: {e}"))?;

    // Tear down the stdin forwarder now the child is gone. On Unix the poll
    // loop observes `stop` within its timeout and joins cleanly; on Windows a
    // blocking console read can't be interrupted, so we detach it (ADR-081).
    stop.store(true, Ordering::SeqCst);
    #[cfg(unix)]
    let _ = stdin_handle.join();
    #[cfg(not(unix))]
    drop(stdin_handle);

    let success = status.success();
    let exit_code = Some(status.exit_code() as i32);
    debug!(cmd = ?self.parts, code = ?exit_code, success = success, "spawn complete (pty)");

    Ok(CommandResult { success, exit_code, stdout: String::new(), stderr: String::new() })
  }

}

/// RAII guard for terminal raw mode: enables on construction, restores on drop
/// so no early return or `?` leaves the developer's shell wedged.
struct RawModeGuard {
  active: bool,
}

impl RawModeGuard {
  fn enable() -> Self {
    // A failure to enter raw mode is non-fatal — the relay still works, input
    // is just line-buffered by the outer terminal.
    let active = crossterm::terminal::enable_raw_mode().is_ok();
    RawModeGuard { active }
  }
}

impl Drop for RawModeGuard {
  fn drop(&mut self) {
    if self.active {
      let _ = crossterm::terminal::disable_raw_mode();
    }
  }
}

/// Ctrl-C byte. In raw mode farm receives this as data, not a signal — the
/// forwarder relays it into the child's PTY (where it becomes the task's
/// SIGINT), and uses a second consecutive one to escalate to a force-kill.
const CTRL_C: u8 = 0x03;

/// Emit a farm-origin notice on its own line while the terminal is in raw mode
/// (raw mode does not translate `\n`, so we write an explicit `\r\n`).
fn raw_notice(msg: &str) {
  use std::io::Write;
  let mut err = std::io::stderr();
  let _ = write!(err, "\r\n[farm] {msg}\r\n");
  let _ = err.flush();
}

/// Force-kill the task on a double Ctrl-C. The child is a session/process-group
/// leader (portable-pty `setsid`), so on Unix we `killpg` the whole group —
/// grandchildren die too, not just the immediate shell — and also call the
/// portable killer (the only path on Windows).
fn force_kill(killer: &mut Box<dyn portable_pty::ChildKiller + Send + Sync>, pid: Option<u32>) {
  #[cfg(unix)]
  {
    if let Some(pid) = pid {
      unsafe { libc::killpg(pid as libc::pid_t, libc::SIGKILL); }
    }
  }
  #[cfg(not(unix))]
  { let _ = pid; }
  let _ = killer.kill();
}

/// Feed a chunk of the developer's keystrokes to the child, applying the
/// double-Ctrl-C policy (ADR-081): the first `^C` is forwarded (the task gets
/// SIGINT); a second *consecutive* `^C` while the task is still running
/// force-kills it. Any non-`^C` byte resets the "armed" state, so only back-to-
/// back `^C` escalates. Returns `false` when the forwarder should stop (write
/// failure or a force-kill was issued).
fn relay_input(
  chunk: &[u8],
  writer: &mut Box<dyn std::io::Write + Send>,
  armed: &mut bool,
  killer: &mut Box<dyn portable_pty::ChildKiller + Send + Sync>,
  pid: Option<u32>,
) -> bool {
  use std::io::Write;
  for &b in chunk {
    if b == CTRL_C {
      if *armed {
        // Second consecutive ^C: the task ignored the first — force-kill it.
        raw_notice("force-killing the task…");
        force_kill(killer, pid);
        return false;
      }
      // Forward this first ^C so the task receives its SIGINT, then arm and
      // hint at the escape hatch.
      if writer.write_all(&[b]).is_err() {
        return false;
      }
      let _ = writer.flush();
      *armed = true;
      raw_notice("interrupting — press Ctrl-C again to force-kill");
    } else {
      // Any other keystroke breaks the ^C-^C sequence.
      if writer.write_all(&[b]).is_err() {
        return false;
      }
      *armed = false;
    }
  }
  let _ = writer.flush();
  true
}

/// Spawn the stdin → PTY-master forwarder. Unix polls stdin with a timeout so
/// it can observe the `stop` flag (prompt teardown) and forward SIGWINCH
/// resizes to the PTY; Windows uses a plain blocking read (detached on exit).
/// Both apply the double-Ctrl-C force-kill policy.
#[cfg(unix)]
fn spawn_stdin_forwarder(
  mut writer: Box<dyn std::io::Write + Send>,
  stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
  master: std::sync::Arc<std::sync::Mutex<Box<dyn portable_pty::MasterPty + Send>>>,
  mut killer: Box<dyn portable_pty::ChildKiller + Send + Sync>,
  pid: Option<u32>,
) -> thread::JoinHandle<()> {
  use std::io::Read;
  use std::os::unix::io::AsRawFd;
  use std::sync::atomic::Ordering;
  use std::sync::Arc;
  use std::sync::atomic::AtomicBool;

  // SIGWINCH sets a flag; the loop resizes the PTY so the child's TIOCGWINSZ
  // stays correct (TUIs redraw on resize).
  let winch = Arc::new(AtomicBool::new(false));
  let _ = signal_hook::flag::register(signal_hook::consts::SIGWINCH, winch.clone());

  thread::spawn(move || {
    let fd = std::io::stdin().as_raw_fd();
    let mut buf = [0u8; 1024];
    let mut armed = false;
    loop {
      if stop.load(Ordering::SeqCst) {
        break;
      }
      if winch.swap(false, Ordering::SeqCst) {
        if let Ok((cols, rows)) = crossterm::terminal::size() {
          if let Ok(m) = master.lock() {
            let _ = m.resize(portable_pty::PtySize { rows, cols, pixel_width: 0, pixel_height: 0 });
          }
        }
      }
      // Wait up to 100ms for input, re-checking `stop`/`winch` between waits so
      // teardown is prompt and no read blocks past the child's lifetime.
      let mut pfd = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
      let r = unsafe { libc::poll(&mut pfd, 1, 100) };
      if r < 0 {
        if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
          continue;
        }
        break;
      }
      if r == 0 || pfd.revents & libc::POLLIN == 0 {
        continue;
      }
      match std::io::stdin().read(&mut buf) {
        Ok(0) => break,
        Ok(n) => {
          if !relay_input(&buf[..n], &mut writer, &mut armed, &mut killer, pid) {
            break;
          }
        }
        Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
        Err(_) => break,
      }
    }
  })
}

/// Windows stdin forwarder: a blocking console read that cannot be interrupted
/// from another thread, so it is effectively detached — it ends when the child
/// exits and the next keystroke fails to write to the closed master. ConPTY
/// parity and clean teardown are validated separately (ADR-081).
#[cfg(not(unix))]
fn spawn_stdin_forwarder(
  mut writer: Box<dyn std::io::Write + Send>,
  _stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
  _master: std::sync::Arc<std::sync::Mutex<Box<dyn portable_pty::MasterPty + Send>>>,
  mut killer: Box<dyn portable_pty::ChildKiller + Send + Sync>,
  pid: Option<u32>,
) -> thread::JoinHandle<()> {
  use std::io::Read;
  thread::spawn(move || {
    let mut buf = [0u8; 1024];
    let mut armed = false;
    loop {
      match std::io::stdin().read(&mut buf) {
        Ok(0) => break,
        Ok(n) => {
          if !relay_input(&buf[..n], &mut writer, &mut armed, &mut killer, pid) {
            break;
          }
        }
        Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
        Err(_) => break,
      }
    }
  })
}

/// Restore the terminal if farm itself is signalled while a PTY relay holds it
/// in raw mode — SIGTERM/SIGHUP from another shell, or SIGINT in the rare case
/// raw mode failed to engage (otherwise `^C` never reaches farm as a signal).
/// Installed once for the process. signal-hook delivers on a dedicated thread
/// in normal context, so touching crossterm and exiting here is safe.
#[cfg(unix)]
fn install_terminal_restore_handler() {
  use std::sync::Once;
  static ONCE: Once = Once::new();
  ONCE.call_once(|| {
    use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
    if let Ok(mut signals) = signal_hook::iterator::Signals::new([SIGINT, SIGTERM, SIGHUP]) {
      thread::spawn(move || {
        if let Some(sig) = signals.forever().next() {
          let _ = crossterm::terminal::disable_raw_mode();
          // Conventional shell exit code for death by signal.
          std::process::exit(128 + sig);
        }
      });
    }
  });
}

impl<'i> std::fmt::Debug for WrappedCommand<'i> {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
      write!(f, "WrappedCommand: {:?}", self.parts)
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::io::Cursor;

  fn collect(data: &[u8], stream: &str) -> Vec<String> {
    let mut out = Vec::new();
    stream_lines(Cursor::new(data.to_vec()), stream, |l| out.push(l));
    out
  }

  #[test]
  fn valid_utf8_unchanged_crlf_stripped_and_final_line_kept() {
    // "a\r\n" -> "a"; "b\n" -> "b"; trailing "c" with no newline -> "c".
    let out = collect(b"a\r\nb\nc", "stdout");
    assert_eq!(out, vec!["a", "b", "c"]);
    assert!(!out.iter().any(|l| l.contains("non-UTF-8")));
  }

  #[test]
  fn non_utf8_decoded_lossily_not_truncated() {
    // The bug: a bad byte mid-stream used to drop every line after it.
    // Now the rest of the stream must survive, lossily decoded.
    let out = collect(b"ok\n\xffbad\nmore\n", "stdout");
    assert_eq!(out[0], "ok");
    assert!(out[1].contains("non-UTF-8"), "missing warning: {out:?}");
    assert_eq!(out[2], "\u{fffd}bad");
    assert_eq!(out[3], "more");
  }

  #[test]
  fn lossy_warning_emitted_only_once_per_stream() {
    let out = collect(b"\xffone\n\xfftwo\n", "stderr");
    let warnings = out.iter().filter(|l| l.contains("non-UTF-8")).count();
    assert_eq!(warnings, 1, "{out:?}");
    // Both corrupted lines are still present despite the single warning.
    assert!(out.iter().any(|l| l == "\u{fffd}one"), "{out:?}");
    assert!(out.iter().any(|l| l == "\u{fffd}two"), "{out:?}");
  }

  #[test]
  fn empty_lines_preserved() {
    let out = collect(b"\n\nx\n", "stdout");
    assert_eq!(out, vec!["", "", "x"]);
  }
}