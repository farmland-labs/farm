//! SPDX-License-Identifier: MIT OR Apache-2.0

//! Loading a Farmfile: parse, report skipped content, enforce `--strict`.
//!
//! The parser is lenient for forward/backward compatibility: unknown sections,
//! fields and lines are skipped and returned as warnings. Here they are printed
//! to stderr (once per process), or, with `--strict` / `FARM_STRICT=1`, turned
//! into an error so CI and tests can insist on a clean Farmfile.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use farm::parser::ParsedPlan;

/// `--strict` / `FARM_STRICT`, set once from the parsed CLI arguments.
static STRICT: AtomicBool = AtomicBool::new(false);

/// Warnings are printed only once even if a command parses the Farmfile twice.
static REPORTED: AtomicBool = AtomicBool::new(false);

pub(crate) fn set_strict(strict: bool) {
    STRICT.store(strict, Ordering::Relaxed);
}

/// Parse `content` (read from `path`). The error is a ready-to-print message.
pub(crate) fn parse_farmfile(path: &Path, content: &str) -> Result<ParsedPlan, String> {
    let plan = farm::vendor::parse::parse_plan(content)
        .map_err(|e| format!("❌ Failed to parse config file {}:\n{}", path.display(), e))?;

    if plan.warnings.is_empty() {
        return Ok(plan);
    }

    let lines: Vec<String> = plan
        .warnings
        .iter()
        .map(|w| format!("  {}:{}: {}", path.display(), w.line, w.message))
        .collect();

    if STRICT.load(Ordering::Relaxed) {
        return Err(format!(
            "❌ {} has {} unrecognised part(s) (strict mode: --strict / FARM_STRICT):\n{}",
            path.display(),
            lines.len(),
            lines.join("\n")
        ));
    }

    if !REPORTED.swap(true, Ordering::Relaxed) {
        let prefix = if std::io::IsTerminal::is_terminal(&std::io::stderr()) && std::env::var_os("NO_COLOR").is_none() {
            "\x1b[33mwarning\x1b[0m"
        } else {
            "warning"
        };
        eprintln!(
            "{prefix}: {} has {} unrecognised part(s), ignored (use --strict to fail):",
            path.display(),
            lines.len()
        );
        for line in &lines {
            eprintln!("{line}");
        }
    }

    Ok(plan)
}
