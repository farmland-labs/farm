//! SPDX-License-Identifier: MIT OR Apache-2.0

//! Log normalization for reading and diffing runs (ADR 0001).
//!
//! Log *files* are written byte-faithful — normalization is strictly read-side.
//! Two runs of the same unchanged work produce logs that differ in every
//! `[farm]` framing line (timestamps, durations) and in every progress-bar
//! redraw, so a raw diff opens with guaranteed noise the reader has to scroll
//! past before reaching anything real. This module removes exactly that noise
//! and nothing else.
//!
//! What is deliberately *kept*: `exit_code`, `success`, `stage`, `variant` and
//! `cmd`. Those change only when something meaningful changed.

/// What to strip when rendering a log.
#[derive(Debug, Clone, Default)]
pub struct NormalizeOptions {
    /// Drop `[farm]` framing lines entirely, leaving only task output.
    pub only_output: bool,
    /// Build IDs to replace with a stable placeholder, so paths containing a
    /// run directory do not differ purely because the runs differ.
    pub run_ids: Vec<String>,
}

/// Placeholder substituted for a run's build ID.
const RUN_PLACEHOLDER: &str = "{run}";

/// Framing fields that change on every run regardless of what happened.
const VOLATILE_FIELDS: [&str; 3] = ["timestamp_start=", "timestamp_end=", "duration_ms="];

/// Normalize a log for display or comparison.
pub fn normalize(content: &str, options: &NormalizeOptions) -> String {
    let mut out = String::with_capacity(content.len());

    for line in content.lines() {
        let line = collapse_carriage_returns(line);

        if is_framing(line) {
            if options.only_output {
                continue;
            }
            out.push_str(&strip_volatile_fields(line));
        } else {
            out.push_str(line);
        }

        out.push('\n');
    }

    for id in &options.run_ids {
        if !id.is_empty() {
            out = out.replace(id.as_str(), RUN_PLACEHOLDER);
        }
    }

    out
}

/// True for lines farm emitted itself rather than the task.
fn is_framing(line: &str) -> bool {
    line.starts_with("[farm] ")
}

/// Apply terminal carriage-return semantics: `\r` returns to column zero, so
/// only the text after the last one was ever visible.
///
/// Progress bars redraw by writing frame after frame separated by `\r` with no
/// newline until they finish, which lands in the log as one very long line whose
/// intermediate frames differ on every run. Collapsing here — rather than when
/// writing the file — keeps the log itself faithful and reversible.
fn collapse_carriage_returns(line: &str) -> &str {
    match line.rfind('\r') {
        Some(pos) => &line[pos + 1..],
        None => line,
    }
}

/// Remove volatile `key=value` tokens from a framing line.
///
/// `cmd=` lines are skipped wholesale: their value is an arbitrary shell command
/// that may itself contain something looking like `duration_ms=`, and mangling a
/// command line would be worse than leaving it alone. Nothing volatile is
/// emitted on those lines anyway.
fn strip_volatile_fields(line: &str) -> String {
    if line.starts_with("[farm] cmd=") {
        return line.to_string();
    }

    let kept: Vec<&str> = line
        .split_whitespace()
        .filter(|token| !VOLATILE_FIELDS.iter().any(|field| token.starts_with(field)))
        .collect();

    kept.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_timestamps_and_durations_but_keeps_outcome() {
        let log = "[farm] stage=test variant=default timestamp_start=2026-08-03T21:28:38.671Z\n\
                   [farm] exit_code=0 success=true duration_ms=7 timestamp_end=2026-08-03T21:28:38.679Z\n";

        let out = normalize(log, &NormalizeOptions::default());

        assert_eq!(
            out,
            "[farm] stage=test variant=default\n[farm] exit_code=0 success=true\n"
        );
    }

    /// The whole point: two runs of identical work must normalize identically.
    #[test]
    fn identical_work_normalizes_identically() {
        let first = "[farm] stage=test variant=default timestamp_start=2026-08-03T21:00:00.000Z\n\
                     hello\n\
                     [farm] exit_code=0 success=true duration_ms=7 timestamp_end=2026-08-03T21:00:00.007Z\n";
        let second = "[farm] stage=test variant=default timestamp_start=2026-08-04T09:15:22.500Z\n\
                      hello\n\
                      [farm] exit_code=0 success=true duration_ms=912 timestamp_end=2026-08-04T09:15:23.412Z\n";

        let options = NormalizeOptions::default();
        assert_eq!(normalize(first, &options), normalize(second, &options));
    }

    /// A real difference must survive normalization.
    #[test]
    fn genuine_differences_survive() {
        let ok = "[farm] exit_code=0 success=true duration_ms=7 timestamp_end=2026-08-03T21:00:00Z\n";
        let failed = "[farm] exit_code=1 success=false duration_ms=9 timestamp_end=2026-08-03T21:00:00Z\n";

        let options = NormalizeOptions::default();
        assert_ne!(normalize(ok, &options), normalize(failed, &options));
    }

    #[test]
    fn collapses_progress_bar_redraws_to_final_frame() {
        let log = "Building [=>    ] 1/50\rBuilding [====> ] 25/50\rBuilding [======] 50/50\n";

        let out = normalize(log, &NormalizeOptions::default());

        assert_eq!(out, "Building [======] 50/50\n");
    }

    /// A command containing something that looks like a volatile field must not
    /// be mangled.
    #[test]
    fn cmd_lines_are_left_intact() {
        let log = "[farm] cmd=/bin/sh -c echo duration_ms=1 timestamp_end=now\n";

        let out = normalize(log, &NormalizeOptions::default());

        assert_eq!(out, log);
    }

    #[test]
    fn run_ids_are_replaced_with_a_placeholder() {
        let log = "wrote /w/.farm/run/test-20260803T210939.361Z-761e2f/out/thing\n";
        let options = NormalizeOptions {
            run_ids: vec!["test-20260803T210939.361Z-761e2f".to_string()],
            ..Default::default()
        };

        let out = normalize(log, &options);

        assert_eq!(out, "wrote /w/.farm/run/{run}/out/thing\n");
    }

    #[test]
    fn only_output_drops_framing() {
        let log = "[farm] stage=test variant=default timestamp_start=2026-08-03T21:00:00Z\n\
                   real output\n\
                   [farm] exit_code=0 success=true duration_ms=7 timestamp_end=2026-08-03T21:00:00Z\n";

        let out = normalize(
            log,
            &NormalizeOptions { only_output: true, ..Default::default() },
        );

        assert_eq!(out, "real output\n");
    }

    /// Task output that merely mentions `[farm]` mid-line is not framing.
    #[test]
    fn only_leading_farm_marker_counts_as_framing() {
        let log = "echo '[farm] not really framing'\n";

        let out = normalize(
            log,
            &NormalizeOptions { only_output: true, ..Default::default() },
        );

        assert_eq!(out, log);
    }
}
