//! SPDX-License-Identifier: MIT OR Apache-2.0

//! Farm - Build operations runner

use std::path::Path;
use clap::Parser;
use farm::vendor::log::info;

mod cli;
mod cmd;

use crate::cli::{Args, Commands, resolve_farmfile};
use crate::cmd::cache::handle_cache_command;
use crate::cmd::ctx::handle_ctx_command;
use crate::cmd::plan::handle_plan_command;
use crate::cmd::replay::handle_replay_command;
use crate::cmd::execute::handle_execute_command;
use crate::cmd::analyze::handle_analyze_run;

/// Build version string with git info (rustc-style format)
/// Example: 1.0.0 (abc123def 2025-12-08)
fn version_string() -> &'static str {
    static VERSION: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    VERSION.get_or_init(|| {
        let git_sha = option_env!("FARM_GIT_SHA").unwrap_or("unknown");
        let git_date = option_env!("FARM_GIT_DATE").unwrap_or("unknown");
        // Built without a git checkout (e.g. `cargo install` from crates.io):
        // show just the version rather than "(unknown unknown)".
        if git_sha == "unknown" {
            env!("CARGO_PKG_VERSION").to_string()
        } else {
            format!("{} ({} {})", env!("CARGO_PKG_VERSION"), git_sha, git_date)
        }
    }).as_str()
}

/// Print the branded `farm` banner to stderr. Uses the Farmland palette
/// (blue/terracotta/lime) as a diagonal three-square stack with a terminal
/// prompt — matching `farm/assets/`. Honors `NO_COLOR`.
fn print_version_banner() {
    let v = version_string();

    if std::env::var_os("NO_COLOR").is_some() {
        eprintln!();
        eprintln!("   farm — Build operations runner");
        eprintln!("   {v}  ·  farmland.rocks/farm");
        eprintln!();
        return;
    }

    // 24-bit (truecolor) brand palette.
    let blue = "\x1b[38;2;58;110;191m";
    let rust = "\x1b[38;2;191;104;69m";
    let lime = "\x1b[38;2;159;191;35m";
    let dim = "\x1b[2m";
    let bold = "\x1b[1m";
    let rst = "\x1b[0m";

    eprintln!();
    eprintln!("   {blue}██████{rst}");
    eprintln!("   {blue}██{rst}{rust}██████{rst}    {bold}farm{rst} {dim}—{rst} Build operations runner");
    eprintln!("   {blue}██{rst}{rust}██{rst}{lime}██████{rst}  {dim}© Christian Staffa · farmland.rocks/farm{rst}");
    eprintln!("     {rust}██{rst}{lime}███{rst}{rust}█{rst}{lime}██{rst}  {bold}{v}{rst}");
    eprintln!("       {lime}██████{rst}");
    eprintln!();
}

/// Load .env file from the current workspace directory if it exists
/// Note: By default, dotenv does NOT override existing environment variables
/// Precedence: shell env > .env file
fn load_dotenv() {
    // Try to load from current directory
    match dotenvy::from_filename(".env") {
        Ok(path) => {
            info!("Loaded environment variables from: {}", path.display());
        }
        Err(_) => {
            // .env file doesn't exist or couldn't be read - this is fine
        }
    }
}


fn main() {
    if let Err(e) = run() {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Load .env file (allows RUST_LOG and other env vars)
    load_dotenv();
    
    // 2. Parse CLI arguments
    let args = Args::parse();
    
    // 3. Initialize logging
    // In interactive terminals, suppress logs unless RUST_LOG is explicitly set or --verbose
    let is_interactive = std::io::IsTerminal::is_terminal(&std::io::stderr());
    let rust_log_set = std::env::var("RUST_LOG").is_ok();
    
    if args.verbose {
        farm::vendor::log::init_with_default("farm=debug,warn");
    } else if rust_log_set {
        // RUST_LOG is set, let farm::log handle it
        farm::vendor::log::init_with_default("farm=info,warn");
    } else if is_interactive {
        // Interactive terminal without RUST_LOG - no logs at all
        // Errors/warnings are shown via eprintln, not tracing
        farm::vendor::log::init_with_default("off");
    } else {
        // Non-interactive (CI/piped) - show info logs
        farm::vendor::log::init_with_default("farm=info,warn");
    }

    match args.command {
        Some(Commands::Run { target, farmfile, variant, farm_dir, build_id, silent, log_output, only, split_streams, no_cache, analyze, track_workspace, track_paths, track_excludes, keep_snapshots, non_interactive }) => {
            // For subcommands, we can't easily detect if arg was explicitly set via clap
            // So we use a heuristic: if Farmfile != default, it was likely explicitly set
            let explicitly_set = farmfile != Path::new("Farmfile");
            let farmfile = resolve_farmfile(farmfile, explicitly_set);
            
            let run_args = Args {
                farmfile,
                variant,
                target: Some(target),
                farm_dir,
                build_id,
                silent,
                log_output,
                only,
                split_streams,
                no_cache,
                non_interactive,
                verbose: args.verbose,
                command: None,
            };
            
            // Handle analyze mode (--analyze, --track-workspace, or --track-path implies analyze)
            if analyze || track_workspace || !track_paths.is_empty() {
                // Force no-cache in analyze mode - we need actual execution to track file changes
                let mut analyze_args = run_args;
                if !analyze_args.no_cache {
                    println!("ℹ️  Analyze mode implies --no-cache (need actual execution to track changes)");
                    analyze_args.no_cache = true;
                }
                return handle_analyze_run(analyze_args, track_workspace, track_paths, track_excludes, !non_interactive, keep_snapshots);
            }
            
            handle_execute_command(run_args)
        }
        Some(Commands::Plan { farmfile, variant, format, verbose }) => {
            // Same heuristic for plan subcommand
            let explicitly_set = farmfile != Path::new("Farmfile");
            let farmfile = resolve_farmfile(farmfile, explicitly_set);
            
            handle_plan_command(farmfile, variant, format, verbose)
        }
        Some(Commands::Ctx { command }) => {
            match command {
                Some(cmd) => handle_ctx_command(cmd),
                None => {
                    // Show current context name (like `git branch`)
                    use farm::context;
                    match context::find_workspace() {
                        Ok(workspace) => {
                            println!("{}", context::current_context_name(&workspace));
                            Ok(())
                        }
                        Err(_) => {
                            eprintln!("No workspace found. Initialize with: farm ctx init --name <name>");
                            std::process::exit(1);
                        }
                    }
                }
            }
        }
        Some(Commands::Replay { build_dir, dry_run, farmfile }) => {
            handle_replay_command(&build_dir, dry_run, farmfile)
        }
        Some(Commands::Cache { command }) => {
            handle_cache_command(command)
        }
        Some(Commands::Version) => {
            if std::io::IsTerminal::is_terminal(&std::io::stderr()) {
                print_version_banner();
            } else {
                // Non-interactive: emit a plain, parseable version line on stdout.
                println!("farm {}", version_string());
            }
            Ok(())
        }
        None => {
            // No subcommand: default to `plan` (like running `farm plan`)
            let explicitly_set = args.farmfile != Path::new("Farmfile");
            let farmfile = resolve_farmfile(args.farmfile, explicitly_set);
            handle_plan_command(farmfile, None, "tree".to_string(), false)
        }
    }
}

