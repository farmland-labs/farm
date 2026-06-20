//! SPDX-License-Identifier: MIT OR Apache-2.0

use nom::IResult;
use nom::Parser;
use nom::bytes::complete::{tag, take_while1};
use nom::character::complete::{char, line_ending};
use nom::sequence::{delimited, preceded, terminated};
use nom::combinator::opt;

use super::general::{work_line, after_line, plugin_comment_line, input_line, output_line, env_line};
use crate::vendor::parse::task::Task;
use crate::vendor::parse::plan::Operation;

/// Parse and skip a variant line (variant are ignored at operation level)
fn parse_and_skip_variant_line(input: &str) -> IResult<&str, ()> {
    use nom::bytes::complete::take_while;
    
    let (input, _) = (
        tag("variant:"),
        take_while(|c| c != '\n' && c != '\r'),
        line_ending
    ).parse(input)?;
    
    Ok((input, ()))
}

/// `operation_tag` parses a tag name of an operation.
/// The tag name is a sequence of alphanumeric characters, underscores, and hyphens.
/// 
/// # Examples
/// ```text
///     "operation" =>  "root"
///     "operation.build"   =>  "build"
///     "operation.debug_config" => "debug_config"
/// ```
pub fn operation_tag(input: &str) -> IResult<&str, String> {
    // Define identifier pattern: alphanumeric, underscore, hyphen
    let identifier = take_while1(|c: char| c.is_alphanumeric() || c == '_' || c == '-');
    let name = preceded(tag("."), identifier);
    let (input, (_, tag_name)) = (tag("operation"), opt(name)).parse(input)?;
    match tag_name {
        Some(name) => Ok((input, name.to_string())),
        None => Ok((input, "root".to_string())),
    }
}

/// `operation_header` parses the header of an operation section.
/// The operation header is an `operation_tag` enclosed in square brackets.
/// 
/// # Examples
/// ```text
///     "[operation]"   =>  "root"
///     "[operation.build]" =>  "build"
/// ```
pub fn operation_header(input: &str) -> IResult<&str, String> {
    let (input, name) = terminated(delimited(char('['), operation_tag, char(']')), line_ending).parse(input)?;
    Ok((input, name.to_string()))
}

/// `operation_section` parses a complete section of an operation.
/// The operation section can have plugin comment directives (//! plugin: key=value) 
/// appearing BEFORE the operation header, acting as decorators.
/// The operation header is enclosed in square brackets.
/// The body comprises a permutation of `work_line`, `after_line`, and `variant_line` (ignored).
/// Variant lines in operations are parsed but ignored, since variant
/// are now treated as environment variables only.
/// 
/// # Examples
/// ```text
/// //! parcel: compress=true
/// //! parcel: format=zip
/// [operation.build]
///     work: make build
///     after: prepare
/// ```
/// 
/// Or the old style (still supported for backward compatibility):
/// ```text
/// [operation.build]
///     //! parcel: compress=true
///     work: make build
/// ```
pub fn operation_section(input: &str) -> IResult<&str, Operation> {
    // Parse the operation header
    let (input, label) = operation_header(input)?;
    
    // Create operation (plugin metadata will be added by the plan parser)
    let mut operation = Operation::new(label.clone());
    
    // Parse body lines until we hit another section or EOF
    let mut remaining = input;
    
    // Track whether we've started parsing the operation body (work lines)
    let mut has_work_lines = false;
    
    loop {
        // Try to parse each type of line
        if let Ok((new_input, shell)) = work_line(remaining) {
            operation.tasks.push(Task::shell(shell));
            remaining = new_input;
            has_work_lines = true;
            continue;
        }
        
        if let Ok((new_input, deps)) = after_line(remaining) {
            operation.depends = deps;
            remaining = new_input;
            continue;
        }
        
        // Parse input line (for cache key computation)
        if let Ok((new_input, inputs)) = input_line(remaining) {
            operation.inputs = inputs;
            remaining = new_input;
            continue;
        }
        
        // Parse output line
        if let Ok((new_input, outputs)) = output_line(remaining) {
            operation.outputs = outputs;
            remaining = new_input;
            continue;
        }
        
        // Parse env line (for cache key computation)
        if let Ok((new_input, env)) = env_line(remaining) {
            operation.declared_env = env;
            remaining = new_input;
            continue;
        }
        
        // Parse inline plugin directive comments (only before work lines start)
        // This prevents consuming decorator comments that belong to the next operation
        if !has_work_lines {
            if let Ok((new_input, (plugin_name, key, value))) = plugin_comment_line(remaining) {
                operation.plugin_metadata
                    .entry(plugin_name)
                    .or_default()
                    .insert(key, value);
                remaining = new_input;
                continue;
            }
        }
        
        // Skip variant lines (variant are ignored at operation level)
        if let Ok((new_input, _)) = parse_and_skip_variant_line(remaining) {
            remaining = new_input;
            continue;
        }
        
        // Skip empty lines
        if let Ok((new_input, _)) = line_ending::<&str, nom::error::Error<&str>>(remaining) {
            remaining = new_input;
            continue;
        }
        
        // If we can't parse anything else, we're done with this section
        break;
    }
    
    Ok((remaining, operation))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_operation_tag() {
        assert_eq!(operation_tag("operation").unwrap(), ("", "root".to_string()));
        assert_eq!(operation_tag("operation.build").unwrap(), ("", "build".to_string()));
        assert_eq!(operation_tag("operation.debug_config").unwrap(), ("", "debug_config".to_string()));
    }

    #[test]
    fn test_operation_header() {
        assert_eq!(operation_header("[operation]\n").unwrap(), ("", "root".to_string()));
        assert_eq!(operation_header("[operation.build]\n").unwrap(), ("", "build".to_string()));
    }

    #[test]
    fn test_simple_operation() {
        let input = "[operation.build]\nwork: make build\n";
        let (_, operation) = operation_section(input).unwrap();
        
        assert_eq!(operation.label, "build");
        assert_eq!(operation.tasks.len(), 1);
        assert_eq!(operation.tasks[0].command().unwrap(), "make build");
    }

    #[test]
    fn test_operation_with_work() {
        let input = "[operation.build]\nwork: make build\n";
        let (_, operation) = operation_section(input).unwrap();
        
        assert_eq!(operation.label, "build");
        assert_eq!(operation.tasks.len(), 1);
    }

    #[test]
    fn test_operation_with_dependencies() {
        let input = "[operation.build]\nwork: make build\nafter: prepare, setup\n";
        let (_, operation) = operation_section(input).unwrap();
        
        assert_eq!(operation.label, "build");
        assert_eq!(operation.depends, vec!["prepare", "setup"]);
        assert_eq!(operation.tasks.len(), 1);
    }

    #[test]
    fn test_complex_operation() {
        let input = "[operation.build]\nwork: make build VARIANT=debug\nafter: prepare\n";
        let (_, operation) = operation_section(input).unwrap();
        
        assert_eq!(operation.label, "build");
        assert_eq!(operation.depends, vec!["prepare"]);
        assert_eq!(operation.tasks.len(), 1);
        assert_eq!(operation.tasks[0].command().unwrap(), "make build VARIANT=debug");
    }

    #[test]
    fn test_operation_with_multiline_work() {
        let input = r#"[operation.build]
work: ```
set -e
echo "Building..."
cargo build --release
```
after: prepare
"#;
        let (_, operation) = operation_section(input).unwrap();
        
        assert_eq!(operation.label, "build");
        assert_eq!(operation.tasks.len(), 1);
        assert_eq!(operation.depends, vec!["prepare"]);
        
        // Check the multiline content
        let expected_command = "set -e\necho \"Building...\"\ncargo build --release";
        assert_eq!(operation.tasks[0].command(), Some(expected_command));
    }

    #[test]
    fn test_operation_mixed_work_types() {
        let input = r#"[operation.build]
work: echo "Starting build"
work: ```
if [ "$DEBUG" = "1" ]; then
    cargo build
else
    cargo build --release
fi
```
work: echo "Build complete"
"#;
        let (_, operation) = operation_section(input).unwrap();
        
        assert_eq!(operation.label, "build");
        assert_eq!(operation.tasks.len(), 3);
        
        // Check all three commands
        assert_eq!(operation.tasks[0].command(), Some("echo \"Starting build\""));
        assert_eq!(operation.tasks[1].command(), Some("if [ \"$DEBUG\" = \"1\" ]; then\n    cargo build\nelse\n    cargo build --release\nfi"));
        assert_eq!(operation.tasks[2].command(), Some("echo \"Build complete\""));
    }
    
    #[test]
    fn test_sunflower_oil_format() {
        // Test parsing exactly like the sunflower_oil.Farmfile format
        let input = "[operation.establish_well]\nvariant: sunburst_premium, classic_bulk\nwork: echo 'Installing irrigation system...'\n";
        let result = operation_section(input);
        assert!(result.is_ok(), "Failed to parse sunflower oil format: {:?}", result);
        let (remaining, parsed_op) = result.unwrap();
        assert_eq!(remaining, "");
        assert_eq!(parsed_op.label, "establish_well");
        assert_eq!(parsed_op.tasks.len(), 1);
        match &parsed_op.tasks[0] {
            Task::Shell(shell_task) => {
                assert_eq!(shell_task.command, "echo 'Installing irrigation system...'");
            }
        }
    }

    #[test] 
    fn test_operation_with_variant_line_ignored() {
        // Regression test: variant lines should be parsed but ignored
        let input = "[operation.test_op]\nvariant: debug, release, production\nwork: echo 'test command'\nafter: dependency\n";
        let result = operation_section(input);
        assert!(result.is_ok(), "Should parse operation with variant line: {:?}", result);
        
        let (remaining, parsed_op) = result.unwrap();
        assert_eq!(remaining, "");
        assert_eq!(parsed_op.label, "test_op");
        
        // Should have exactly one task (variant line ignored)
        assert_eq!(parsed_op.tasks.len(), 1);
        match &parsed_op.tasks[0] {
            Task::Shell(shell_task) => {
                assert_eq!(shell_task.command, "echo 'test command'");
            }
        }
        
        // Should have dependencies parsed correctly
        assert_eq!(parsed_op.depends, vec!["dependency"]);
    }

    #[test]
    fn test_multiple_variant_formats() {
        // Test different variant line formats that should all be ignored
        let test_cases = ["[operation.test1]\nvariant: debug\nwork: echo 'single variant'\n",
            "[operation.test2]\nvariant: debug, release\nwork: echo 'multiple variant'\n", 
            "[operation.test3]\nvariant: debug,release,production\nwork: echo 'no spaces'\n",
            "[operation.test4]\nvariant: very_long_variant_name_with_underscores\nwork: echo 'long name'\n"];
        
        for (i, test_case) in test_cases.iter().enumerate() {
            let result = operation_section(test_case);
            assert!(result.is_ok(), "Test case {} failed: {:?}", i, result);
            
            let (remaining, parsed_op) = result.unwrap();
            assert_eq!(remaining, "");
            assert_eq!(parsed_op.tasks.len(), 1, "Test case {} should have exactly 1 task", i);
        }
    }

    #[test]
    fn test_operation_with_plugin_comments() {
        let input = r#"[operation.build]
//! parcel: output_dir=build/artifacts
//! parcel: compress=true
//! ai-collector: format=json
work: make build
after: prepare
"#;
        let (_, operation) = operation_section(input).unwrap();
        
        assert_eq!(operation.label, "build");
        assert_eq!(operation.tasks.len(), 1);
        assert_eq!(operation.depends, vec!["prepare"]);
        
        // Check plugin metadata
        assert_eq!(operation.plugin_metadata.len(), 2);
        
        // Check parcel plugin metadata
        let parcel_metadata = operation.plugin_metadata.get("parcel").unwrap();
        assert_eq!(parcel_metadata.get("output_dir"), Some(&"build/artifacts".to_string()));
        assert_eq!(parcel_metadata.get("compress"), Some(&"true".to_string()));
        
        // Check ai-collector plugin metadata
        let ai_metadata = operation.plugin_metadata.get("ai-collector").unwrap();
        assert_eq!(ai_metadata.get("format"), Some(&"json".to_string()));
    }

    #[test]
    fn test_operation_plugin_comments_mixed_order() {
        // Plugin comments are only parsed BEFORE work lines (inline) or BEFORE the header (decorator)
        let input = r#"[operation.deploy]
//! deployment: environment=staging
//! parcel: target=remote
//! deployment: retry_count=3
work: echo "Starting deployment"
work: ./deploy.sh
"#;
        let (_, operation) = operation_section(input).unwrap();
        
        assert_eq!(operation.label, "deploy");
        assert_eq!(operation.tasks.len(), 2);
        
        // Check deployment plugin metadata (both should be captured before work lines)
        let deployment_metadata = operation.plugin_metadata.get("deployment").unwrap();
        assert_eq!(deployment_metadata.get("environment"), Some(&"staging".to_string()));
        assert_eq!(deployment_metadata.get("retry_count"), Some(&"3".to_string()));
        
        // Check parcel plugin metadata
        let parcel_metadata = operation.plugin_metadata.get("parcel").unwrap();
        assert_eq!(parcel_metadata.get("target"), Some(&"remote".to_string()));
    }
}
