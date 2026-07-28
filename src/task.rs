//! SPDX-License-Identifier: MIT OR Apache-2.0

use std::fmt;
use std::convert::From;
use std::fmt::Debug;

use crate::cmd::WrappedCommand;
use crate::env::CapturedEnv;

#[derive(Debug, Clone)]
pub struct TaskExecutionResult {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
}

// Define a trait that combines all our requirements
pub trait Task: Callable + Send + Sync + Debug {}

// Blanket implementation for any type that meets the requirements
impl<T> Task for T where T: Callable + Send + Sync + Debug {}



pub struct Context<'a> {
  pub env: &'a CapturedEnv,
  pub silent: bool,
  pub split_streams: bool,
  /// When `true`, this operation runs through a PTY so the child sees a real
  /// terminal and a developer can answer prompts; farm relays keystrokes and
  /// tees a clean (ANSI-stripped) log. When `false` (a tool/buddy passed
  /// `--non-interactive`, or there is no controlling TTY) the child's stdin is
  /// `/dev/null` and it fails fast on any read. Positive polarity: `true` =
  /// interactive on. Resolved once at the CLI boundary. See ADR-081.
  pub opt_interactive: bool,
  /// Optional sink for streaming command output line-by-line into the build
  /// log file as it arrives. `None` when there is no file logging (or in
  /// non-engine callers such as tests).
  pub log: Option<crate::build_logger::LogStream>,
}



pub trait Callable {
  fn run(&self, ctx: &Context) -> TaskExecutionResult;
  fn label(&self) -> String;
}


impl fmt::Debug for dyn Callable {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(f, "Callable: {}", self.label())
  }
}

#[derive(Debug, Default)]
pub struct Exec {
  args: Vec<String>,
}

impl Callable for Exec {
  fn run(&self, ctx: &Context) -> TaskExecutionResult {
    let args: Vec<&str> = self.args.iter().map(|a| a.as_str()).collect();
    let cmd = WrappedCommand::new(&args);
    match cmd.run(ctx) {
      Ok(result) => TaskExecutionResult {
        success: result.success,
        stdout: result.stdout,
        stderr: result.stderr,
        exit_code: result.exit_code,
      },
      Err(e) => TaskExecutionResult {
        success: false,
        stdout: String::new(),
        stderr: format!("Command execution failed: {}", e),
        exit_code: None,
      }
    }
  }

  fn label(&self) -> String {
    self.args.join(" ")
  }
}

impl From<Vec<&str>> for Exec {
  fn from(args: Vec<&str>) -> Self {
    Self {
      args: args.into_iter().map(|a| a.to_string()).collect(),
    }
  }
}
