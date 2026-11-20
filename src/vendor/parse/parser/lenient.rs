//! SPDX-License-Identifier: MIT OR Apache-2.0

//! Lenient parsing: unknown content is skipped and reported, not fatal.
//!
//! For forward and backward compatibility a Farmfile that is valid in the parts
//! this parser understands must still parse. Unknown sections, unknown fields
//! and unrecognised lines are skipped (together with their indented
//! continuation lines) and reported as [`ParseWarning`]s on the [`Plan`].
//! Callers decide what to do with them (`farm` prints them, or fails in
//! `--strict` mode).
//!
//! What stays a hard error: a missing/unsupported `version:`, a *known* field
//! with an invalid value, and semantic problems (cycles, unknown dependencies,
//! duplicates, ...).
//!
//! [`Plan`]: crate::vendor::parse::Plan

use serde::{Deserialize, Serialize};

/// Content the parser did not understand and skipped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParseWarning {
    /// 1-based line number of the skipped content
    pub line: usize,
    /// What was skipped and where, e.g. `unknown field 'timeout' in operation 'build'`
    pub message: String,
}

impl std::fmt::Display for ParseWarning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {}: {} (ignored)", self.line, self.message)
    }
}

/// A skipped piece of input, before it is resolved to a line number.
/// `at` is the remaining input at the start of the skipped line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Skipped<'a> {
    pub at: &'a str,
    pub message: String,
}

impl<'a> Skipped<'a> {
    pub fn new(at: &'a str, message: impl Into<String>) -> Self {
        Self { at, message: message.into() }
    }
}

/// The first line of `input`, without its line ending.
pub(crate) fn first_line(input: &str) -> &str {
    input.lines().next().unwrap_or("").trim_end()
}

/// Short, single-line excerpt of the line at `input` for messages.
pub(crate) fn excerpt(input: &str) -> String {
    let line = first_line(input).trim();
    let mut out: String = line.chars().take(40).collect();
    if line.chars().count() > 40 {
        out.push('…');
    }
    out
}

/// Name of a `name:` field at the start of `input`, if the line looks like one.
pub(crate) fn field_name(input: &str) -> Option<&str> {
    let line = first_line(input);
    let (name, _) = line.split_once(':')?;
    let valid = !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-');
    valid.then_some(name)
}

/// `true` at EOF, a `[...]` section header or a `//!` decorator (which belongs
/// to the next operation): the points where a section body ends.
pub(crate) fn at_section_end(input: &str) -> bool {
    input.is_empty() || input.starts_with('[') || input.starts_with("//!")
}

/// Skip the line at `input` and every following indented or blank line
/// (the continuation of a multi-line field). Trailing blank lines are left in
/// place.
pub(crate) fn skip_line_and_continuation(input: &str) -> &str {
    let mut rest = after_line(input);
    loop {
        let line = first_line(rest);
        let is_continuation = rest.starts_with([' ', '\t']) && !line.trim().is_empty();
        if is_continuation {
            rest = after_line(rest);
            continue;
        }
        // Blank lines only belong to the block if more indented lines follow.
        if !rest.is_empty() && line.trim().is_empty() {
            let mut probe = after_line(rest);
            while !probe.is_empty() && first_line(probe).trim().is_empty() {
                probe = after_line(probe);
            }
            if probe.starts_with([' ', '\t']) {
                rest = probe;
                continue;
            }
        }
        return rest;
    }
}

/// Input after a blank (empty or whitespace-only) line at `input`, if it is one.
pub(crate) fn skip_blank_line(input: &str) -> Option<&str> {
    (!input.is_empty() && first_line(input).trim().is_empty()).then(|| after_line(input))
}

/// Record the unrecognised line at `input` and skip it together with its
/// indented continuation lines. `place` completes the message, e.g.
/// `in [goal] section`.
pub(crate) fn skip_unrecognised<'a>(input: &'a str, place: &str, skipped: &mut Vec<Skipped<'a>>) -> &'a str {
    let message = match field_name(input) {
        Some(field) => format!("unknown field '{field}' {place}"),
        None => format!("unrecognised line '{}' {place}", excerpt(input)),
    };
    skipped.push(Skipped::new(input, message));
    skip_line_and_continuation(input)
}

/// Skip a whole unknown section: from its header to the next section end.
pub(crate) fn skip_section(input: &str) -> &str {
    let mut rest = after_line(input);
    while !at_section_end(rest) {
        rest = after_line(rest);
    }
    rest
}

/// Input after the first line (including its line ending).
fn after_line(input: &str) -> &str {
    match input.find('\n') {
        Some(i) => &input[i + 1..],
        None => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_field_name() {
        assert_eq!(field_name("timeout: 5m\n"), Some("timeout"));
        assert_eq!(field_name("labels:\n  gpu\n"), Some("labels"));
        assert_eq!(field_name("%%% x: y\n"), None);
        assert_eq!(field_name("no colon here\n"), None);
    }

    #[test]
    fn test_skip_line_and_continuation() {
        assert_eq!(skip_line_and_continuation("x: 1\nwork: a\n"), "work: a\n");
        assert_eq!(skip_line_and_continuation("labels:\n  gpu\n\n  linux\nwork: a\n"), "work: a\n");
        // Trailing blank lines stay (they separate sections).
        assert_eq!(skip_line_and_continuation("x: 1\n  y\n\n[.next]\n"), "\n[.next]\n");
        assert_eq!(skip_line_and_continuation("last line"), "");
    }

    #[test]
    fn test_skip_section() {
        let input = "[secrets]\nTOKEN: x\n\n  indented\n//! parcel: a=b\n[.build]\n";
        assert_eq!(skip_section(input), "//! parcel: a=b\n[.build]\n");
        assert_eq!(skip_section("[future]\nfoo\n"), "");
    }

    #[test]
    fn test_excerpt() {
        assert_eq!(excerpt("  short  \nnext"), "short");
        assert_eq!(excerpt(&"x".repeat(50)), format!("{}…", "x".repeat(40)));
    }
}
