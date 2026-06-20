//! SPDX-License-Identifier: MIT OR Apache-2.0

//! `farm ctx` / `farm ctx run` subcommands.

use clap::Subcommand;
use std::path::PathBuf;

use crate::cmd::branch::{
    CtxBranchCommands, handle_branch_clean, handle_branch_delete, handle_branch_list,
    handle_branch_show, handle_branch_use,
};

/// Commands for context management
#[derive(Subcommand, Clone)]
pub(crate) enum CtxCommands {
    /// Shortcut: same as `farm ctx run init` (auto-creates the branch
    /// context if missing). One-shot setup after a `git checkout`.
    Init {
        /// Build ID. When omitted, a fresh UUIDv4 is generated.
        #[arg(long, short = 'b')]
        build_id: Option<String>,

        /// Context name. Default order: FARM_CTX env → `.farm/ctx/current`
        /// → current git branch (sanitised) → `_main`.
        #[arg(long, short = 'c')]
        ctx: Option<String>,

        /// Force the *live* git branch as the context (auto-created if
        /// missing). Bypasses FARM_CTX and `.farm/ctx/current`. Use this
        /// to always get a fresh run scoped to the actual branch you're
        /// on. Conflicts with `--ctx`. Errors if the workspace isn't a
        /// git checkout.
        #[arg(long, short = 'B', conflicts_with = "ctx")]
        branch: bool,

        /// Workspace directory (defaults to current directory)
        #[arg(long, short = 'w')]
        workspace: Option<PathBuf>,

        /// Output format: human (default), json
        #[arg(long, short = 'f', default_value = "human")]
        format: String,
    },

    /// Manage branch contexts (init / list / use / show / delete / clean).
    /// All branch-context operations live here; there are no top-level
    /// `farm ctx list/use/show/delete` aliases.
    Branch {
        #[command(subcommand)]
        command: CtxBranchCommands,
    },

    /// Export environment variables for current context
    Env {
        /// Context name (defaults to current context)
        name: Option<String>,
        
        /// Output format: export (default), json, fish
        #[arg(long, short = 'f', default_value = "export")]
        format: String,
    },
    
    /// Sync VCS hint in current context metadata
    Sync {
        /// What to sync (vcs)
        what: String,
    },
    
    /// Manage pipeline runs within context
    Run {
        #[command(subcommand)]
        command: CtxRunCommands,
    },
}

/// Commands for managing pipeline runs
#[derive(Subcommand, Clone)]
pub(crate) enum CtxRunCommands {
    /// Initialize a pipeline run directory
    ///
    /// Creates .farm/run/{build_id}/ with in/ and out/ directories.
    /// Also creates .farm/ctx/{ctx}/mnt/ for shared parcel mounting.
    /// Outputs JSON with paths for the caller.
    #[command(after_help = "\
EXAMPLES:
    farm ctx run init
    farm ctx run init --build-id abc123 --ctx ci
    farm ctx run init --build-id abc123 --format json | jq .out_dir")]
    Init {
        /// Build ID. When omitted, a fresh UUIDv4 is generated.
        #[arg(long, short = 'b')]
        build_id: Option<String>,

        /// Context name. Default order: FARM_CTX env → `.farm/ctx/current`
        /// → current git branch (sanitised) → `_main`.
        #[arg(long, short = 'c')]
        ctx: Option<String>,

        /// Force the *live* git branch as the context (auto-created if
        /// missing). Bypasses FARM_CTX and `.farm/ctx/current`. Use this
        /// to always get a fresh run scoped to the actual branch you're
        /// on. Conflicts with `--ctx`. Errors if the workspace isn't a
        /// git checkout.
        #[arg(long, short = 'B', conflicts_with = "ctx")]
        branch: bool,

        /// Workspace directory (defaults to current directory)
        #[arg(long, short = 'w')]
        workspace: Option<PathBuf>,

        /// Output format: human (default), json
        #[arg(long, short = 'f', default_value = "human")]
        format: String,
    },

    /// Show run paths for a build ID
    Show {
        /// Build ID
        #[arg(long, short = 'b')]
        build_id: String,
        
        /// Context name (default: from FARM_CTX env or 'main')
        #[arg(long, short = 'c')]
        ctx: Option<String>,
        
        /// Workspace directory (defaults to current directory)
        #[arg(long, short = 'w')]
        workspace: Option<PathBuf>,
        
        /// Output format: human (default), json
        #[arg(long, short = 'f', default_value = "human")]
        format: String,
    },
    
    /// List all runs in the workspace
    List {
        /// Workspace directory (defaults to current directory)
        #[arg(long, short = 'w')]
        workspace: Option<PathBuf>,
        
        /// Show detailed info
        #[arg(long, short = 'l')]
        long: bool,
    },
    
    /// Delete a single run directory (`.farm/run/{build_id}/`).
    Delete {
        /// Build ID of the run to delete.
        #[arg(long, short = 'b')]
        build_id: String,

        /// Workspace directory (defaults to current directory)
        #[arg(long, short = 'w')]
        workspace: Option<PathBuf>,

        /// Skip confirmation prompt
        #[arg(long, short = 'y')]
        yes: bool,
    },

    /// Clean up old run directories
    Clean {
        /// Workspace directory (defaults to current directory)
        #[arg(long, short = 'w')]
        workspace: Option<PathBuf>,

        /// Keep this many recent runs (default: 10)
        #[arg(long, short = 'k', default_value = "10")]
        keep: usize,

        /// Skip confirmation prompt
        #[arg(long, short = 'y')]
        yes: bool,
    }

}

/// Shared implementation for `farm ctx init` (the shortcut) and
/// `farm ctx run init`. Auto-generates a UUID build_id when none is
/// supplied, resolves the context name, and prints in human or json
/// form. The branch context is auto-created by `init_run` when missing.
///
/// Resolution order for the context name:
/// - `branch == true` (`-B/--branch`) — read the *live* git branch
///   directly. Errors if the workspace isn't a git checkout. Bypasses
///   FARM_CTX and `.farm/ctx/current`.
/// - `ctx == Some(_)` — sanitised user input.
/// - otherwise — `current_context_name`: FARM_CTX env → ctx/current
///   file → live git branch → `_main`.
pub(crate) fn do_run_init(
    build_id: Option<String>,
    ctx: Option<String>,
    branch: bool,
    workspace: Option<PathBuf>,
    format: &str,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    use farm::context;
    let workspace = workspace.unwrap_or_else(|| std::env::current_dir().unwrap());
    let build_id = build_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let ctx_name = if branch {
        let raw = context::current_git_branch(&workspace).ok_or_else(|| {
            format!(
                "-B/--branch requires a git checkout, but no branch was detected at {}",
                workspace.display()
            )
        })?;
        context::sanitize_branch_name(&raw)
            .map_err(|e| format!("git branch {:?} is not a valid context name: {}", raw, e))?
    } else {
        match ctx {
            Some(raw) => context::sanitize_branch_name(&raw)
                .map_err(|e| format!("invalid --ctx value: {}", e))?,
            None => context::current_context_name(&workspace),
        }
    };

    let paths = context::init_run(&workspace, &build_id, &ctx_name)?;

    match format {
        "json" => println!("{}", serde_json::to_string_pretty(&paths)?),
        _ => {
            println!("✅ Initialized pipeline run");
            if paths.auto_initialized {
                println!("   (auto-initialised branch context: {})", paths.ctx_name);
            }
            println!();
            println!("   Build ID:  {}", paths.build_id);
            println!("   Context:   {}", paths.ctx_name);
            println!();
            println!("   Run dir:   {}", paths.run_dir.display());
            println!("   In dir:    {}", paths.in_dir.display());
            println!("   Out dir:   {}", paths.out_dir.display());
            println!("   Mount dir: {}", paths.mnt_dir.display());
            println!();
            println!("Environment variables:");
            println!("   export FARM_BUILD_ID={}", paths.build_id);
            println!("   export FARM_CTX={}", paths.ctx_name);
        }
    }
    Ok(())
}

pub(crate) fn handle_ctx_command(command: CtxCommands) -> Result<(), Box<dyn std::error::Error>> {
    use farm::context;

    // Shortcut: `farm ctx init` is `farm ctx run init` with all args optional.
    if let CtxCommands::Init { build_id, ctx, branch, workspace, format } = command {
        return do_run_init(build_id, ctx, branch, workspace, &format);
    }

    // `farm ctx branch ...` — branch context lifecycle. Init / Clean don't
    // require .farm to exist; Show / List / Use / Delete delegate to the
    // top-level equivalents (which require .farm).
    if let CtxCommands::Branch { command: branch_cmd } = command {
        match branch_cmd {
            CtxBranchCommands::Init { name, switch } => {
                let ws = context::workspace_dir()?;
                let resolved = match name {
                    Some(raw) => context::sanitize_branch_name(&raw)
                        .map_err(|e| format!("invalid --name: {}", e))?,
                    None => context::auto_resolve_branch_name(&ws),
                };
                let ctx = context::create_context(&ws, &resolved)?;
                println!("✅ Initialized branch context: {}", resolved);
                println!("   Directory: {}", ctx.ctx_dir.display());
                if switch {
                    context::switch_context(&ws, &resolved)?;
                    println!("   Switched to: {}", resolved);
                }
                return Ok(());
            }
            CtxBranchCommands::Show { format } => return handle_branch_show(format),
            CtxBranchCommands::List { long } => return handle_branch_list(long),
            CtxBranchCommands::Use { name } => return handle_branch_use(name),
            CtxBranchCommands::Delete { name, yes } => return handle_branch_delete(name, yes),
            CtxBranchCommands::Clean { dry_run, yes } => return handle_branch_clean(dry_run, yes),
        }
    }

    // `farm ctx run init` — same logic as the shortcut.
    if let CtxCommands::Run { command: CtxRunCommands::Init { build_id, ctx, branch, workspace, format } } = command {
        return do_run_init(build_id, ctx, branch, workspace, &format);
    }

    // All other commands require .farm to exist
    let workspace = context::find_workspace()?;
    
    match command {
        CtxCommands::Init { .. } => unreachable!(), // Handled above
        CtxCommands::Branch { .. } => unreachable!(), // Handled above
        
        CtxCommands::Env { name, format } => {
            let ctx_name = match name {
                Some(n) => context::resolve_context_name(&workspace, &n)
                    .ok_or("No git branch detected. '.' requires a git repository.")?,
                None => context::current_context_name(&workspace),
            };
            let ctx = context::ContextPaths::new(&workspace, &ctx_name);
            
            match format.as_str() {
                "fish" => {
                    println!("set -gx FARM_CTX {}", ctx_name);
                    println!("set -gx FARM_CTX_DIR {}", ctx.ctx_dir.display());
                    println!("set -gx FARM_MNT_DIR {}", ctx.mnt_dir.display());
                }
                "json" => {
                    let vars = serde_json::json!({
                        "FARM_CTX": ctx_name,
                        "FARM_CTX_DIR": ctx.ctx_dir.display().to_string(),
                        "FARM_MNT_DIR": ctx.mnt_dir.display().to_string(),
                    });
                    println!("{}", serde_json::to_string_pretty(&vars)?);
                }
                // "export" (default)
                _ => {
                    println!("export FARM_CTX={}", ctx_name);
                    println!("export FARM_CTX_DIR={}", ctx.ctx_dir.display());
                    println!("export FARM_MNT_DIR={}", ctx.mnt_dir.display());
                }
            }
        }
        
        CtxCommands::Sync { what } => {
            match what.as_str() {
                "vcs" => {
                    // Just show current VCS state (vcs_hint in goal.json is immutable)
                    if let Some(hint) = context::detect_vcs(&workspace) {
                        println!("📍 Current VCS state:");
                        println!("   Type:   {}", hint.vcs_type);
                        if let Some(ref branch) = hint.branch {
                            println!("   Branch: {}", branch);
                        }
                        if let Some(ref commit) = hint.commit {
                            println!("   Commit: {}", commit);
                        }
                    } else {
                        println!("ℹ️  No VCS detected in workspace");
                    }
                }
                _ => {
                    eprintln!("❌ Unknown sync target: '{}'", what);
                    eprintln!("   Available: vcs");
                    std::process::exit(1);
                }
            }
        }
        
        CtxCommands::Run { command } => {
            handle_ctx_run_command(command)?;
        }
    }
    
    Ok(())
}

/// Handle `farm ctx run` subcommands for managing pipeline runs
pub(crate) fn handle_ctx_run_command(command: CtxRunCommands) -> Result<(), Box<dyn std::error::Error>> {
    use farm::context;
    
    match command {
        CtxRunCommands::Init { .. } => unreachable!(), // Handled in handle_ctx_command before find_workspace
        
        CtxRunCommands::Show { build_id, ctx, workspace, format } => {
            let workspace = workspace.unwrap_or_else(|| std::env::current_dir().unwrap());
            let ctx_name = ctx.unwrap_or_else(|| context::current_context_name(&workspace));
            
            let paths = context::RunPaths::new(&workspace, &build_id, &ctx_name);
            
            if !paths.exists() {
                eprintln!("❌ Run not found: {}", build_id);
                std::process::exit(1);
            }
            
            match format.as_str() {
                "json" => {
                    println!("{}", serde_json::to_string_pretty(&paths)?);
                }
                // "human" (default)
                _ => {
                    println!("📋 Pipeline Run: {}", paths.build_id);
                    println!();
                    println!("   Context:   {}", paths.ctx_name);
                    println!("   Run dir:   {}", paths.run_dir.display());
                    println!("   In dir:    {}", paths.in_dir.display());
                    println!("   Out dir:   {}", paths.out_dir.display());
                    println!("   Mount dir: {}", paths.mnt_dir.display());
                    
                    // Show input tokens
                    if paths.in_dir.exists() {
                        if let Ok(entries) = std::fs::read_dir(&paths.in_dir) {
                            let tokens: Vec<_> = entries
                                .filter_map(|e| e.ok())
                                .filter(|e| e.path().extension().map(|x| x == "token").unwrap_or(false))
                                .collect();
                            if !tokens.is_empty() {
                                println!();
                                println!("   Input tokens: {}", tokens.len());
                                for token in tokens {
                                    let path = token.path();
                                    let name = path.file_stem()
                                        .and_then(|s| s.to_str())
                                        .unwrap_or("?");
                                    println!("     - {}", name);
                                }
                            }
                        }
                    }
                }
            }
        }
        
        CtxRunCommands::List { workspace, long: _ } => {
            let workspace = workspace.unwrap_or_else(|| std::env::current_dir().unwrap());
            let run_dir = workspace.join(".farm").join("run");
            
            if !run_dir.exists() {
                println!("No runs found.");
                return Ok(());
            }
            
            let mut runs: Vec<_> = std::fs::read_dir(&run_dir)?
                .filter_map(|e| e.ok())
                .filter(|e| e.path().is_dir())
                .collect();
            
            if runs.is_empty() {
                println!("No runs found.");
                return Ok(());
            }
            
            // Sort by modification time (newest first)
            runs.sort_by(|a, b| {
                let a_time = a.metadata().and_then(|m| m.modified()).ok();
                let b_time = b.metadata().and_then(|m| m.modified()).ok();
                b_time.cmp(&a_time)
            });
            
            println!("📋 Pipeline Runs:");
            for run in runs {
                let name = run.file_name().to_string_lossy().to_string();
                println!("   {}", name);
            }
        }
        
        CtxRunCommands::Clean { workspace, keep, yes } => {
            let workspace = workspace.unwrap_or_else(|| std::env::current_dir().unwrap());

            if !yes {
                eprintln!("⚠️  This will delete old runs, keeping the {} most recent.", keep);
                eprintln!("   Use -y to confirm, or press Ctrl+C to cancel.");
                return Err("Use -y flag to confirm cleanup".into());
            }

            let removed = context::cleanup_old_runs(&workspace, keep)?;
            println!("✅ Cleaned up {} old runs (kept {} most recent)", removed, keep);
        }

        CtxRunCommands::Delete { build_id, workspace, yes } => {
            let workspace = workspace.unwrap_or_else(|| std::env::current_dir().unwrap());
            let run_dir = workspace.join(".farm").join("run").join(&build_id);

            if !run_dir.exists() {
                return Err(format!("Run not found: {}", run_dir.display()).into());
            }
            if !yes {
                eprintln!("⚠️  This will delete run '{}' and all its data.", build_id);
                eprintln!("   {}", run_dir.display());
                eprintln!("   Use -y to confirm, or press Ctrl+C to cancel.");
                return Err("Use -y flag to confirm deletion".into());
            }

            std::fs::remove_dir_all(&run_dir)?;
            println!("✅ Deleted run: {}", build_id);
        }
    }
    
    Ok(())
}

