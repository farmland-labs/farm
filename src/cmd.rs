//! SPDX-License-Identifier: MIT OR Apache-2.0

use std::process::{Command, Stdio};
use std::io::{BufRead, BufReader};
use std::thread;
use std::sync::mpsc;
use crate::vendor::log::debug;

use anyhow::Result;

use crate::task::Context;

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
            for line in reader.lines().map_while(Result::ok) {
              // Print to the console first so the live stdout stream (which
              // a parent process may capture) is never delayed by the
              // per-line log flush below.
              if !silent {
                println!("{}", line);
              }
              if let Some(ref l) = log {
                l.stdout_line(&line);
              }
            }
          });
          handles.push(handle);
        }

        if let Some(stderr_handle) = child.stderr.take() {
          let silent = ctx.silent;
          let log = log.clone();
          let handle = thread::spawn(move || {
            let reader = BufReader::new(stderr_handle);
            for line in reader.lines().map_while(Result::ok) {
              if !silent {
                eprintln!("{}", line);
              }
              if let Some(ref l) = log {
                l.stderr_line(&line);
              }
            }
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
            for line in reader.lines().map_while(Result::ok) {
              let _ = tx_stdout.send(line);
            }
          });
          handles.push(handle);
        }

        if let Some(stderr_handle) = child.stderr.take() {
          let tx_stderr = tx;
          let handle = thread::spawn(move || {
            let reader = BufReader::new(stderr_handle);
            for line in reader.lines().map_while(Result::ok) {
              let _ = tx_stderr.send(line);
            }
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