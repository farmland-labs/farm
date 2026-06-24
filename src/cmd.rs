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