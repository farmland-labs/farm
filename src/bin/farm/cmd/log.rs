//! SPDX-License-Identifier: MIT OR Apache-2.0

//! `farm log` command — read the logs of previous runs.
//!
//! This is the porcelain over run history (ADR 0001). The layout under `.farm/`
//! is internal and free to change, so everything users need to reach is reached
//! through here rather than by constructing paths.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use colored::Colorize;
use farm::log_view::{normalize, NormalizeOptions};
use farm::manifest::RunStatus;
use farm::runs::{self, RunEntry};

/// Everything `farm log` was invoked with.
pub(crate) struct LogArgs {
    /// Run specifier (goal, `goal@N`, `@N`, build ID or unique part of one).
    pub run: Option<String>,
    pub variant: Option<String>,
    pub list: bool,
    /// `--diff` (`Some(None)`) or `--diff RUN` (`Some(Some(run))`).
    pub diff: Option<Option<String>>,
    /// `--against N`: diff against N runs before the selected one.
    pub against: Option<usize>,
    pub follow: bool,
    pub raw: bool,
    pub only_output: bool,
    pub path: bool,
    pub limit: usize,
}

type CmdResult = Result<(), Box<dyn std::error::Error>>;

/// Handle the `farm log` command.
pub(crate) fn handle_log_command(args: LogArgs) -> CmdResult {
    let farm_dir = farm_dir()?;
    let selection = runs::select(&farm_dir, args.run.as_deref(), args.variant.as_deref())?;

    if args.list {
        return print_run_list(&selection, args.limit);
    }

    if args.diff.is_some() || args.against.is_some() {
        return print_diff(&farm_dir, &selection, &args);
    }

    let latest = selection.run();

    if args.path {
        for path in log_paths(latest) {
            println!("{}", path.display());
        }
        eprintln!(
            "{}",
            "note: this path is internal and changes between versions; do not script against it"
                .dimmed()
        );
        return Ok(());
    }

    if args.follow {
        return follow(&farm_dir, latest, &args);
    }

    print!("{}", render(latest, &args)?);
    Ok(())
}

/// `.farm/` of the current workspace.
pub(crate) fn farm_dir() -> std::io::Result<PathBuf> {
    let workspace = farm::context::find_workspace().unwrap_or(std::env::current_dir()?);
    Ok(workspace.join(".farm"))
}

/// Log files belonging to a run: the combined log, or the split pair.
fn log_paths(run: &RunEntry) -> Vec<PathBuf> {
    if let Some(combined) = run.combined_log() {
        return vec![combined];
    }

    let (Some(goal), Some(variant)) = (run.goal(), run.variant()) else {
        return Vec::new();
    };

    [
        run.log_dir().join(format!("{}_{}_stdout.log", goal, variant)),
        run.log_dir().join(format!("{}_{}_stderr.log", goal, variant)),
    ]
    .into_iter()
    .filter(|p| p.exists())
    .collect()
}

/// Read a run's log. Bytes are decoded lossily — a task that emits invalid
/// UTF-8 should still be readable rather than erroring out.
fn read_log(run: &RunEntry) -> Result<String, String> {
    let paths = log_paths(run);
    if paths.is_empty() {
        return Err(format!(
            "Run {} has no log file (it may have been started with --log-output none)",
            run.build_id
        ));
    }

    let mut content = String::new();
    for path in &paths {
        let bytes = std::fs::read(path)
            .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
        if paths.len() > 1 {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            content.push_str(&format!("[farm] --- {} ---\n", name));
        }
        content.push_str(&String::from_utf8_lossy(&bytes));
    }

    Ok(content)
}

/// Read a run's log and apply the read-side normalization, unless `--raw`.
fn render(run: &RunEntry, args: &LogArgs) -> Result<String, String> {
    let content = read_log(run)?;

    if args.raw {
        return Ok(content);
    }

    Ok(normalize(
        &content,
        &NormalizeOptions {
            only_output: args.only_output,
            run_ids: vec![run.build_id.clone()],
        },
    ))
}

fn print_run_list(selection: &runs::Selection, limit: usize) -> CmdResult {
    let scope = &selection.scope;
    // Always show the selected run, even past the limit.
    let shown = limit.max(selection.index + 1).min(scope.len());
    let index_width = format!("@{}", shown.saturating_sub(1)).len();

    for (i, run) in scope.iter().enumerate().take(shown) {
        let pointer = if i == selection.index && selection.index != 0 { "▶" } else { " " };
        println!(
            "{}{:>width$} {}",
            pointer,
            format!("@{i}").bold(),
            run_line(run),
            width = index_width
        );
    }

    if scope.len() > shown {
        println!(
            "{}",
            format!("… {} older run(s) not shown (-n to show more)", scope.len() - shown).dimmed()
        );
    }

    if std::io::IsTerminal::is_terminal(&std::io::stdout()) {
        println!(
            "{}",
            "open one with `farm log @N`, `farm log <goal>@N` or `farm log <highlighted id>`".dimmed()
        );
    }

    Ok(())
}

/// One `--list` row (without the index): status, local start time, duration,
/// tty marker and the build ID with its short ID highlighted.
fn run_line(run: &RunEntry) -> String {
    let (marker, status) = match run.status() {
        RunStatus::Ok => ("✓".green(), "ok".green()),
        RunStatus::Fail => ("✗".red(), "fail".red()),
        RunStatus::Running => ("•".yellow(), "running".yellow()),
    };
    let mode = if run.interactive() { "tty" } else { "" };
    let short = run.short_id();
    let head = &run.build_id[..run.build_id.len() - short.len()];

    format!(
        "{} {:<7} {}  {:>7} {:<3}  {}{}",
        marker,
        status,
        started_local(run),
        duration(run),
        mode.dimmed(),
        head.dimmed(),
        short.cyan().bold()
    )
}

/// Start time in local time, short form (`11-22 12:59:37`).
pub(crate) fn started_local(run: &RunEntry) -> String {
    let local: chrono::DateTime<chrono::Local> = run.started_at().into();
    local.format("%m-%d %H:%M:%S").to_string()
}

/// Run duration, or `-` while it is still running.
pub(crate) fn duration(run: &RunEntry) -> String {
    run.manifest
        .as_ref()
        .filter(|m| m.run_status() != RunStatus::Running)
        .map(|m| format_duration(m.duration_ms))
        .unwrap_or_else(|| "-".to_string())
}

/// Short status word.
pub(crate) fn status_word(run: &RunEntry) -> &'static str {
    match run.status() {
        RunStatus::Ok => "ok",
        RunStatus::Fail => "fail",
        RunStatus::Running => "running",
    }
}

/// The older side of a diff: `--diff RUN`, `--against N`, or the run before
/// the selected one. `@N` counts within the selected run's scope (its goal);
/// every other form resolves like the positional argument.
fn diff_base(farm_dir: &Path, selection: &runs::Selection, args: &LogArgs) -> Result<RunEntry, String> {
    let spec = args.diff.clone().flatten();
    let back = match (&spec, args.against) {
        (Some(_), Some(_)) => {
            return Err("--against cannot be combined with --diff RUN; use one of them".to_string());
        }
        (Some(spec), None) => {
            if let Some(n) = spec.strip_prefix('@') {
                let n: usize = n
                    .parse()
                    .map_err(|_| format!("Invalid run index '{spec}': expected @N, e.g. @1"))?;
                return selection.scope.get(n).cloned().ok_or_else(|| {
                    format!(
                        "'{spec}' is out of range: {} run(s) recorded (@0 to @{})",
                        selection.scope.len(),
                        selection.scope.len() - 1
                    )
                });
            }
            return runs::select(farm_dir, Some(spec), args.variant.as_deref())
                .map(|sel| sel.run().clone());
        }
        (None, Some(0)) => return Err("--against must be at least 1 (1 = the previous run)".to_string()),
        (None, Some(n)) => n,
        (None, None) => 1,
    };

    selection.scope.get(selection.index + back).cloned().ok_or_else(|| {
        let older = selection.scope.len() - selection.index - 1;
        format!("Only {older} earlier run(s) recorded; cannot diff against {back} back")
    })
}

fn print_diff(farm_dir: &Path, selection: &runs::Selection, args: &LogArgs) -> CmdResult {
    let newer = selection.run();
    let older = diff_base(farm_dir, selection, args)?;
    let older = &older;

    if newer.build_id == older.build_id {
        return Err(format!("Both sides of the diff are the same run ({})", newer.build_id).into());
    }

    // Normalize both against *both* build IDs, so a path mentioning either run
    // collapses to the same placeholder instead of showing up as a difference.
    let run_ids = vec![newer.build_id.clone(), older.build_id.clone()];
    let options = NormalizeOptions { only_output: args.only_output, run_ids };

    let render_one = |run: &RunEntry| -> Result<String, String> {
        let content = read_log(run)?;
        Ok(if args.raw {
            content
        } else {
            normalize(&content, &options)
        })
    };

    let old_text = render_one(older)?;
    let new_text = render_one(newer)?;

    if newer.interactive() != older.interactive() {
        eprintln!(
            "{}",
            "warning: comparing an interactive (tty) run with a non-interactive one; \
             a PTY merges stdout and stderr, so the logs are not structurally comparable"
                .yellow()
        );
    }

    println!(
        "{} {} {}",
        "---".red(),
        older.build_id.red(),
        describe(older).dimmed()
    );
    println!(
        "{} {} {}",
        "+++".green(),
        newer.build_id.green(),
        describe(newer).dimmed()
    );

    let diff = similar::TextDiff::from_lines(&old_text, &new_text);
    let mut hunks = 0;

    for hunk in diff.unified_diff().context_radius(3).iter_hunks() {
        hunks += 1;
        for change in hunk.iter_changes() {
            let value = change.value();
            let line = value.strip_suffix('\n').unwrap_or(value);
            match change.tag() {
                similar::ChangeTag::Delete => println!("{}", format!("-{}", line).red()),
                similar::ChangeTag::Insert => println!("{}", format!("+{}", line).green()),
                similar::ChangeTag::Equal => println!(" {}", line),
            }
        }
    }

    if hunks == 0 {
        println!("{}", "(no differences)".dimmed());
    }

    Ok(())
}

/// One-line description of a run for diff headers.
fn describe(run: &RunEntry) -> String {
    format!("({}, {})", status_word(run), started_local(run))
}

/// Print the log and keep printing as it grows, until the run stops running.
///
/// Polling rather than filesystem notification: a build log is appended to a few
/// times a second at most, and a quarter-second poll keeps this to a handful of
/// lines of code with no platform-specific watcher.
fn follow(farm_dir: &Path, run: &RunEntry, args: &LogArgs) -> CmdResult {
    let paths = log_paths(run);
    let Some(path) = paths.first() else {
        return Err(read_log(run).unwrap_err().into());
    };

    if paths.len() > 1 {
        eprintln!(
            "{}",
            format!("note: following {} only (split-stream run)", path.display()).dimmed()
        );
    }

    let mut offset = 0u64;
    let mut stdout = std::io::stdout();

    loop {
        let bytes = std::fs::read(path).unwrap_or_default();
        if bytes.len() as u64 > offset {
            let fresh = &bytes[offset as usize..];
            let text = String::from_utf8_lossy(fresh);
            let text = if args.raw {
                text.into_owned()
            } else {
                normalize(
                    &text,
                    &NormalizeOptions {
                        only_output: args.only_output,
                        run_ids: vec![run.build_id.clone()],
                    },
                )
            };
            print!("{}", text);
            let _ = stdout.flush();
            offset = bytes.len() as u64;
        }

        // Re-read the manifest each poll: the run writes its final one on exit,
        // which is the signal to stop.
        let still_running = runs::list_runs(farm_dir)
            .into_iter()
            .find(|r| r.build_id == run.build_id)
            .map(|r| r.status() == RunStatus::Running)
            .unwrap_or(false);

        if !still_running {
            return Ok(());
        }

        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Human-readable duration; build steps range from milliseconds to many minutes.
fn format_duration(ms: u64) -> String {
    if ms < 1000 {
        format!("{}ms", ms)
    } else if ms < 60_000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else {
        format!("{}m{}s", ms / 60_000, (ms % 60_000) / 1000)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_durations_by_magnitude() {
        assert_eq!(format_duration(7), "7ms");
        assert_eq!(format_duration(1500), "1.5s");
        assert_eq!(format_duration(125_000), "2m5s");
    }
}
