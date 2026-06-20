//! SPDX-License-Identifier: MIT OR Apache-2.0

use nom::branch::alt;
use nom::bytes::complete::tag;
use nom::character::complete::{line_ending, space0, space1};
use nom::multi::separated_list0;
use nom::sequence::{preceded, terminated};
use nom::IResult;
use nom::Parser;

use super::utils::line_until_end;

/// `version_line` parses the version tag followed by a number and a newline.
/// 
/// # Examples
/// ```text
///     version: 1 => 1
/// ```
pub fn version_line(input: &str) -> IResult<&str, u32> {
    let (input, (_, _, version, _)) = (
        tag("version:"),
        space1,
        nom::character::complete::u32,
        line_ending
    ).parse(input)?;
    Ok((input, version))
}

/// `work_single_line` parses a commandline as it would be called on the shell,
/// optionally followed by indented continuation lines that are folded into the
/// command with a single space joining each.
///
/// # Examples
/// ```text
///     work: ls -al /tmp                     =>  "ls -al /tmp"
///
///     work: docker buildx build
///           --platform linux/amd64
///           --output type=local,dest=dist
///           .                               =>  "docker buildx build --platform linux/amd64 --output type=local,dest=dist ."
/// ```
fn work_single_line(input: &str) -> IResult<&str, String> {
    let (input, (_, _, line)) = (
        tag("work:"),
        space1,
        line_until_end
    ).parse(input)?;

    let mut parts: Vec<String> = vec![line.trim_end().to_string()];
    let mut remaining = input;

    // Field names / directives that should NEVER be swallowed as a work
    // continuation, even when accidentally indented.
    const FIELD_PREFIXES: &[&str] = &[
        "work:", "after:", "input:", "output:", "env:", "variant:",
        "//!", "//", "#",
    ];

    loop {
        // Continuation requires leading whitespace.
        if !remaining.starts_with(' ') && !remaining.starts_with('\t') {
            break;
        }

        let line_end = remaining
            .find(['\n', '\r'])
            .unwrap_or(remaining.len());
        let line_content = &remaining[..line_end];

        // Pure-whitespace line ends the continuation block.
        if line_content.trim().is_empty() {
            break;
        }

        let trimmed = line_content.trim_start();
        if FIELD_PREFIXES.iter().any(|p| trimmed.starts_with(p)) {
            break;
        }

        let (new_input, _) = space1(remaining)?;
        let (new_input, cont) = line_until_end(new_input)?;
        parts.push(cont.trim().to_string());
        remaining = new_input;
    }

    if parts.len() == 1 {
        Ok((remaining, parts.into_iter().next().unwrap()))
    } else {
        Ok((remaining, parts.join(" ")))
    }
}

/// `work_single_line` parses a multiline commandline blok as it would be called on the shell.
/// 
/// # Examples
/// ```text
///     shell: ```
///     set -e
///     echo "hello"
///     ```  =>  "set -e\necho \"hello\""
/// ```
fn work_block(input: &str) -> IResult<&str, String> {
    use nom::bytes::complete::take_until;
    use nom::character::complete::space0;
    
    // Parse "work:" followed by optional spaces and triple backticks
    let (input, (_, _, _, _)) = (
        tag("work:"),
        space1,
        space0,
        tag("```")
    ).parse(input)?;
    
    // Consume the rest of the line after the opening backticks
    let (input, _) = line_ending.parse(input)?;
    
    // Parse content until we find the closing triple backticks
    let (input, content) = take_until("```").parse(input)?;
    
    // Consume the closing triple backticks and optional trailing content on that line
    let (input, _) = tag("```").parse(input)?;
    
    // Consume the rest of the line (in case there's whitespace after closing backticks)
    let (input, _) = line_ending.parse(input)?;
    
    // Trim trailing newline from content if present
    let content = content.strip_suffix('\n').unwrap_or(content);
    
    Ok((input, content.into()))
}

/// `work_line` parses either single-line or multi-line work commands.
/// First tries to parse a block format, then falls back to single-line format.
pub fn work_line(input: &str) -> IResult<&str, String> {
    alt((work_block, work_single_line)).parse(input)
}

/// `after_line` parses the dependency-operations list. Two equivalent forms
/// are accepted (identical to `input:` / `output:`):
///
/// Single-line, comma-separated:
/// ```text
///     after: a, b, c               => ["a", "b", "c"]
/// ```
///
/// Multi-line, one identifier per indented line:
/// ```text
///     after:
///       build-linux-amd64-docker
///       build-linux-arm64-docker  => ["build-linux-amd64-docker", "build-linux-arm64-docker"]
/// ```
///
/// Operation names are `[A-Za-z0-9_-]+`.
pub fn after_line(input: &str) -> IResult<&str, Vec<String>> {
    use nom::bytes::complete::take_while1;

    let (input, _) = tag("after:")(input)?;

    let identifier_char = |c: char| c.is_alphanumeric() || c == '_' || c == '-';

    // Peek: multi-line if the next non-space char is a newline.
    let trimmed = input.trim_start_matches(' ').trim_start_matches('\t');

    if trimmed.starts_with('\n') || trimmed.starts_with('\r') {
        // Multi-line: parse indented identifiers, one per line.
        let (input, _) = space0(input)?;
        let (input, _) = line_ending(input)?;
        let mut remaining = input;
        let mut result = Vec::new();

        loop {
            if !remaining.starts_with(' ') && !remaining.starts_with('\t') {
                break;
            }
            let (new_input, _) = space1(remaining)?;
            // Skip blank indented lines.
            if new_input.starts_with('\n') || new_input.starts_with('\r') {
                let (new_input, _) = line_ending(new_input)?;
                remaining = new_input;
                continue;
            }
            let (new_input, name): (&str, &str) = take_while1(identifier_char)(new_input)?;
            let (new_input, _) = line_ending(new_input)?;
            result.push(name.to_string());
            remaining = new_input;
        }

        Ok((remaining, result))
    } else {
        // Single-line: comma-separated identifiers.
        let identifier = take_while1(identifier_char);
        let deps = separated_list0(tag(","), preceded(space0, identifier));
        let (input, (_, names, _)) = (space1, deps, line_ending).parse(input)?;
        Ok((input, names.into_iter().map(|t| t.to_string()).collect()))
    }
}

/// `input_line` parses input file patterns for cache key computation.
/// Supports both single-line (comma-separated) and multi-line (indented) formats.
/// 
/// # Examples
/// ```text
///     input: src/**, Makefile  => ["src/**", "Makefile"]
///     
///     input:                   # multi-line format
///       src/**
///       Makefile
///       Cargo.toml
/// ```
pub fn input_line(input: &str) -> IResult<&str, Vec<String>> {
    parse_list_field("input:", input)
}

/// `output_line` parses output file patterns.
/// Supports both single-line (comma-separated) and multi-line (indented) formats.
/// 
/// # Examples
/// ```text
///     output: .build/**, dist/  => [".build/**", "dist/"]
///     
///     output:                   # multi-line format
///       target/release/**
///       dist/
/// ```
pub fn output_line(input: &str) -> IResult<&str, Vec<String>> {
    parse_list_field("output:", input)
}

/// Environment variable specification for cache key computation.
/// - `Explicit(value)` = explicit value declared in Farmfile
/// - `Capture` = capture from current environment at build time
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum EnvValue {
    /// Explicit value declared in Farmfile
    Explicit(String),
    /// Capture from current environment (value logged at build time)
    Capture,
}

/// `env_line` parses environment declarations for cache key computation.
/// Supports both explicit values and environment capture.
/// 
/// # Examples
/// ```text
///     env: OS=ubuntu-22.04, NODE_VERSION  # explicit + capture
///     
///     env:                                # multi-line format
///       OS=ubuntu-22.04                   # explicit value
///       NODE_VERSION                      # capture from env at build time
///       RUST_VERSION=1.75
/// ```
pub fn env_line(input: &str) -> IResult<&str, std::collections::HashMap<String, EnvValue>> {
    use nom::bytes::complete::take_while1;
    
    let (input, _) = tag("env:")(input)?;
    
    let key_chars = |c: char| c.is_alphanumeric() || c == '_';
    let value_chars = |c: char| c.is_alphanumeric() || c == '_' || c == '-' || c == '.';
    
    // Peek: check if next non-space character is newline (multi-line) or content (single-line)
    let trimmed = input.trim_start_matches(' ').trim_start_matches('\t');
    
    if trimmed.starts_with('\n') || trimmed.starts_with('\r') {
        // Multi-line format: parse indented lines
        let (input, _) = space0(input)?;
        let (input, _) = line_ending(input)?;
        let mut remaining = input;
        let mut result = std::collections::HashMap::new();
        
        loop {
            // Check for indentation (at least one space)
            if !remaining.starts_with(' ') && !remaining.starts_with('\t') {
                break;
            }
            
            // Skip indentation
            let (new_input, _) = space1(remaining)?;
            
            // Skip empty lines
            if new_input.starts_with('\n') || new_input.starts_with('\r') {
                let (new_input, _) = line_ending(new_input)?;
                remaining = new_input;
                continue;
            }
            
            // Parse key
            let (new_input, k): (&str, &str) = take_while1(key_chars)(new_input)?;
            
            // Check for =value or just key
            if new_input.starts_with('=') {
                let (new_input, _) = tag("=")(new_input)?;
                let (new_input, v): (&str, &str) = take_while1(value_chars)(new_input)?;
                let (new_input, _) = line_ending(new_input)?;
                result.insert(k.to_string(), EnvValue::Explicit(v.to_string()));
                remaining = new_input;
            } else {
                let (new_input, _) = line_ending(new_input)?;
                result.insert(k.to_string(), EnvValue::Capture);
                remaining = new_input;
            }
        }
        
        Ok((remaining, result))
    } else {
        // Single-line format: comma-separated KEY=value or KEY
        let (input, _) = space1(input)?;
        let mut remaining = input;
        let mut result = std::collections::HashMap::new();
        
        loop {
            // Skip whitespace
            while remaining.starts_with(' ') {
                remaining = &remaining[1..];
            }
            
            // Check for end of line
            if remaining.starts_with('\n') || remaining.starts_with('\r') {
                break;
            }
            
            // Parse key
            let key_end = remaining.find(|c: char| !key_chars(c)).unwrap_or(remaining.len());
            if key_end == 0 {
                break;
            }
            let key = &remaining[..key_end];
            remaining = &remaining[key_end..];
            
            // Check for =value or just key
            if remaining.starts_with('=') {
                remaining = &remaining[1..];
                let value_end = remaining.find(|c: char| !value_chars(c)).unwrap_or(remaining.len());
                let value = &remaining[..value_end];
                remaining = &remaining[value_end..];
                result.insert(key.to_string(), EnvValue::Explicit(value.to_string()));
            } else {
                result.insert(key.to_string(), EnvValue::Capture);
            }
            
            // Skip comma if present
            while remaining.starts_with(' ') {
                remaining = &remaining[1..];
            }
            if remaining.starts_with(',') {
                remaining = &remaining[1..];
            }
        }
        
        let (remaining, _) = line_ending(remaining)?;
        Ok((remaining, result))
    }
}

/// Helper to parse a list field (inputs/outputs) that supports single-line or multi-line format.
fn parse_list_field<'a>(field_tag: &str, input: &'a str) -> IResult<&'a str, Vec<String>> {
    use nom::bytes::complete::take_while1;
    
    let (input, _) = tag(field_tag)(input)?;
    
    // Pattern chars: alphanumeric, underscore, hyphen, dot, slash, asterisk, curly braces
    let pattern_char = |c: char| {
        c.is_alphanumeric() || c == '_' || c == '-' || c == '.' || c == '/' || c == '*' || c == '{' || c == '}'
    };
    
    // Peek: check if next non-space character is newline (multi-line) or content (single-line)
    let trimmed = input.trim_start_matches(' ').trim_start_matches('\t');
    
    if trimmed.starts_with('\n') || trimmed.starts_with('\r') {
        // Multi-line format: parse indented lines
        let (input, _) = space0(input)?;
        let (input, _) = line_ending(input)?;
        let mut remaining = input;
        let mut result = Vec::new();
        
        loop {
            // Check for indentation (at least one space)
            if !remaining.starts_with(' ') && !remaining.starts_with('\t') {
                break;
            }
            
            // Skip indentation
            let (new_input, _) = space1(remaining)?;
            
            // Skip empty lines
            if new_input.starts_with('\n') || new_input.starts_with('\r') {
                let (new_input, _) = line_ending(new_input)?;
                remaining = new_input;
                continue;
            }
            
            // Parse pattern
            let (new_input, pattern): (&str, &str) = take_while1(pattern_char)(new_input)?;
            let (new_input, _) = line_ending(new_input)?;
            
            result.push(pattern.to_string());
            remaining = new_input;
        }
        
        Ok((remaining, result))
    } else {
        // Single-line format: comma-separated patterns (require space after colon)
        let (input, _) = space1(input)?;
        let mut patterns = separated_list0(
            tag(","),
            preceded(space0, take_while1(pattern_char))
        );
        let (input, names): (&str, Vec<&str>) = patterns.parse(input)?;
        let (input, _) = line_ending(input)?;
        Ok((input, names.into_iter().map(|t| t.trim().to_string()).collect()))
    }
}

/// Skip comment lines that start with // or #
pub fn comment_line(input: &str) -> IResult<&str, String> {
    // Don't match plugin comments (//! ...) - those should be handled by plugin_comment_line
    // First check if this is a plugin comment and reject it
    if input.starts_with("//!") {
        return Err(nom::Err::Error(nom::error::Error::new(input, nom::error::ErrorKind::Tag)));
    }
    
    let mut comment_content = alt((
        preceded(
            tag("//"),
            terminated(nom::character::complete::not_line_ending, line_ending)
        ),
        preceded(
            tag("#"),
            terminated(nom::character::complete::not_line_ending, line_ending)
        ),
    ));
    let (input, content) = comment_content.parse(input)?;
    Ok((input, content.trim_start().to_string()))
}

/// Parse plugin directive comments like //! plugin: key=value
pub fn plugin_comment_line(input: &str) -> IResult<&str, (String, String, String)> {
    use nom::character::complete::{space0, not_line_ending};
    use nom::bytes::complete::take_while1;
    
    // Parse //! plugin:
    let (input, _) = tag("//!")(input)?;
    let (input, _) = space0(input)?;
    
    // Parse plugin name
    let (input, plugin_name) = take_while1(|c: char| c.is_alphanumeric() || c == '_' || c == '-')(input)?;
    let (input, _) = tag(":")(input)?;
    let (input, _) = space0(input)?;
    
    // Parse key=value pair
    let (input, key) = take_while1(|c: char| c.is_alphanumeric() || c == '_' || c == '-')(input)?;
    let (input, _) = tag("=")(input)?;
    
    // Parse value (everything until end of line)
    let (input, value) = not_line_ending(input)?;
    let (input, _) = line_ending(input)?;
    
    Ok((input, (plugin_name.to_string(), key.to_string(), value.to_string())))
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_comment_line_slash() {
        assert_eq!(comment_line("// this is a comment\n"), Ok(("", "this is a comment".into())));
    }

    #[test]
    fn test_comment_line_hash() {
        assert_eq!(comment_line("# another comment\n"), Ok(("", "another comment".into())));
    }
    
    #[test]
    fn test_plugin_comment_line() {
        assert_eq!(
            plugin_comment_line("//! parcel: output_dir=build/artifacts\n"), 
            Ok(("", ("parcel".to_string(), "output_dir".to_string(), "build/artifacts".to_string())))
        );
        
        assert_eq!(
            plugin_comment_line("//!ai-collector:format=json\n"), 
            Ok(("", ("ai-collector".to_string(), "format".to_string(), "json".to_string())))
        );
        
        assert_eq!(
            plugin_comment_line("//! test_plugin: enable_feature=true\n"), 
            Ok(("", ("test_plugin".to_string(), "enable_feature".to_string(), "true".to_string())))
        );
        
        // Test exact user case
        assert_eq!(
            plugin_comment_line("//! parcel: parcelfile=parcelfile.alfs\n"), 
            Ok(("", ("parcel".to_string(), "parcelfile".to_string(), "parcelfile.alfs".to_string())))
        );
    }
    use super::*;

    #[test]
    fn test_work_line_simple() {
        assert_eq!(work_line("work: ls -al /tmp\n"), Ok(("", "ls -al /tmp".into())));
        assert_eq!(work_line("work: ls -al /tmp\r\n"), Ok(("", "ls -al /tmp".into())));
        // EOF is also accepted (for last line in file without trailing newline)
        assert_eq!(work_line("work: ls -al /tmp"), Ok(("", "ls -al /tmp".into())));
    }

    #[test]
    fn test_work_line_indented_continuation() {
        // Continuation lines indented under `work:` are folded with a single space.
        let input = "work: docker buildx build\n      --platform linux/amd64\n      --target export\n      .\n";
        let expected = "docker buildx build --platform linux/amd64 --target export .";
        assert_eq!(work_line(input), Ok(("", expected.into())));
    }

    #[test]
    fn test_work_line_continuation_stops_at_field() {
        // `input:` at column 0 ends the continuation block; remaining input is preserved.
        let input = "work: docker buildx build\n      --platform linux/amd64\n      .\ninput: Dockerfile\n";
        let (remaining, command) = work_line(input).unwrap();
        assert_eq!(command, "docker buildx build --platform linux/amd64 .");
        assert_eq!(remaining, "input: Dockerfile\n");
    }

    #[test]
    fn test_work_line_continuation_stops_at_blank_line() {
        let input = "work: cmd one\n  two\n\n  three\n";
        let (remaining, command) = work_line(input).unwrap();
        assert_eq!(command, "cmd one two");
        assert_eq!(remaining, "\n  three\n");
    }

    #[test]
    fn test_after_line() {
        assert_eq!(after_line("after: a, b, c\n"), Ok(("", vec!["a".into(), "b".into(), "c".into()])));
        assert_eq!(after_line("after: a\n"), Ok(("", vec!["a".into()])));
    }

    #[test]
    fn test_after_line_multiline() {
        let input = "after:\n    build-linux-amd64-docker\n    build-linux-arm64-docker\n";
        assert_eq!(
            after_line(input),
            Ok(("", vec!["build-linux-amd64-docker".into(), "build-linux-arm64-docker".into()]))
        );
    }

    #[test]
    fn test_after_line_multiline_single_entry() {
        let input = "after:\n  setup_infrastructure\n";
        assert_eq!(after_line(input), Ok(("", vec!["setup_infrastructure".into()])));
    }

    #[test] 
    fn test_work_block_simple() {
        let input = "work: ```\necho hello\n```\n";
        let expected = "echo hello".into();
        assert_eq!(work_line(input), Ok(("", expected)));
    }

    #[test]
    fn test_work_block_multiline() {
        let input = "work: ```\nset -e\necho \"Building...\"\ncargo build --release\n```\n";
        let expected = "set -e\necho \"Building...\"\ncargo build --release".into();
        assert_eq!(work_line(input), Ok(("", expected)));
    }

    #[test]
    fn test_work_block_preserves_whitespace() {
        let input = "work: ```\n  indented line\n    more indented\n  back to two spaces\n```\n";
        let expected = "  indented line\n    more indented\n  back to two spaces".into();
        assert_eq!(work_line(input), Ok(("", expected)));
    }

    #[test]
    fn test_work_block_complex() {
        let input = r#"work: ```
if [ "$DEBUG" = "1" ]; then
    cargo build
else
    cargo build --release
fi
```
"#;
        let expected = "if [ \"$DEBUG\" = \"1\" ]; then\n    cargo build\nelse\n    cargo build --release\nfi".into();
        assert_eq!(work_line(input), Ok(("", expected)));
    }

    #[test]
    fn test_work_block_empty() {
        let input = "work: ```\n```\n";
        let expected = "".into();
        assert_eq!(work_line(input), Ok(("", expected)));
    }

    #[test]
    fn test_work_block_with_empty_lines() {
        let input = "work: ```\necho start\n\necho end\n```\n";
        let expected = "echo start\n\necho end".into();
        assert_eq!(work_line(input), Ok(("", expected)));
    }

    #[test]
    fn test_work_block_with_heredoc_content() {
        let input = r#"work: ```
cat << 'EOF'
This is a heredoc
with multiple lines
EOF
```
"#;
        let expected = "cat << 'EOF'\nThis is a heredoc\nwith multiple lines\nEOF".into();
        assert_eq!(work_line(input), Ok(("", expected)));
    }

    #[test]
    fn test_work_single_line_vs_block() {
        let single = "work: echo hello\n";
        let single_expected = "echo hello".into();
        
        let block = "work: ```\necho hello\n```\n";
        let block_expected = "echo hello".into();
        
        assert_eq!(work_line(single), Ok(("", single_expected)));
        assert_eq!(work_line(block), Ok(("", block_expected)));
    }

    #[test]
    fn parse() {
        let input = "version: 1\n";
        let expected = 1;
        assert_eq!(version_line(input), Ok(("", expected)));
    }

    #[test]
    fn test_input_line_single() {
        assert_eq!(
            input_line("input: src/**, Makefile\n"),
            Ok(("", vec!["src/**".into(), "Makefile".into()]))
        );
    }

    #[test]
    fn test_input_line_single_one() {
        assert_eq!(
            input_line("input: src/**\n"),
            Ok(("", vec!["src/**".into()]))
        );
    }

    #[test]
    fn test_input_line_multiline() {
        let input = "input:\n  src/**\n  Makefile\n  Cargo.toml\n";
        assert_eq!(
            input_line(input),
            Ok(("", vec!["src/**".into(), "Makefile".into(), "Cargo.toml".into()]))
        );
    }

    #[test]
    fn test_output_line_single() {
        assert_eq!(
            output_line("output: .build/**, dist/\n"),
            Ok(("", vec![".build/**".into(), "dist/".into()]))
        );
    }

    #[test]
    fn test_output_line_multiline() {
        let input = "output:\n  target/release/**\n  dist/\n";
        assert_eq!(
            output_line(input),
            Ok(("", vec!["target/release/**".into(), "dist/".into()]))
        );
    }

    #[test]
    fn test_env_line_explicit_single() {
        let result = env_line("env: OS=ubuntu-22.04, ARCH=x86_64\n").unwrap();
        assert_eq!(result.0, "");
        assert_eq!(result.1.get("OS"), Some(&EnvValue::Explicit("ubuntu-22.04".into())));
        assert_eq!(result.1.get("ARCH"), Some(&EnvValue::Explicit("x86_64".into())));
    }

    #[test]
    fn test_env_line_capture_single() {
        let result = env_line("env: NODE_VERSION, RUST_VERSION\n").unwrap();
        assert_eq!(result.0, "");
        assert_eq!(result.1.get("NODE_VERSION"), Some(&EnvValue::Capture));
        assert_eq!(result.1.get("RUST_VERSION"), Some(&EnvValue::Capture));
    }

    #[test]
    fn test_env_line_mixed_single() {
        let result = env_line("env: OS=ubuntu-22.04, NODE_VERSION\n").unwrap();
        assert_eq!(result.0, "");
        assert_eq!(result.1.get("OS"), Some(&EnvValue::Explicit("ubuntu-22.04".into())));
        assert_eq!(result.1.get("NODE_VERSION"), Some(&EnvValue::Capture));
    }

    #[test]
    fn test_env_line_multiline() {
        let input = "env:\n  OS=ubuntu-22.04\n  NODE_VERSION\n  RUST_VERSION=1.75\n";
        let result = env_line(input).unwrap();
        assert_eq!(result.0, "");
        assert_eq!(result.1.get("OS"), Some(&EnvValue::Explicit("ubuntu-22.04".into())));
        assert_eq!(result.1.get("NODE_VERSION"), Some(&EnvValue::Capture));
        assert_eq!(result.1.get("RUST_VERSION"), Some(&EnvValue::Explicit("1.75".into())));
    }
}
