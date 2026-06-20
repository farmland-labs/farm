//! SPDX-License-Identifier: MIT OR Apache-2.0

//! Task definitions for parser
//! 
//! Tasks represent executable units within stages.

use std::fmt;
use std::fmt::Debug;

use serde::{Deserialize, Serialize};

/// Result of task execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskExecutionResult {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
}

/// Task represents an executable unit within a stage
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Task {
    Shell(ShellTask),
    // Future task types can be added here
    // Docker(DockerTask),
    // Script(ScriptTask),
}

impl Task {
    /// Create a new shell task
    pub fn shell<S: Into<String>>(command: S) -> Self {
        Task::Shell(ShellTask {
            command: command.into(),
        })
    }
    
    /// Get a human-readable label for the task
    pub fn label(&self) -> String {
        match self {
            Task::Shell(shell_task) => {
                // Truncate long commands for display
                let cmd = &shell_task.command;
                if cmd.len() > 50 {
                    format!("shell: {}...", &cmd[..47])
                } else {
                    format!("shell: {}", cmd)
                }
            }
        }
    }
    
    /// Get the command string for shell tasks
    pub fn command(&self) -> Option<&str> {
        match self {
            Task::Shell(shell_task) => Some(&shell_task.command),
        }
    }
}

/// Shell task that executes a shell command
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ShellTask {
    pub command: String,
}

impl ShellTask {
    pub fn new<S: Into<String>>(command: S) -> Self {
        Self {
            command: command.into(),
        }
    }
}

impl fmt::Display for Task {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.label())
    }
}
