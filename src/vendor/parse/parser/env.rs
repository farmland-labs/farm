//! SPDX-License-Identifier: MIT OR Apache-2.0

//! `[env]` section parser for Farmland configuration files.
//!
//! The `[env]` section declares environment variables that are injected
//! into the command environment of **every** operation. Values are literal
//! (everything after the first `=`), so they may contain `:`, `=` or `#`:
//!
//! ```text
//! [env]
//! NODE_ENV=production
//! API_URL=https://api.example.com
//! GREETING=hello world
//! ```
//!
//! Only whole-line `#` / `//` comments are recognised; `#` inside a value is
//! kept verbatim.

use nom::{
    IResult,
    bytes::complete::{tag, take_till, take_while1},
    character::complete::{char, line_ending, space0},
    sequence::{delimited, terminated},
    Parser,
};
use std::collections::HashMap;

/// Parse a complete `[env]` section into a map of `KEY -> value`.
pub fn env_section(input: &str) -> IResult<&str, HashMap<String, String>> {
    use nom::error::Error;
    use super::general::comment_line;

    // Parse the `[env]` header.
    let mut header = delimited(
        char('['),
        tag("env"),
        terminated(char(']'), line_ending::<&str, Error<&str>>),
    );
    let (mut remaining, _) = header.parse(input)?;

    let mut vars = HashMap::new();

    loop {
        // Skip blank lines and whole-line comments.
        loop {
            if let Ok((new_input, _)) = line_ending::<&str, Error<&str>>(remaining) {
                remaining = new_input;
                continue;
            }
            if let Ok((new_input, _)) = comment_line(remaining) {
                remaining = new_input;
                continue;
            }
            break;
        }

        // Stop at EOF or the start of the next section.
        if remaining.is_empty() || remaining.starts_with('[') {
            break;
        }

        if let Ok((new_input, (key, value))) = env_assignment(remaining) {
            vars.insert(key, value);
            remaining = new_input;
            continue;
        }

        break;
    }

    Ok((remaining, vars))
}

/// Parse a single `KEY=value` assignment line. The key is alphanumeric plus
/// `_`; the value is the rest of the line (trailing whitespace trimmed) and is
/// taken verbatim, so it may contain `=`, `:` or `#`.
fn env_assignment(input: &str) -> IResult<&str, (String, String)> {
    use nom::error::Error;

    let (input, _) = space0(input)?;
    let is_key_char = |c: char| c.is_alphanumeric() || c == '_';
    let (input, key) = take_while1(is_key_char)(input)?;
    let (input, _) = space0(input)?;
    let (input, _) = char('=')(input)?;
    let (input, _) = space0(input)?;
    let (input, value) = take_till(|c| c == '\n' || c == '\r')(input)?;
    let (input, _) = line_ending::<&str, Error<&str>>(input)?;

    Ok((input, (key.to_string(), value.trim_end().to_string())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_env_section_basic() {
        let input = "[env]\nNODE_ENV=production\nRUST_LOG=debug\n";
        let (remaining, vars) = env_section(input).unwrap();
        assert!(remaining.is_empty());
        assert_eq!(vars.get("NODE_ENV"), Some(&"production".to_string()));
        assert_eq!(vars.get("RUST_LOG"), Some(&"debug".to_string()));
    }

    #[test]
    fn test_env_section_value_with_spaces() {
        let input = "[env]\nGREETING=hello world\n";
        let (_remaining, vars) = env_section(input).unwrap();
        assert_eq!(vars.get("GREETING"), Some(&"hello world".to_string()));
    }

    #[test]
    fn test_env_section_value_with_equals_and_colon() {
        let input = "[env]\nURL=https://x.io/a?b=c\nTOKEN=aGVsbG8=\n";
        let (_remaining, vars) = env_section(input).unwrap();
        assert_eq!(vars.get("URL"), Some(&"https://x.io/a?b=c".to_string()));
        assert_eq!(vars.get("TOKEN"), Some(&"aGVsbG8=".to_string()));
    }

    #[test]
    fn test_env_section_hash_in_value_is_kept() {
        let input = "[env]\nCOLOR=#ff0000\n";
        let (_remaining, vars) = env_section(input).unwrap();
        assert_eq!(vars.get("COLOR"), Some(&"#ff0000".to_string()));
    }

    #[test]
    fn test_env_section_with_comments_and_blanks() {
        let input = "[env]\n# a comment\nA=1\n\n// another\nB=2\n";
        let (_remaining, vars) = env_section(input).unwrap();
        assert_eq!(vars.get("A"), Some(&"1".to_string()));
        assert_eq!(vars.get("B"), Some(&"2".to_string()));
        assert_eq!(vars.len(), 2);
    }

    #[test]
    fn test_env_section_empty_value() {
        let input = "[env]\nEMPTY=\n";
        let (_remaining, vars) = env_section(input).unwrap();
        assert_eq!(vars.get("EMPTY"), Some(&"".to_string()));
    }

    #[test]
    fn test_env_section_stops_at_next_section() {
        let input = "[env]\nA=1\n[operation.build]\n";
        let (remaining, vars) = env_section(input).unwrap();
        assert_eq!(remaining, "[operation.build]\n");
        assert_eq!(vars.get("A"), Some(&"1".to_string()));
    }
}
