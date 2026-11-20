//! SPDX-License-Identifier: MIT OR Apache-2.0

//! Main parser module for Farmland configuration files

// `ParseError` is a rich, multi-variant diagnostic type; a boxed error would
// only obscure the parser API for a marginal size win.
#![allow(clippy::result_large_err)]

use crate::vendor::log::debug;
use nom::character::complete::line_ending;
use nom::Parser;
use nom::error::Error;

mod general;
mod utils;
mod operation;
mod variant;
mod goal;
mod cache;
mod env;
mod error;
mod lenient;

use operation::operation_section;
use variant::variant_section;
use goal::goal_section;
use cache::cache_section;
use env::env_section;
use general::{comment_line, plugin_comment_line};
use crate::vendor::parse::Plan;

pub use error::{ParseError, ParseErrorKind, ParseResultExt};
pub use lenient::ParseWarning;
pub use general::EnvValue;

/// Parse a Farmland Farmfile into an operation `Plan`
///
/// Parsing is lenient for forward and backward compatibility: unknown sections,
/// unknown fields and unrecognised lines are skipped and reported in
/// [`Plan::warnings`] (see [`ParseWarning`]). It fails only when the file
/// violates what this parser knows: missing/unsupported `version:`, a known
/// field with an invalid value, or semantic errors (cycles, unknown
/// dependencies, duplicates, ...). Callers wanting a clean file treat
/// non-empty warnings as an error (`farm --strict`).
pub fn parse_plan(input: &str) -> Result<Plan, ParseError> {
    let mut remaining = input;
    
    // Skip comments and empty lines before version line
    loop {
        let mut made_progress = false;
        
        // Skip empty lines
        while let Ok((new_input, _)) = line_ending::<&str, Error<&str>>(remaining) {
            remaining = new_input;
            made_progress = true;
        }
        
        // Skip comment lines
        while let Ok((new_input, _)) = comment_line(remaining) {
            remaining = new_input;
            made_progress = true;
        }
        
        if !made_progress {
            break;
        }
    }
    
    // Now parse and validate the version (mandatory)
    let (mut remaining, version) = general::version_line.parse(remaining)
        .map_err(|e| error::convert_nom_error(input, e))?;
    
    if version != 1 {
        return Err(ParseError::invalid_version(input, input, Some(version.to_string())));
    }

    let mut plan = Plan::new();
    
    // Accumulator for plugin comments that precede operations
    let mut pending_plugin_metadata: std::collections::HashMap<String, std::collections::HashMap<String, String>> = 
        std::collections::HashMap::new();

    // Content skipped for compatibility, reported as warnings
    let mut skipped: Vec<lenient::Skipped> = Vec::new();
    
    // Parse sections manually to avoid many0 issues
    loop {
        // Skip empty lines and comments  
        let mut made_progress = false;
        
        // Skip empty lines
        while let Ok((new_input, _)) = line_ending::<&str, Error<&str>>(remaining) {
            remaining = new_input;
            made_progress = true;
        }
        
        // Try to parse plugin comment lines (these are decorators for the next operation)
        while let Ok((new_input, (plugin_name, key, value))) = plugin_comment_line(remaining) {
            debug!("Parsed plugin comment: plugin={}, key={}, value={}", plugin_name, key, value);
            pending_plugin_metadata
                .entry(plugin_name)
                .or_default()
                .insert(key, value);
            debug!("pending_plugin_metadata now: {:?}", pending_plugin_metadata);
            remaining = new_input;
            made_progress = true;
        }
        
        // Skip regular comment lines
        while let Ok((new_input, _)) = comment_line(remaining) {
            remaining = new_input;
            made_progress = true;
        }
        
        // Check for EOF
        if remaining.is_empty() {
            break;
        }
        
        // Try to parse different section types
        if let Ok((new_input, (variant, variant_skipped))) = variant_section(remaining) {
            debug!("Parsed variant section: {:?}", variant);
            plan.add_variants(variant)
                .map_err(|e| ParseError::generic(input, remaining, e))?;
            skipped.extend(variant_skipped);
            remaining = new_input;
            made_progress = true;
            // variant don't consume plugin metadata, so keep it for the next operation
        } else if let Ok((new_input, (goals, goal_skipped))) = goal_section(remaining) {
            debug!("Parsed goal section: {:?}", goals);
            plan.goals.extend(goals);
            skipped.extend(goal_skipped);
            remaining = new_input;
            made_progress = true;
            // Goals don't consume plugin metadata, so keep it for the next operation
        } else if let Ok((new_input, (cache_config, cache_skipped))) = cache_section(remaining) {
            debug!("Parsed cache section: {:?}", cache_config);
            plan.cache = Some(cache_config);
            skipped.extend(cache_skipped);
            remaining = new_input;
            made_progress = true;
            // Cache doesn't consume plugin metadata
        } else if let Ok((new_input, (env_vars, env_skipped))) = env_section(remaining) {
            debug!("Parsed env section: {:?}", env_vars);
            plan.globals.extend(env_vars);
            skipped.extend(env_skipped);
            remaining = new_input;
            made_progress = true;
            // The [env] section doesn't consume plugin metadata
        } else {
            match operation_section(remaining) {
                Ok((new_input, (mut operation, operation_skipped))) => {
                    debug!("Parsed operation section: {:?}", operation.label);
                    debug!("pending_plugin_metadata before transfer: {:?}", pending_plugin_metadata);

                    // Apply accumulated plugin metadata to this operation
                    for (plugin_name, metadata) in pending_plugin_metadata.drain() {
                        debug!("Transferring metadata for plugin '{}': {:?}", plugin_name, metadata);
                        operation.plugin_metadata
                            .entry(plugin_name)
                            .or_default()
                            .extend(metadata);
                    }

                    debug!("Operation '{}' final plugin_metadata: {:?}", operation.label, operation.plugin_metadata);

                    plan.add_operation(operation)
                        .map_err(|e| ParseError::generic(input, remaining, e))?;
                    skipped.extend(operation_skipped);
                    remaining = new_input;
                    made_progress = true;
                }
                // A known field with an invalid value: the file violates known syntax
                Err(nom::Err::Failure(e)) => {
                    let field = lenient::field_name(e.input).unwrap_or("field");
                    return Err(ParseError::invalid_syntax(
                        input,
                        e.input,
                        format!("a valid '{}:' value", field),
                        lenient::first_line(e.input).to_string(),
                    ));
                }
                Err(_) => {}
            }
        }

        // Nothing known here: skip it for compatibility and report it
        if !made_progress {
            if remaining.starts_with('[') {
                skipped.push(lenient::Skipped::new(
                    remaining,
                    format!("unknown section '{}'", lenient::first_line(remaining)),
                ));
                remaining = lenient::skip_section(remaining);
                // Decorators directly above belonged to the skipped section
                pending_plugin_metadata.clear();
            } else {
                remaining = lenient::skip_unrecognised(remaining, "outside any section", &mut skipped);
            }
        }
    }

    plan.warnings = skipped
        .into_iter()
        .map(|s| ParseWarning { line: error::line_of(input, s.at), message: s.message })
        .collect();
    for warning in &plan.warnings {
        debug!("Farmfile warning: {}", warning);
    }
    
    // Build the dependency graph
    plan.build_graph()
        .map_err(|e| {
            if e.contains("Circular dependency") {
                // Extract operation names from the error message for better reporting
                let ops = vec![]; // TODO: Parse operation names from error
                ParseError::new(input, input, ParseErrorKind::CircularDependency { operations: ops })
            } else if e.contains("depends on unknown operation") {
                // Extract operation names from the error message
                ParseError::new(input, input, ParseErrorKind::Generic { message: e })
            } else {
                ParseError::generic(input, input, e)
            }
        })?;
    
    // Validate that all goal targets reference existing operations
    let operation_labels: std::collections::HashSet<&str> = plan.operations
        .iter()
        .map(|op| op.label.as_str())
        .collect();
    
    for (goal_name, target_operation) in &plan.goals {
        if !operation_labels.contains(target_operation.as_str()) {
            return Err(ParseError::generic(
                input,
                input,
                format!(
                    "Goal '{}' references unknown operation '{}'. Available operations: {}",
                    goal_name,
                    target_operation,
                    operation_labels.iter().cloned().collect::<Vec<_>>().join(", ")
                )
            ));
        }
    }
    
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_minimal_plan() {
        let input = "version: 1\n";
        let plan = parse_plan(input).unwrap();
        
        assert!(plan.variants.is_empty());
        assert!(plan.operations.is_empty());
    }

    #[test]
    fn test_parse_plan_with_variant() {
        let input = r#"version: 1

[variant]
debug
release
test
"#;
        let plan = parse_plan(input).unwrap();
        
        assert_eq!(plan.variants, vec!["debug", "release", "test"]);
        assert!(plan.operations.is_empty());
    }

    #[test]
    fn test_parse_plan_with_stages() {
        let input = r#"version: 1

[operation.prepare]
work: echo "preparing..."

[operation.build]
after: prepare
work: make build

[operation.test]
after: build
work: make test
"#;
        let plan = parse_plan(input).unwrap();
        
        assert_eq!(plan.operations.len(), 3);
        
        // Check prepare stage
        let prepare = plan.find_operation("prepare").unwrap();
        assert_eq!(prepare.label, "prepare");
        assert!(prepare.depends.is_empty());
        assert_eq!(prepare.tasks.len(), 1);
        
        // Check build stage
        let build = plan.find_operation("build").unwrap();
        assert_eq!(build.label, "build");
        assert_eq!(build.depends, vec!["prepare"]);
        assert_eq!(build.tasks.len(), 1);
        
        // Check test stage
        let test = plan.find_operation("test").unwrap();
        assert_eq!(test.label, "test");
        assert_eq!(test.depends, vec!["build"]);
        assert_eq!(test.tasks.len(), 1);
    }

    #[test]
    fn test_parse_full_plan() {
        let input = r#"version: 1

// This is a comment
[variant]
debug
release

// Build operations
[operation.prepare]
work: mkdir -p build
work: echo "prepared"

[operation.build]
after: prepare
work: make build

[operation.test]
after: build
work: make test

// Deployment operation
[operation.deploy]
after: test
work: make deploy
"#;
        let plan = parse_plan(input).unwrap();
        
        assert_eq!(plan.variants, vec!["debug", "release"]);
        assert_eq!(plan.operations.len(), 4);
        
        // Test execution order
        let execution_order = plan.get_execution_order().unwrap();
        let labels: Vec<_> = execution_order.iter().map(|s| &s.label).collect();
        assert_eq!(labels, vec!["prepare", "build", "test", "deploy"]);
    }

    #[test]
    fn test_parse_invalid_version() {
        let input = "version: 2\n";
        let result = parse_plan(input);
        
        assert!(result.is_err());
        let error = result.unwrap_err();
        let error_string = format!("{}", error);
        assert!(error_string.contains("Invalid version '2'"));
    }

    #[test]
    fn test_parse_circular_dependency() {
        let input = r#"version: 1

[operation.a]
after: b
work: echo "a"

[operation.b]
after: a
work: echo "b"
"#;
        let result = parse_plan(input);
        
        assert!(result.is_err());
        let error = result.unwrap_err();
        let error_string = format!("{}", error);
        assert!(error_string.contains("Circular dependency") || error_string.contains("circular"));
    }

    #[test]
    fn test_parse_unknown_dependency() {
        let input = r#"version: 1

[operation.build]
after: unknown_operation
work: make build
"#;
        let result = parse_plan(input);
        
        assert!(result.is_err());
        let error = result.unwrap_err();
        let error_string = format!("{}", error);
        assert!(error_string.contains("unknown operation") || error_string.contains("depends on"));
    }

    #[test]
    fn test_parse_duplicate_stage_labels_no_variant() {
        let input = r#"version: 1

[operation.build]
work: make build

[operation.build]
work: make build2
"#;
        let result = parse_plan(input);
        
        assert!(result.is_err());
        let error = result.unwrap_err();
        let error_string = format!("{}", error);
        assert!(error_string.contains("Duplicate operation") || error_string.contains("duplicate"));
    }

    #[test]
    fn test_parse_with_comments_and_empty_lines() {
        let input = r#"version: 1

// This is a build configuration
// with comments and empty lines

[variant]
debug


release

// Build operation
[operation.build]
work: make build

// Empty line above and below

[operation.test]
after: build
work: make test

"#;
        let plan = parse_plan(input).unwrap();
        
        assert_eq!(plan.variants, vec!["debug", "release"]);
        assert_eq!(plan.operations.len(), 2);
    }

    #[test]
    fn test_parse_with_comments_before_version() {
        let input = r#"# This is a comment before version
// Another comment before version
version: 1

[variant]
test

[operation.test]
work: echo "test"
"#;
        let result = parse_plan(input);
        
        // This should now succeed after fixing the parser
        assert!(result.is_ok(), "Parser should now handle comments before version line");
        
        let plan = result.unwrap();
        assert_eq!(plan.variants, vec!["test"]);
        assert_eq!(plan.operations.len(), 1);
        assert_eq!(plan.operations[0].label, "test");
    }

    #[test]
    fn test_parse_with_mixed_content_before_version() {
        let input = r#"
# This is a comment with empty line before
// Another comment

# Yet another comment
version: 1

[operation.test]
work: echo "test"
"#;
        let result = parse_plan(input);
        
        // This should also succeed
        assert!(result.is_ok(), "Parser should handle mixed empty lines and comments before version");
        
        let plan = result.unwrap();
        assert_eq!(plan.operations.len(), 1);
        assert_eq!(plan.operations[0].label, "test");
    }

    #[test]
    fn test_parse_with_decorator_style_plugin_comments() {
        // Test the decorator-style syntax where plugin comments come BEFORE the operation header
        let input = r#"version: 1

//! parcel: compress=true
//! parcel: format=zip
//! ai-collector: model=production
[operation.build]
work: make build
after: prepare

[operation.prepare]
work: echo "preparing"
"#;
        let plan = parse_plan(input).unwrap();
        
        assert_eq!(plan.operations.len(), 2);
        
        // Find the build operation
        let build_op = plan.operations.iter().find(|op| op.label == "build").unwrap();
        
        // Check plugin metadata from decorators
        let parcel_metadata = build_op.plugin_metadata.get("parcel").unwrap();
        assert_eq!(parcel_metadata.get("compress"), Some(&"true".to_string()));
        assert_eq!(parcel_metadata.get("format"), Some(&"zip".to_string()));
        
        let ai_metadata = build_op.plugin_metadata.get("ai-collector").unwrap();
        assert_eq!(ai_metadata.get("model"), Some(&"production".to_string()));
        
        // Prepare operation should have no metadata
        let prepare_op = plan.operations.iter().find(|op| op.label == "prepare").unwrap();
        assert!(prepare_op.plugin_metadata.is_empty());
    }

    #[test]
    fn test_parse_with_decorator_and_inline_plugin_comments() {
        // Test that both decorator-style and inline plugin comments work together
        let input = r#"version: 1

//! parcel: compress=true
[operation.build]
//! parcel: format=zip
//! ai-collector: format=json
work: make build
"#;
        let plan = parse_plan(input).unwrap();
        
        assert_eq!(plan.operations.len(), 1);
        let operation = &plan.operations[0];
        
        assert_eq!(operation.label, "build");
        
        // Both decorator and inline comments should be combined
        let parcel_metadata = operation.plugin_metadata.get("parcel").unwrap();
        assert_eq!(parcel_metadata.get("compress"), Some(&"true".to_string()));
        assert_eq!(parcel_metadata.get("format"), Some(&"zip".to_string()));
        
        let ai_metadata = operation.plugin_metadata.get("ai-collector").unwrap();
        assert_eq!(ai_metadata.get("format"), Some(&"json".to_string()));
    }

    #[test]
    fn test_parse_multiple_operations_with_decorators() {
        // Test that decorators are correctly assigned to their respective operations
        let input = r#"version: 1

[variant]
debug
release

//! parcel: compress=true
[operation.prepare]
work: echo "preparing"

//! parcel: parcelfile=debug.toml
//! ai-collector: model=debug
[operation.build_debug]
variant: debug
work: make build
after: prepare

//! parcel: parcelfile=release.toml
//! ai-collector: model=production
[operation.build_release]
variant: release
work: make build
after: prepare
"#;
        let plan = parse_plan(input).unwrap();
        
        assert_eq!(plan.operations.len(), 3);
        
        // Check prepare operation has its decorator
        let prepare_op = plan.operations.iter().find(|op| op.label == "prepare").unwrap();
        let prepare_parcel = prepare_op.plugin_metadata.get("parcel").unwrap();
        assert_eq!(prepare_parcel.get("compress"), Some(&"true".to_string()));
        
        // Check build_debug operation has its decorators
        let debug_op = plan.operations.iter().find(|op| op.label == "build_debug").unwrap();
        let debug_parcel = debug_op.plugin_metadata.get("parcel").unwrap();
        assert_eq!(debug_parcel.get("parcelfile"), Some(&"debug.toml".to_string()));
        let debug_ai = debug_op.plugin_metadata.get("ai-collector").unwrap();
        assert_eq!(debug_ai.get("model"), Some(&"debug".to_string()));
        
        // Check build_release operation has its decorators
        let release_op = plan.operations.iter().find(|op| op.label == "build_release").unwrap();
        let release_parcel = release_op.plugin_metadata.get("parcel").unwrap();
        assert_eq!(release_parcel.get("parcelfile"), Some(&"release.toml".to_string()));
        let release_ai = release_op.plugin_metadata.get("ai-collector").unwrap();
        assert_eq!(release_ai.get("model"), Some(&"production".to_string()));
    }
    
    #[test]
    fn test_parse_user_case_parcelfile_alfs() {
        // Test exact user case: //! parcel: parcelfile=parcelfile.alfs before upload operation
        let input = r#"version: 1

//! parcel: parcelfile=parcelfile.alfs
[operation.upload]
work: make upload
"#;
        let plan = parse_plan(input).unwrap();
        
        assert_eq!(plan.operations.len(), 1);
        
        let upload_op = plan.find_operation("upload").unwrap();
        assert_eq!(upload_op.label, "upload");
        
        // This should have parcel metadata
        assert!(upload_op.plugin_metadata.contains_key("parcel"), 
                "upload operation should have parcel plugin metadata");
        
        let parcel_metadata = upload_op.plugin_metadata.get("parcel").unwrap();
        assert_eq!(parcel_metadata.get("parcelfile"), Some(&"parcelfile.alfs".to_string()),
                   "parcel metadata should have parcelfile=parcelfile.alfs");
    }

    #[test]
    fn test_parse_plan_with_goals() {
        let input = r#"version: 1

[goal]
build: build_project
test: run_tests

[operation.build_project]
work: cargo build

[operation.run_tests]
work: cargo test
"#;
        let plan = parse_plan(input).unwrap();
        
        assert_eq!(plan.goals.len(), 2);
        assert_eq!(plan.goals.get("build"), Some(&"build_project".to_string()));
        assert_eq!(plan.goals.get("test"), Some(&"run_tests".to_string()));
        assert_eq!(plan.operations.len(), 2);
    }

    #[test]
    fn test_parse_goal_invalid_target() {
        let input = r#"version: 1

[goal]
build: nonexistent_operation

[operation.actual_build]
work: cargo build
"#;
        let result = parse_plan(input);
        assert!(result.is_err(), "Should fail when goal references unknown operation");
        
        let err = result.unwrap_err();
        let err_msg = err.to_string();
        assert!(err_msg.contains("nonexistent_operation"), 
                "Error should mention the unknown operation name");
    }

    #[test]
    fn test_parse_goal_section_order_independent() {
        // Goals can appear before or after operations
        let input = r#"version: 1

[operation.build_project]
work: cargo build

[goal]
build: build_project
"#;
        let plan = parse_plan(input).unwrap();
        
        assert_eq!(plan.goals.len(), 1);
        assert_eq!(plan.goals.get("build"), Some(&"build_project".to_string()));
    }

    #[test]
    fn test_parse_plan_short_form_syntax() {
        // `[.name]` headers, multi-line `work:` blocks, `help:` and `!` excludes, mixed
        // with the long `[operation.name]` form.
        let input = r#"version: 1

[goal]
build: build_workspace

[operation.setup]
work: echo setup

// Compile everything
[.build_workspace]
help: Compile all workspace crates
after: setup
input:
    **/*.rs
    !firmware/**
output: target/debug/*, !target/debug/*.d
work:
    cargo build --workspace
    # shell comment, passed verbatim
    cargo build --workspace --release
"#;
        let plan = parse_plan(input).unwrap();

        assert_eq!(plan.operations.len(), 2);
        let op = plan.find_operation("build_workspace").unwrap();
        assert_eq!(op.help.as_deref(), Some("Compile all workspace crates"));
        assert_eq!(op.depends, vec!["setup"]);
        assert_eq!(op.inputs, vec!["**/*.rs", "!firmware/**"]);
        assert_eq!(op.outputs, vec!["target/debug/*", "!target/debug/*.d"]);
        assert_eq!(op.tasks.len(), 1);
        assert_eq!(
            op.tasks[0].command(),
            Some("cargo build --workspace\n# shell comment, passed verbatim\ncargo build --workspace --release")
        );
        assert_eq!(plan.find_operation("setup").unwrap().help, None);
    }

    #[test]
    fn test_parse_plan_exclude_only_patterns_is_error() {
        let input = r#"version: 1

[.build]
input: !firmware/**
work: make
"#;
        let err = parse_plan(input).unwrap_err().to_string();
        assert!(err.contains("Operation 'build'") && err.contains("only exclude patterns"), "{err}");
    }

    /// `(line, message)` of every warning, for compact assertions.
    fn warnings(plan: &Plan) -> Vec<(usize, &str)> {
        plan.warnings.iter().map(|w| (w.line, w.message.as_str())).collect()
    }

    #[test]
    fn test_lenient_clean_file_has_no_warnings() {
        let plan = parse_plan("version: 1\n[.build]\nhelp: x\nwork: make\n").unwrap();
        assert!(plan.warnings.is_empty());
    }

    #[test]
    fn test_lenient_unknown_fields_in_operation() {
        let input = r#"version: 1

[.build]
retry: 3
work: make
labels:
    gpu

    linux
timeout: 5m
after: prep

[.prep]
work: echo prep
"#;
        let plan = parse_plan(input).unwrap();
        let build = plan.find_operation("build").unwrap();
        assert_eq!(build.tasks.len(), 1);
        assert_eq!(build.depends, vec!["prep"], "known fields after unknown ones still apply");
        assert_eq!(warnings(&plan), vec![
            (4, "unknown field 'retry' in operation 'build'"),
            (6, "unknown field 'labels' in operation 'build'"),
            (10, "unknown field 'timeout' in operation 'build'"),
        ]);
    }

    #[test]
    fn test_lenient_unknown_section_is_skipped_with_its_decorators() {
        let input = r#"version: 1

//! parcel: compress=true
[secrets]
TOKEN: vault://x
  nested: y

[.build]
work: make
"#;
        let plan = parse_plan(input).unwrap();
        assert_eq!(plan.operations.len(), 1);
        assert!(plan.operations[0].plugin_metadata.is_empty(), "decorators of the skipped section are dropped");
        assert_eq!(warnings(&plan), vec![(4, "unknown section '[secrets]'")]);
    }

    #[test]
    fn test_lenient_unrecognised_top_level_and_goal_lines() {
        let input = r#"version: 1
%%% corrupted ~~~

[goal]
b: build
not a goal line
t: build

[.build]
work: make
"#;
        let plan = parse_plan(input).unwrap();
        assert_eq!(plan.goals.len(), 2, "goal lines after the junk line are kept");
        assert_eq!(warnings(&plan), vec![
            (2, "unrecognised line '%%% corrupted ~~~' outside any section"),
            (6, "unrecognised line 'not a goal line' in [goal] section"),
        ]);
    }

    #[test]
    fn test_lenient_comment_inside_operation_does_not_end_it() {
        let plan = parse_plan("version: 1\n[.build]\n// note\n# note\nwork: make\nafter: prep\n[.prep]\nwork: x\n").unwrap();
        assert_eq!(plan.find_operation("build").unwrap().depends, vec!["prep"]);
        assert!(plan.warnings.is_empty());
    }

    #[test]
    fn test_lenient_misspelled_field_is_only_a_warning() {
        // Typos are not interpreted: `afetr` is an unknown field, so the
        // dependency is dropped and reported (and fails in strict mode).
        let plan = parse_plan("version: 1\n[.a]\nwork: x\n[.build]\nafetr: a\nwork: make\n").unwrap();
        assert!(plan.find_operation("build").unwrap().depends.is_empty());
        assert_eq!(warnings(&plan), vec![(5, "unknown field 'afetr' in operation 'build'")]);
    }

    #[test]
    fn test_lenient_known_field_with_invalid_value_is_error() {
        let err = parse_plan("version: 1\n[.build]\nafter: @@bad\nwork: make\n").unwrap_err().to_string();
        assert!(err.contains("line 3") && err.contains("a valid 'after:' value"), "{err}");

        let err = parse_plan("version: 1\n[.build]\nwork:\nafter: x\n").unwrap_err().to_string();
        assert!(err.contains("a valid 'work:' value"), "empty work block: {err}");
    }

    #[test]
    fn test_lenient_unsupported_version_is_still_error() {
        assert!(parse_plan("version: 2\n[.build]\nwork: make\n").is_err());
    }

    #[test]
    fn test_parse_plan_with_cache() {
        let input = r#"version: 1

[cache]
max_size: 10GB
ttl: 30d
env_key:
    FARM_PLATFORM
    RUST_TARGET
env_command:
    RUSTC_VERSION: rustc --version

[operation.build]
work: cargo build
"#;
        let plan = parse_plan(input).unwrap();
        
        assert!(plan.cache.is_some());
        let cache = plan.cache.unwrap();
        assert_eq!(cache.max_size, Some("10GB".to_string()));
        assert_eq!(cache.ttl, Some("30d".to_string()));
        assert_eq!(cache.env_key, vec!["FARM_PLATFORM", "RUST_TARGET"]);
        assert_eq!(cache.env_command.get("RUSTC_VERSION"), Some(&"rustc --version".to_string()));
    }

    #[test]
    fn test_parse_plan_with_env() {
        let input = r#"version: 1

[env]
NODE_ENV=production
GREETING=hello world

[operation.build]
work: cargo build
"#;
        let plan = parse_plan(input).unwrap();

        assert_eq!(plan.globals.get("NODE_ENV"), Some(&"production".to_string()));
        assert_eq!(plan.globals.get("GREETING"), Some(&"hello world".to_string()));
        assert_eq!(plan.operations.len(), 1);
    }

    #[test]
    fn test_parse_plan_env_section_order_independent() {
        // The [env] section can appear after operations.
        let input = r#"version: 1

[operation.build]
work: cargo build

[env]
RUST_LOG=debug
"#;
        let plan = parse_plan(input).unwrap();
        assert_eq!(plan.globals.get("RUST_LOG"), Some(&"debug".to_string()));
    }

    #[test]
    fn test_parse_plan_cache_section_order_independent() {
        // Cache section can appear anywhere
        let input = r#"version: 1

[operation.build]
work: cargo build

[cache]
max_size: 5GB
"#;
        let plan = parse_plan(input).unwrap();
        
        assert!(plan.cache.is_some());
        assert_eq!(plan.cache.unwrap().max_size, Some("5GB".to_string()));
    }
}
