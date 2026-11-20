//! SPDX-License-Identifier: MIT OR Apache-2.0

//! Include / exclude semantics of `input:` and `output:` patterns.
//!
//! A pattern prefixed with `!` is an exclude. A file is selected when it matches
//! at least one include pattern and no exclude pattern. Order does not matter
//! (unlike `.gitignore`, a later include cannot re-include an excluded file).
//!
//! ```text
//! // nothing under firmware/ is hashed
//! input:
//!     **/*.rs
//!     !firmware/**
//! ```
//!
//! The parser keeps patterns as written (with the `!`), so declaration order and
//! the work hash stay stable; consumers split them with [`split_patterns`].

/// Prefix marking an exclude pattern.
pub const EXCLUDE_PREFIX: char = '!';

/// Patterns of one `input:` / `output:` list, split into includes and excludes
/// (excludes without their `!` prefix).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SplitPatterns<'a> {
    pub includes: Vec<&'a str>,
    pub excludes: Vec<&'a str>,
}

/// Split patterns into includes and excludes, preserving declaration order.
pub fn split_patterns(patterns: &[String]) -> SplitPatterns<'_> {
    let mut split = SplitPatterns::default();
    for pattern in patterns {
        match pattern.strip_prefix(EXCLUDE_PREFIX) {
            Some(exclude) => split.excludes.push(exclude),
            None => split.includes.push(pattern),
        }
    }
    split
}

/// `true` if the pattern is an exclude (`!`-prefixed).
pub fn is_exclude(pattern: &str) -> bool {
    pattern.starts_with(EXCLUDE_PREFIX)
}

/// Validate one pattern list; `field` is the Farmfile field name for messages.
///
/// A list with excludes but no includes selects nothing, which is always a mistake.
pub fn validate_patterns(field: &str, patterns: &[String]) -> Result<(), String> {
    let split = split_patterns(patterns);
    if split.includes.is_empty() && !split.excludes.is_empty() {
        return Err(format!(
            "'{field}' has only exclude patterns ({}); add at least one include pattern",
            patterns.join(", ")
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|p| p.to_string()).collect()
    }

    #[test]
    fn test_split_patterns() {
        let patterns = s(&["src/**", "!src/gen/**", "Cargo.toml", "!**/*.bak"]);
        let split = split_patterns(&patterns);
        assert_eq!(split.includes, vec!["src/**", "Cargo.toml"]);
        assert_eq!(split.excludes, vec!["src/gen/**", "**/*.bak"]);
    }

    #[test]
    fn test_validate_patterns() {
        assert!(validate_patterns("input", &s(&[])).is_ok());
        assert!(validate_patterns("input", &s(&["src/**"])).is_ok());
        assert!(validate_patterns("input", &s(&["src/**", "!src/gen/**"])).is_ok());

        let err = validate_patterns("output", &s(&["!dist/**"])).unwrap_err();
        assert!(err.contains("'output' has only exclude patterns"), "{err}");
    }
}
