//! SPDX-License-Identifier: MIT OR Apache-2.0

//! `farm ctx branch` subcommands.

use clap::Subcommand;

/// Commands for managing branch contexts (per-branch `.farm/ctx/{name}/`).
/// A branch context survives across builds; new builds create per-build
/// `.farm/run/{build_id}/` dirs underneath via `farm ctx run init`.
#[derive(Subcommand, Clone)]
pub(crate) enum CtxBranchCommands {
    /// Initialize a branch context (creates `.farm/ctx/{name}/`).
    ///
    /// When `--name` is omitted: detects the current git branch, sanitises
    /// it (`/` → `_`, must match `[a-zA-Z0-9_-]+`, ≤ 64 chars). Falls back
    /// to `_main` when there is no git repo or the head is detached.
    /// Picked deliberately so the fallback can never collide with a real
    /// git branch named `main`.
    Init {
        /// Branch / context name. Auto-detect from git when omitted.
        #[arg(long, short = 'n')]
        name: Option<String>,

        /// Switch to the new context after creation
        #[arg(long, short = 's')]
        switch: bool,
    },

    /// Show the current branch context (alias of `farm ctx show`).
    Show {
        /// Output format: name, json, human (default)
        #[arg(long, short = 'f', default_value = "human")]
        format: String,
    },

    /// List all branch contexts (alias of `farm ctx list`).
    List {
        /// Show detailed info (VCS hint, last used, etc.)
        #[arg(long, short = 'l')]
        long: bool,
    },

    /// Switch the active branch context (alias of `farm ctx use`).
    Use {
        /// Branch / context name (or `.` for current git branch)
        name: String,
    },

    /// Delete a branch context and its build data (alias of `farm ctx delete`).
    Delete {
        /// Branch / context name (or `.` for current git branch)
        name: String,

        /// Skip confirmation prompt
        #[arg(long, short = 'y')]
        yes: bool,
    },

    /// Remove orphan branch contexts (no matching local git branch).
    ///
    /// A context is considered orphan when its name doesn't match any
    /// local git branch (sanitised via the same rules as `branch init`).
    /// The current context and `_main` are always preserved.
    Clean {
        /// Just list what would be removed; don't delete.
        #[arg(long)]
        dry_run: bool,

        /// Skip confirmation prompt
        #[arg(long, short = 'y')]
        yes: bool,
    },
}

/// `farm ctx branch list` — print all branch contexts with the active one
/// marked. Extracted from the previous top-level `farm ctx list` body.
pub(crate) fn handle_branch_list(long: bool) -> std::result::Result<(), Box<dyn std::error::Error>> {
    use farm::context;
    let workspace = context::find_workspace()?;
    let current = context::current_context_name(&workspace);
    let contexts = context::list_contexts(&workspace)?;

    if contexts.is_empty() {
        println!("No branch contexts found. Create one with: farm ctx branch init");
        return Ok(());
    }

    println!("📋 Branch Contexts:");
    println!();
    for (name, goal) in contexts {
        let marker = if name == current { "▶ " } else { "  " };
        if long {
            if let Some(g) = &goal {
                let vcs_info = g
                    .vcs_hint
                    .as_ref()
                    .map(|h| format!("{}:{}", h.vcs_type, h.branch.as_deref().unwrap_or("?")))
                    .unwrap_or_else(|| "none".to_string());
                let variant_str = g.variant.as_deref().unwrap_or("");
                println!(
                    "{}{} → {}:{} (vcs: {}, invoked: {})",
                    marker,
                    name,
                    g.target,
                    variant_str,
                    vcs_info,
                    g.invoked_at.format("%Y-%m-%d %H:%M")
                );
            } else {
                println!("{}{} (no goal set)", marker, name);
            }
        } else {
            println!("{}{}", marker, name);
        }
    }
    Ok(())
}

/// `farm ctx branch use` — switch the active branch context. Auto-creates
/// the context if missing (so `farm ctx branch use .` works after a
/// `git checkout`). Extracted from the previous top-level `farm ctx use`.
pub(crate) fn handle_branch_use(name: String) -> std::result::Result<(), Box<dyn std::error::Error>> {
    use farm::context;
    let workspace = context::find_workspace()?;

    let resolved = context::resolve_context_name(&workspace, &name)
        .ok_or("No git branch detected. '.' requires a git repository.")?;

    let ctx_path = context::ContextPaths::new(&workspace, &resolved);
    if !ctx_path.exists() {
        println!("📁 Creating context: {}", resolved);
        context::create_context(&workspace, &resolved)?;
    }

    let ctx = context::switch_context(&workspace, &resolved)?;
    println!("✅ Switched to context: {}", resolved);

    if let Some(goal) = context::read_goal(&ctx)? {
        if let Some((stored, current)) = context::check_vcs_mismatch(&workspace, &goal) {
            eprintln!();
            eprintln!("⚠️  VCS mismatch detected:");
            eprintln!(
                "   Goal was set on: {}:{}",
                stored.vcs_type,
                stored.branch.as_deref().unwrap_or("?")
            );
            eprintln!(
                "   Current VCS state:      {}:{}",
                current.vcs_type,
                current.branch.as_deref().unwrap_or("?")
            );
            eprintln!();
            eprintln!("   Consider creating a new context for this branch.");
        }
    }
    Ok(())
}

/// `farm ctx branch delete` — drop a single branch context after `-y`
/// confirmation. Extracted from the previous top-level `farm ctx delete`.
pub(crate) fn handle_branch_delete(
    name: String,
    yes: bool,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    use farm::context;
    let workspace = context::find_workspace()?;

    let resolved = context::resolve_context_name(&workspace, &name)
        .ok_or("No git branch detected. '.' requires a git repository.")?;
    if !yes {
        eprintln!(
            "⚠️  This will delete context '{}' and all its build data.",
            resolved
        );
        eprintln!("   Use -y to confirm, or press Ctrl+C to cancel.");
        return Err("Use -y flag to confirm deletion".into());
    }

    context::delete_context(&workspace, &resolved)?;
    println!("✅ Deleted context: {}", resolved);
    Ok(())
}

/// `farm ctx branch show` — print the current branch context's metadata
/// (goal / VCS hint / current operation). Extracted from the previous
/// top-level `farm ctx show`.
pub(crate) fn handle_branch_show(format: String) -> std::result::Result<(), Box<dyn std::error::Error>> {
    use farm::context;
    let workspace = context::find_workspace()?;
    let name = context::current_context_name(&workspace);
    let ctx = context::ContextPaths::new(&workspace, &name);

    match format.as_str() {
        "name" => println!("{}", name),
        "json" => {
            let (goal, current) = if ctx.exists() {
                (context::read_goal(&ctx)?, context::read_state(&ctx)?)
            } else {
                (None, None)
            };
            let info = serde_json::json!({
                "name": name,
                "exists": ctx.exists(),
                "ctx_dir": ctx.ctx_dir.display().to_string(),
                "goal": goal,
                "current": current,
            });
            println!("{}", serde_json::to_string_pretty(&info)?);
        }
        _ => {
            println!("📋 Current Context: {}", name);
            println!();
            if ctx.exists() {
                println!("  Directory: {}", ctx.ctx_dir.display());
                if let Some(goal) = context::read_goal(&ctx)? {
                    let variant_str = goal.variant.as_deref().unwrap_or("");
                    println!("  Goal:      {}:{}", goal.target, variant_str);
                    if let Some(ref vcs) = goal.vcs_hint {
                        println!(
                            "  VCS:       {}:{}",
                            vcs.vcs_type,
                            vcs.branch.as_deref().unwrap_or("?")
                        );
                    }
                    println!(
                        "  Invoked:   {}",
                        goal.invoked_at.format("%Y-%m-%d %H:%M:%S")
                    );
                } else {
                    println!("  Goal:      (not set)");
                }
                if let Some(current) = context::read_state(&ctx)? {
                    println!(
                        "  Operation: {} ({:?})",
                        current.operation, current.status
                    );
                }
            } else {
                println!("  (context does not exist yet, will be created on first use)");
            }
        }
    }
    Ok(())
}

/// Implementation for `farm ctx branch clean` — drop branch contexts that
/// don't match any local git branch. Always preserves the current context
/// and `_main`. Lists candidates first; deletes only with `--yes`
/// (or unconditionally with `--dry-run` set, which lists without acting).
pub(crate) fn handle_branch_clean(
    dry_run: bool,
    yes: bool,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    use farm::context;
    use std::collections::HashSet;

    let workspace = context::find_workspace()?;
    let current = context::current_context_name(&workspace);
    let contexts = context::list_contexts(&workspace)?;
    if contexts.is_empty() {
        println!("No branch contexts found.");
        return Ok(());
    }

    // Build the keep-set: every local git branch (sanitised) + _main +
    // the active context. Anything else is an orphan candidate.
    let mut keep: HashSet<String> = HashSet::new();
    keep.insert(context::FARM_DEFAULT_CTX_NAME.to_string());
    keep.insert(current.clone());
    for branch in context::list_local_git_branches(&workspace) {
        if let Ok(sanitized) = context::sanitize_branch_name(&branch) {
            keep.insert(sanitized);
        }
    }

    let orphans: Vec<String> = contexts
        .into_iter()
        .map(|(name, _)| name)
        .filter(|name| !keep.contains(name))
        .collect();

    if orphans.is_empty() {
        println!("No orphan branch contexts.");
        println!("   (kept: {} branch(es), {} = current)", keep.len(), current);
        return Ok(());
    }

    println!("🧹 Orphan branch contexts ({}):", orphans.len());
    for name in &orphans {
        println!("  - {}", name);
    }

    if dry_run {
        println!();
        println!("(dry-run: nothing deleted; rerun without --dry-run -y to apply)");
        return Ok(());
    }

    if !yes {
        println!();
        eprintln!("⚠️  Use -y to confirm deletion of the contexts listed above.");
        return Err("Use -y flag to confirm deletion".into());
    }

    let mut deleted = 0;
    for name in &orphans {
        match context::delete_context(&workspace, name) {
            Ok(()) => {
                println!("✅ Deleted: {}", name);
                deleted += 1;
            }
            Err(e) => {
                eprintln!("❌ Failed to delete {}: {}", name, e);
            }
        }
    }
    println!();
    println!("Removed {} of {} orphan(s).", deleted, orphans.len());
    Ok(())
}

