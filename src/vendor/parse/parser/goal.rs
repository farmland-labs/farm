//! SPDX-License-Identifier: MIT OR Apache-2.0

//! Goal section parser for Farmland configuration files
//!
//! Goals define user-facing entry points that map to internal operations.
//! They decouple the public API from the internal dependency graph.

use nom::{
    IResult,
    bytes::complete::{tag, take_while1},
    character::complete::{char, line_ending, space0},
    sequence::{delimited, terminated},
    multi::many0,
    combinator::map,
    branch::alt,
    Parser,
};
use std::collections::HashMap;

/// `goal_section` parses a complete goal mapping section.
/// 
/// Goals map user-friendly names to internal operation labels.
/// 
/// # Examples
/// ```text
/// [goal]
/// build: build_website
/// test: run_all_tests
/// deploy: deploy_production
/// ```
pub fn goal_section(input: &str) -> IResult<&str, HashMap<String, String>> {
    use nom::error::Error;
    use super::general::comment_line;

    // Parse [goal] header
    let mut goal_section_header = delimited(
        char('['),
        tag("goal"),
        terminated(char(']'), line_ending::<&str, Error<&str>>)
    );

    // Valid identifier characters for goal names and operation labels
    let _is_valid_identifier = |c: char| c.is_alphanumeric() || c == '_' || c == '-';

    // Helper to skip empty lines and comments
    let mut skip_empty_and_comments = many0(alt((
        map(line_ending::<&str, Error<&str>>, |_| ()),
        map(comment_line, |_| ())
    )));

    // Parse the header
    let (mut input, _) = goal_section_header.parse(input)?;

    let mut goals = HashMap::new();

    loop {
        // Skip empty lines and comments
        let (new_input, _) = skip_empty_and_comments.parse(input)?;
        input = new_input;

        // Try to parse a goal line: `goal_name: operation_label`
        let goal_line_result: IResult<&str, (&str, &str)> = goal_line(input);

        if let Ok((new_input, (goal_name, operation_label))) = goal_line_result {
            goals.insert(goal_name.to_string(), operation_label.to_string());
            input = new_input;
        } else {
            // No more goal lines, break
            break;
        }
    }

    if goals.is_empty() {
        return Err(nom::Err::Error(nom::error::Error::new(input, nom::error::ErrorKind::Many1)));
    }

    Ok((input, goals))
}

/// Parse a single goal line: `goal_name: operation_label`
fn goal_line(input: &str) -> IResult<&str, (&str, &str)> {
    use nom::error::Error;
    
    let is_valid_identifier = |c: char| c.is_alphanumeric() || c == '_' || c == '-';
    
    let (input, goal_name) = take_while1(is_valid_identifier).parse(input)?;
    let (input, _) = char(':').parse(input)?;
    let (input, _) = space0.parse(input)?;
    let (input, operation_label) = take_while1(is_valid_identifier).parse(input)?;
    let (input, _) = space0.parse(input)?;
    let (input, _) = line_ending::<&str, Error<&str>>.parse(input)?;
    
    Ok((input, (goal_name, operation_label)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_goal_section_single() {
        let input = "[goal]\nbuild: build_website\n";
        let (remaining, goals) = goal_section(input).unwrap();
        assert_eq!(remaining, "");
        assert_eq!(goals.len(), 1);
        assert_eq!(goals.get("build"), Some(&"build_website".to_string()));
    }

    #[test]
    fn test_goal_section_multiple() {
        let input = "[goal]\nbuild: build_website\ntest: run_tests\ndeploy: deploy_prod\n";
        let (remaining, goals) = goal_section(input).unwrap();
        assert_eq!(remaining, "");
        assert_eq!(goals.len(), 3);
        assert_eq!(goals.get("build"), Some(&"build_website".to_string()));
        assert_eq!(goals.get("test"), Some(&"run_tests".to_string()));
        assert_eq!(goals.get("deploy"), Some(&"deploy_prod".to_string()));
    }

    #[test]
    fn test_goal_section_with_comments() {
        let input = "[goal]\n# Main build goal\nbuild: build_website\n# Testing goal\ntest: run_tests\n";
        let (remaining, goals) = goal_section(input).unwrap();
        assert_eq!(remaining, "");
        assert_eq!(goals.len(), 2);
        assert_eq!(goals.get("build"), Some(&"build_website".to_string()));
        assert_eq!(goals.get("test"), Some(&"run_tests".to_string()));
    }

    #[test]
    fn test_goal_section_with_empty_lines() {
        let input = "[goal]\nbuild: build_website\n\ntest: run_tests\n";
        let (remaining, goals) = goal_section(input).unwrap();
        assert_eq!(remaining, "");
        assert_eq!(goals.len(), 2);
    }

    #[test]
    fn test_goal_section_with_hyphens_underscores() {
        let input = "[goal]\nbuild-all: build_all_targets\nrun_tests: test-suite\n";
        let (remaining, goals) = goal_section(input).unwrap();
        assert_eq!(remaining, "");
        assert_eq!(goals.get("build-all"), Some(&"build_all_targets".to_string()));
        assert_eq!(goals.get("run_tests"), Some(&"test-suite".to_string()));
    }

    #[test]
    fn test_goal_section_empty_error() {
        let input = "[goal]\n";
        assert!(goal_section(input).is_err(), "Should fail with empty goal list");
    }

    #[test]
    fn test_goal_section_invalid_header() {
        let input = "[goals]\nbuild: build_website\n";
        assert!(goal_section(input).is_err(), "Should fail with wrong header name");
    }

    #[test]
    fn test_goal_section_leaves_next_section() {
        let input = "[goal]\nbuild: build_website\n[operation.test]\nwork: cargo test\n";
        let (remaining, goals) = goal_section(input).unwrap();
        assert_eq!(remaining, "[operation.test]\nwork: cargo test\n");
        assert_eq!(goals.len(), 1);
        assert_eq!(goals.get("build"), Some(&"build_website".to_string()));
    }
}
