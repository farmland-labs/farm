//! SPDX-License-Identifier: MIT OR Apache-2.0

//! Shell completion: `farm completions <shell>` and dynamic goal/operation names.
//!
//! The registration script printed by `farm completions <shell>` calls back into
//! `farm` (with `FARM_COMPLETE=<shell>` set) on every <TAB>, so goal and operation
//! names are read from the Farmfile in the current directory at completion time,
//! with their `help:` text as description (zsh, fish, powershell show it).
//!
//! ```text
//! bash:        echo 'source <(farm completions bash)' >> ~/.bashrc
//! zsh:         echo 'source <(farm completions zsh)'  >> ~/.zshrc
//! fish:        echo 'farm completions fish | source'  >> ~/.config/fish/config.fish
//! powershell:  farm completions powershell | Out-String | Invoke-Expression   # in $PROFILE
//! ```

use std::io::Write;
use std::path::PathBuf;

use clap::CommandFactory;
use clap_complete::env::Shells;
use clap_complete::{CompleteEnv, CompletionCandidate};

use crate::cli::{resolve_farmfile, Args};

/// Env var that switches `farm` into completion mode (set by the registration script).
const COMPLETE_VAR: &str = "FARM_COMPLETE";

/// Answer a pending completion request and exit; no-op on a normal run.
///
/// Must run before anything is written to stdout.
pub(crate) fn complete_if_requested() {
    CompleteEnv::with_factory(Args::command)
        .var(COMPLETE_VAR)
        .complete();
}

/// Shell names accepted by `farm completions`.
pub(crate) fn shell_names() -> Vec<&'static str> {
    Shells::builtins().names().collect()
}

/// `farm completions <shell>`: print the registration script for `shell`.
pub(crate) fn handle_completions_command(shell: &str) -> Result<(), Box<dyn std::error::Error>> {
    let shells = Shells::builtins();
    let completer = shells
        .completer(shell)
        .ok_or_else(|| format!("Unsupported shell '{}'. Use one of: {}", shell, shell_names().join(", ")))?;

    let mut out = Vec::new();
    // `farm` resolved via PATH, so the script keeps working across upgrades.
    completer.write_registration(COMPLETE_VAR, "farm", "farm", "farm", &mut out)?;
    std::io::stdout().write_all(&out)?;
    Ok(())
}

/// Goal and operation names of the Farmfile in the current directory
/// (`FARM_FARMFILE` respected). Empty if there is none or it does not parse.
///
/// TODO: honor `-f/--farmfile` from the command line being completed; the
///       candidates API only sees the current word.
pub(crate) fn target_candidates() -> Vec<CompletionCandidate> {
    let farmfile = resolve_farmfile(PathBuf::from("Farmfile"), false);
    let Ok(content) = std::fs::read_to_string(&farmfile) else {
        return Vec::new();
    };
    let Ok(plan) = farm::vendor::parse::parse_plan(&content) else {
        return Vec::new();
    };

    let help_of = |label: &str| {
        plan.operations
            .iter()
            .find(|op| op.label == label)
            .and_then(|op| op.help.clone())
    };

    let mut goals: Vec<_> = plan.goals.iter().collect();
    goals.sort();
    let goal_candidates = goals.into_iter().map(|(goal, operation)| {
        let help = help_of(operation).unwrap_or_else(|| format!("→ {operation}"));
        CompletionCandidate::new(goal)
            .help(Some(help.into()))
            .tag(Some("goals".into()))
            .display_order(Some(0))
    });

    let operation_candidates = plan.operations.iter().map(|op| {
        CompletionCandidate::new(&op.label)
            .help(op.help.clone().map(Into::into))
            .tag(Some("operations".into()))
            .display_order(Some(1))
    });

    goal_candidates.chain(operation_candidates).collect()
}

/// How many recent runs completion offers.
const RUN_CANDIDATES: usize = 20;

/// Recent runs as their short ID (see `farm log --list`), described by goal,
/// status, start time and duration, newest first. Falls back to the full build
/// ID where two runs share a short ID.
pub(crate) fn run_candidates() -> Vec<CompletionCandidate> {
    let Ok(farm_dir) = crate::cmd::log::farm_dir() else {
        return Vec::new();
    };
    let runs = farm::runs::list_runs(&farm_dir);
    let recent = &runs[..runs.len().min(RUN_CANDIDATES)];

    recent
        .iter()
        .enumerate()
        .map(|(i, run)| {
            let short = run.short_id();
            let unique = runs.iter().filter(|r| r.build_id.ends_with(short)).count() == 1;
            let value = if unique { short } else { run.build_id.as_str() };
            let help = format!(
                "{} {} {} {}",
                run.goal().unwrap_or("?"),
                crate::cmd::log::status_word(run),
                crate::cmd::log::started_local(run),
                crate::cmd::log::duration(run),
            );
            CompletionCandidate::new(value)
                .help(Some(help.into()))
                .tag(Some("runs".into()))
                // Keep newest-first order rather than the shell's alphabetical sort.
                .display_order(Some(2 + i))
        })
        .collect()
}

/// `farm log RUN`: goals and operations, then recent runs.
pub(crate) fn log_run_candidates() -> Vec<CompletionCandidate> {
    let mut candidates = target_candidates();
    candidates.extend(run_candidates());
    candidates
}
