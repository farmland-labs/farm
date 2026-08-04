//! SPDX-License-Identifier: MIT OR Apache-2.0

//! Command-line interface definition (clap) and shared CLI helpers.

use std::path::PathBuf;
use clap::{Parser, Subcommand};
use farm::vendor::log::{info, warn};

use crate::version_string;
use crate::cmd::cache::CacheCommands;
use crate::cmd::ctx::CtxCommands;

#[derive(Parser, Clone)]
#[command(
    author = "Christian Staffa",
    version = version_string(),
    about = "Farm - Build operations runner",
    long_about = "A build operation executor with dependency resolution and simple caching."
)]
pub(crate) struct Args {
    /// Farmfile describing the operations and dependencies
    #[arg(
        long,
        short = 'f',
        default_value = "Farmfile",
        help = "Path to the Farm operation file (env: FARM_FARMFILE)"
    )]
    pub(crate) farmfile: PathBuf,

    /// the variant to use on the execution
    #[arg(
        long,
        default_value = "default",
        help = "The variant to use for execution"
    )]
    pub(crate) variant: String,

    /// The target operation to reach
    #[arg(
        long,
        help = "The target operation to reach"
    )]
    pub(crate) target: Option<String>,

    /// Farm runtime directory
    #[arg(
        long,
        help = "Directory for Farm runtime files (logs, cache, etc.)"
    )]
    pub(crate) farm_dir: Option<PathBuf>,

    /// Build ID for this run (overrides FARM_BUILD_ID env var)
    #[arg(
        long,
        help = "Build ID for run isolation (default: <goal>)"
    )]
    pub(crate) build_id: Option<String>,

    /// Suppress task output to stdout (only show operation progress)
    #[arg(
        long,
        help = "Suppress real-time task output, only show final results"
    )]
    pub(crate) silent: bool,

    /// Control where logs are written
    #[arg(
        long,
        default_value = "file",
        help = "Where to write logs: 'file' (combined, default), 'file-split' (separate stdout/stderr), 'stdout', or 'none'"
    )]
    pub(crate) log_output: String,

    /// Run only the specified target without its dependencies
    #[arg(
        long,
        short = '1',
        help = "Run only the specified target, skip dependencies"
    )]
    pub(crate) only: bool,

    /// Keep stdout and stderr as separate streams (by default they are merged for correct ordering)
    #[arg(
        long,
        help = "Keep stdout and stderr as separate streams (may cause ordering issues)"
    )]
    pub(crate) split_streams: bool,

    /// Bypass the target cache (always re-execute even if inputs unchanged)
    #[arg(
        long,
        short = 'n',
        help = "Bypass the target cache and always re-execute operations"
    )]
    pub(crate) no_cache: bool,

    /// Force non-interactive execution: children get /dev/null on stdin and
    /// fail fast on any prompt, even when a TTY is present. Tools/buddies pass
    /// this. Without it, a run on a controlling TTY is interactive (ADR-081).
    #[arg(
        long,
        help = "Force non-interactive execution (close child stdin even on a TTY)"
    )]
    pub(crate) non_interactive: bool,

    /// Enable verbose logging (debug level)
    #[arg(
        long,
        help = "Enable verbose logging (debug level)"
    )]
    pub(crate) verbose: bool,

    #[command(subcommand)]
    pub(crate) command: Option<Commands>,
}

#[derive(Subcommand, Clone)]
pub(crate) enum Commands {
    /// Run a target operation (shortcut for --target)
    Run {
        /// The target operation to run
        target: String,

        /// Farmfile describing the operations and dependencies
        #[arg(
            long,
            short = 'f',
            default_value = "Farmfile",
            help = "Path to the Farm operation file (env: FARM_FARMFILE)"
        )]
        farmfile: PathBuf,

        /// the variant to use on the execution
        #[arg(
            long,
            default_value = "default",
            help = "The variant to use for execution"
        )]
        variant: String,

        /// Farm runtime directory
        #[arg(
            long,
            help = "Directory for Farm runtime files (logs, cache, etc.)"
        )]
        farm_dir: Option<PathBuf>,

        /// Build ID for this run (overrides FARM_BUILD_ID env var)
        #[arg(
            long,
            help = "Build ID for run isolation (default: <goal>)"
        )]
        build_id: Option<String>,

        /// Suppress task output to stdout (only show operation progress)
        #[arg(
            long,
            help = "Suppress real-time task output, only show final results"
        )]
        silent: bool,

        /// Control where logs are written
        #[arg(
            long,
            default_value = "file",
            help = "Where to write logs: 'file' (combined, default), 'file-split' (separate stdout/stderr), 'stdout', or 'none'"
        )]
        log_output: String,

        /// Run only the specified target without its dependencies
        #[arg(
            long,
            short = '1',
            help = "Run only the specified target, skip dependencies"
        )]
        only: bool,

        /// Keep stdout and stderr as separate streams (by default they are merged for correct ordering)
        #[arg(
            long,
            help = "Keep stdout and stderr as separate streams (may cause ordering issues)"
        )]
        split_streams: bool,

        /// Bypass the target cache (always re-execute even if inputs unchanged)
        #[arg(
            long,
            short = 'n',
            help = "Bypass the target cache and always re-execute operations"
        )]
        no_cache: bool,

        /// Analyze file changes during execution to detect missing input declarations
        #[arg(
            long,
            help = "Track file changes and report undeclared inputs after execution"
        )]
        analyze: bool,
        
        /// Track entire workspace instead of just declared outputs
        #[arg(
            long,
            help = "Track all files in workspace (implies --analyze)"
        )]
        track_workspace: bool,
        
        /// Additional paths to track (can be specified multiple times)
        #[arg(
            long = "track-path",
            help = "Additional paths/patterns to track (implies --analyze)"
        )]
        track_paths: Vec<String>,
        
        /// Exclude paths matching glob from tracking (comma-separated)
        #[arg(
            long = "track-excludes",
            value_delimiter = ',',
            help = "Exclude paths matching glob (comma-separated, dir/ expands to dir/**/*)"
        )]
        track_excludes: Vec<String>,
        
        /// Keep snapshot files after analysis (for debugging)
        #[arg(
            long,
            help = "Keep .farm/.analyze.STAGE.before.txt and .after.txt files"
        )]
        keep_snapshots: bool,
        
        /// Disable interactive mode (no pause after analysis)
        #[arg(
            long,
            help = "Don't pause after analysis to review changes"
        )]
        non_interactive: bool,
    },

    /// Parse and display the operation plan without executing
    Plan {
        /// Farmfile describing the operations and dependencies
        #[arg(
            long,
            short = 'f',
            default_value = "Farmfile",
            help = "Path to the Farm operation file (env: FARM_FARMFILE)"
        )]
        farmfile: PathBuf,

        /// Filter operations by variant
        #[arg(
            long,
            help = "Filter operations by variant"
        )]
        variant: Option<String>,

        /// Output format (tree, list, dot, json, structure)
        #[arg(
            long,
            default_value = "tree",
            help = "Output format (tree, list, dot, json, structure)"
        )]
        format: String,

        /// Show detailed task information for each operation
        #[arg(
            long,
            short = 'v',
            help = "Show task details for each operation"
        )]
        verbose: bool,
    },
    
    /// Manage named contexts for build state isolation
    Ctx {
        #[command(subcommand)]
        command: Option<CtxCommands>,
    },
    
    /// Re-run a build from preserved ops directory
    Replay {
        /// Goal name, build ID, or path to a run directory
        build_dir: String,
        
        /// Dry run - show what would be executed without running
        #[arg(long)]
        dry_run: bool,
        
        /// Override the Farmfile path
        #[arg(long, short = 'f')]
        farmfile: Option<PathBuf>,
    },
    
    /// Show, list or diff the logs of previous runs
    ///
    /// Every run keeps its own log, so a failing run can be compared against
    /// the last working one. Run history is bounded automatically.
    #[command(after_help = "\
EXAMPLES:
    farm log                     Log of the most recent run, whatever the goal
    farm log test                Log of the most recent `test` run
    farm log test --list         Recent `test` runs, newest first
    farm log test --diff         Compare the last two `test` runs
    farm log test --diff --against 3   Compare against 3 runs back
    farm log test --follow       Follow a run as it executes")]
    Log {
        /// Goal to show logs for. Omit for the most recent run of any goal.
        goal: Option<String>,

        /// Restrict to one variant. Omit to match any.
        #[arg(long)]
        variant: Option<String>,

        /// List recent runs instead of showing a log
        #[arg(long, short = 'l')]
        list: bool,

        /// Diff the latest run against an earlier one
        #[arg(long, short = 'd')]
        diff: bool,

        /// How many runs back to diff against (default: the previous run)
        #[arg(long, default_value = "1")]
        against: usize,

        /// Follow the log as it is written
        #[arg(long, short = 'F')]
        follow: bool,

        /// Show the log exactly as stored, skipping normalization
        #[arg(long)]
        raw: bool,

        /// Drop farm's own `[farm]` framing lines, leaving only task output
        #[arg(long)]
        only_output: bool,

        /// Print the log file's path instead of its contents
        ///
        /// The path is an internal implementation detail and will change
        /// between versions; do not script against it.
        #[arg(long)]
        path: bool,

        /// Number of runs to list (with --list)
        #[arg(long, short = 'n', default_value = "10")]
        limit: usize,
    },

    /// Manage target cache
    Cache {
        #[command(subcommand)]
        command: CacheCommands,
    },
    
    /// Show version information and banner
    Version,
}


/// Helper function to resolve Farmfile path with precedence:
///     CLI arg > env var > `Farmfile` > `farmfile`
///
/// When the user does not pass `--farmfile` and `FARM_FARMFILE` is unset,
/// the default `Farmfile` is tried first, then lowercase `farmfile`
/// (capitalised) as a fallback so projects that already use the
/// capitalised convention (Dockerfile-style) work out of the box.
pub(crate) fn resolve_farmfile(farmfile: PathBuf, explicitly_set: bool) -> PathBuf {
    if !explicitly_set {
        // CLI arg not explicitly set, check environment variable
        if let Ok(env_farmfile) = std::env::var("FARM_FARMFILE") {
            info!("Using Farmfile from FARM_FARMFILE environment variable: {}", env_farmfile);
            return PathBuf::from(env_farmfile);
        }

        // Default lowercase missing → try capitalised `Farmfile`.
        if !farmfile.is_file() {
            let cap = PathBuf::from("Farmfile");
            if cap.is_file() {
                return cap;
            }
        }
    } else if let Ok(env_farmfile) = std::env::var("FARM_FARMFILE") {
        warn!("FARM_FARMFILE environment variable set to '{}' but command-line argument '--farmfile {}' takes precedence",
              env_farmfile, farmfile.display());
    }
    farmfile
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::path::PathBuf;

    #[test]
    #[serial]
    fn test_resolve_farmfile_cli_explicit() {
        // CLI argument should always win, even if it's the default value
        let cli_arg = PathBuf::from("Farmfile");
        let result = resolve_farmfile(cli_arg.clone(), true);
        assert_eq!(result, cli_arg);
    }

    #[test]
    #[serial]
    fn test_resolve_farmfile_cli_custom() {
        // Custom CLI argument should be used
        let cli_arg = PathBuf::from("Farmfile.custom");
        let result = resolve_farmfile(cli_arg.clone(), true);
        assert_eq!(result, cli_arg);
    }

    #[test]
    #[serial]
    fn test_resolve_farmfile_env_var() {
        // Clean slate first
        std::env::remove_var("FARM_FARMFILE");
        
        // Set environment variable
        std::env::set_var("FARM_FARMFILE", "farmfile.test");
        
        // Default CLI value should be overridden by env var
        let cli_arg = PathBuf::from("farmfile");
        let result = resolve_farmfile(cli_arg, false);
        assert_eq!(result, PathBuf::from("farmfile.test"), 
                   "FARM_FARMFILE env var should override default farmfile");
        
        // Clean up
        std::env::remove_var("FARM_FARMFILE");
    }

    #[test]
    #[serial]
    fn test_resolve_farmfile_env_var_overrides_default() {
        // Set environment variable
        std::env::set_var("FARM_FARMFILE", "farmfile.alfs");
        
        // Even though CLI has "farmfile", env var should win if not explicitly set
        let cli_arg = PathBuf::from("farmfile");
        let result = resolve_farmfile(cli_arg, false);
        assert_eq!(result, PathBuf::from("farmfile.alfs"));
        
        // Clean up
        std::env::remove_var("FARM_FARMFILE");
    }

    #[test]
    #[serial]
    fn test_resolve_farmfile_cli_beats_env() {
        // Set environment variable
        std::env::set_var("FARM_FARMFILE", "farmfile.env");
        
        // Explicit CLI argument should beat env var
        let cli_arg = PathBuf::from("farmfile.cli");
        let result = resolve_farmfile(cli_arg.clone(), true);
        assert_eq!(result, cli_arg);
        
        // Clean up
        std::env::remove_var("FARM_FARMFILE");
    }

    #[test]
    #[serial]
    fn test_resolve_farmfile_default() {
        // Make sure no env var is set
        std::env::remove_var("FARM_FARMFILE");
        
        // Default should be used when no env var and not explicit
        let cli_arg = PathBuf::from("Farmfile");
        let result = resolve_farmfile(cli_arg.clone(), false);
        assert_eq!(result, cli_arg);
    }

    #[test]
    #[serial]
    fn test_resolve_farmfile_preserves_path() {
        // Set environment variable with a full path
        std::env::set_var("FARM_FARMFILE", "/tmp/custom/Farmfile.test");
        
        // Should preserve the full path
        let cli_arg = PathBuf::from("Farmfile");
        let result = resolve_farmfile(cli_arg, false);
        assert_eq!(result, PathBuf::from("/tmp/custom/Farmfile.test"));
        
        // Clean up
        std::env::remove_var("FARM_FARMFILE");
    }
}
