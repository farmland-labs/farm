//! SPDX-License-Identifier: MIT OR Apache-2.0

//! `farm run --analyze` command.


use crate::cli::Args;
use crate::cmd::execute::handle_execute_command;

/// Handle `farm run --analyze` - run with file change tracking
pub(crate) fn handle_analyze_run(
    args: Args,
    track_workspace: bool,
    track_paths: Vec<String>,
    track_excludes: Vec<String>,
    interactive: bool,
    keep_snapshots: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    use farm::cache::{WorkspaceSnapshot, check_diff_coverage, prompt_continue};
    
    let workspace = std::env::current_dir()?;
    let target = args.target.as_ref().ok_or("No target specified")?;
    
    println!("🔍 Analyze mode: tracking file changes during execution");
    
    // Parse Farmfile to get declared input/output
    let content = std::fs::read_to_string(&args.farmfile)?;
    let plan = farm::vendor::parse::parse_plan(&content)
        .map_err(|e| format!("Failed to parse Farmfile: {}", e))?;
    
    // Find the target operation
    let operation = plan.operations
        .iter()
        .find(|op| op.label == *target)
        .ok_or(format!("Target '{}' not found in Farmfile", target))?;
    
    // Determine what patterns to track
    let patterns: Vec<String> = if track_workspace {
        vec!["**/*".to_string()]
    } else {
        // Track declared output from dependencies
        let mut patterns = track_paths.clone();
        
        for dep_label in &operation.depends {
            if let Some(dep) = plan.operations.iter().find(|op| op.label == *dep_label) {
                patterns.extend(dep.outputs.clone());
            }
        }
        
        if patterns.is_empty() {
            println!("⚠️  No output patterns found from dependencies.");
            println!("   Try --track-workspace or --track-path to specify patterns.");
            return Ok(());
        }
        
        patterns
    };
    
    println!("📁 Tracking {} pattern(s):", patterns.len());
    for pattern in &patterns {
        println!("   • {}", pattern);
    }
    
    if !track_excludes.is_empty() {
        println!("🚫 Excluding {} pattern(s):", track_excludes.len());
        for pattern in &track_excludes {
            // Show normalized pattern (dir/ -> dir/**/*)
            if pattern.ends_with('/') {
                let p = pattern.trim_end_matches('/');
                println!("   • {} → {}/**/*", pattern, p);
            } else {
                println!("   • {}", pattern);
            }
        }
    }
    
    // Interactive prompt BEFORE expensive file enumeration
    if interactive {
        println!();
        if !prompt_continue("Proceed with analysis?")? {
            println!("Analysis cancelled.");
            return Ok(());
        }
    }
    
    // Expand patterns to show what files will be tracked (can be slow in large repos)
    println!();
    print!("📋 Enumerating files ");
    use std::io::Write;
    std::io::stdout().flush().ok();
    
    let mut matched_files: Vec<std::path::PathBuf> = Vec::new();
    let mut scanned_count = 0usize;

    for pattern in &patterns {
        let full_pattern = if pattern.starts_with('/') {
            pattern.to_string()
        } else {
            workspace.join(pattern).to_string_lossy().to_string()
        };
        
        if let Ok(paths) = glob::glob(&full_pattern) {
            for entry in paths.flatten() {
                scanned_count += 1;
                
                // Progress indicator: dot every 1000, number every 10000
                if scanned_count.is_multiple_of(1000) {
                    if scanned_count.is_multiple_of(10000) {
                        print!("{}k", scanned_count / 1000);
                    } else {
                        print!(".");
                    }
                    std::io::stdout().flush().ok();
                }
                
                if entry.is_file() {
                    if let Ok(rel) = entry.strip_prefix(&workspace) {
                        matched_files.push(rel.to_path_buf());
                    } else {
                        matched_files.push(entry);
                    }
                }
            }
        }
    }
    print!(" done");
    std::io::stdout().flush().ok();
    println!(); // End the progress line
    
    print!("   Sorting and deduplicating...");
    std::io::stdout().flush().ok();
    matched_files.sort();
    matched_files.dedup();
    println!(" done");
    
    // Compile exclude patterns once (reused for both before and after snapshots)
    let exclude_patterns: Vec<glob::Pattern> = if !track_excludes.is_empty() {
        // Normalize exclude patterns: "dir/" -> "dir/**/*" for convenience
        track_excludes
            .iter()
            .map(|p| {
                if p.ends_with('/') {
                    let p = p.trim_end_matches('/');
                    format!("{}/**/*", p)
                } else {
                    p.to_string()
                }
            })
            .filter_map(|p| glob::Pattern::new(&p).ok())
            .collect()
    } else {
        Vec::new()
    };
    
    // Apply exclude patterns
    if !exclude_patterns.is_empty() {
        let pre_exclude_count = matched_files.len();
        print!("   Applying {} exclude pattern(s)...", exclude_patterns.len());
        std::io::stdout().flush().ok();
        
        matched_files.retain(|path| {
            let path_str = path.to_string_lossy();
            !exclude_patterns.iter().any(|pattern| pattern.matches(&path_str))
        });
        
        let excluded = pre_exclude_count - matched_files.len();
        println!(" excluded {} file(s)", excluded);
    }
    
    println!("✓  Found {} file(s) to track (scanned {})", matched_files.len(), scanned_count);
    for path in matched_files.iter().take(10) {
        println!("   • {}", path.display());
    }
    if matched_files.len() > 10 {
        println!("   ... and {} more", matched_files.len() - 10);
    }
    
    // Capture snapshot before execution (reuse enumerated files - no re-scan)
    println!();
    println!("📸 Capturing snapshot (before):");
    let snap_before_path = workspace.join(".farm").join(format!(".analyze.{}.before.txt", target));
    std::fs::create_dir_all(snap_before_path.parent().unwrap())?;
    
    let snap_before = WorkspaceSnapshot::capture_files(&workspace, &matched_files, &snap_before_path, true)?;
    println!("✓  Captured {} files", snap_before.file_count());
    
    // Run the build
    println!();
    let result = handle_execute_command(args.clone());
    
    // Capture snapshot after execution (must re-enumerate - files may have changed)
    println!();
    println!("📸 Capturing snapshot (after):");
    
    // Re-enumerate files (new files may have been created)
    print!("   Enumerating ");
    std::io::stdout().flush().ok();
    let mut after_files: Vec<std::path::PathBuf> = Vec::new();
    let mut after_scanned = 0usize;
    for pattern in &patterns {
        let full_pattern = if pattern.starts_with('/') {
            pattern.to_string()
        } else {
            workspace.join(pattern).to_string_lossy().to_string()
        };
        
        if let Ok(paths) = glob::glob(&full_pattern) {
            for entry in paths.flatten() {
                after_scanned += 1;
                if after_scanned.is_multiple_of(1000) {
                    if after_scanned.is_multiple_of(10000) {
                        print!("{}k", after_scanned / 1000);
                    } else {
                        print!(".");
                    }
                    std::io::stdout().flush().ok();
                }
                if entry.is_file() {
                    if let Ok(rel) = entry.strip_prefix(&workspace) {
                        after_files.push(rel.to_path_buf());
                    } else {
                        after_files.push(entry);
                    }
                }
            }
        }
    }
    print!(" done");
    std::io::stdout().flush().ok();
    println!();
    
    after_files.sort();
    after_files.dedup();
    
    // Apply same exclude patterns
    if !exclude_patterns.is_empty() {
        after_files.retain(|path| {
            let path_str = path.to_string_lossy();
            !exclude_patterns.iter().any(|pattern| pattern.matches(&path_str))
        });
    }
    
    let snap_after_path = workspace.join(".farm").join(format!(".analyze.{}.after.txt", target));
    let snap_after = WorkspaceSnapshot::capture_files(&workspace, &after_files, &snap_after_path, true)?;
    println!("✓  Captured {} files", snap_after.file_count());
    
    // Compare snapshots
    let diff = snap_before.diff(&snap_after)?;
    
    // Display results
    println!();
    println!("📊 File Change Analysis:");
    println!("   Created:  {} files", diff.created.len());
    println!("   Modified: {} files", diff.modified.len());
    println!("   Deleted:  {} files", diff.deleted.len());
    
    if !diff.created.is_empty() {
        println!();
        println!("   Created files:");
        for path in &diff.created[..diff.created.len().min(10)] {
            println!("     + {}", path.display());
        }
        if diff.created.len() > 10 {
            println!("     ... and {} more", diff.created.len() - 10);
        }
    }
    
    if !diff.modified.is_empty() {
        println!();
        println!("   Modified files:");
        for path in &diff.modified[..diff.modified.len().min(10)] {
            println!("     ~ {}", path.display());
        }
        if diff.modified.len() > 10 {
            println!("     ... and {} more", diff.modified.len() - 10);
        }
    }
    
    // Check coverage against input declaration
    let uncovered = check_diff_coverage(&diff, &operation.inputs, &workspace);
    
    if !uncovered.is_empty() {
        println!();
        println!("⚠️  {} modified file(s) not declared in '{}' input:", uncovered.len(), target);
        for path in &uncovered[..uncovered.len().min(10)] {
            println!("     ! {}", path.display());
        }
        if uncovered.len() > 10 {
            println!("     ... and {} more", uncovered.len() - 10);
        }
        println!();
        println!("💡 If '{}' reads these files, consider adding them to input:", target);
        println!("   input: <pattern that matches these files>");
    }
    
    // Cleanup temp files (unless --keep-snapshots)
    if keep_snapshots {
        // Save diff to file
        let diff_path = workspace.join(".farm").join(format!(".analyze.{}.diff.txt", target));
        let mut diff_content = String::new();
        diff_content.push_str(&format!("# Analyze diff for target: {}\n", target));
        diff_content.push_str(&format!("# Created:  {} files\n", diff.created.len()));
        diff_content.push_str(&format!("# Modified: {} files\n", diff.modified.len()));
        diff_content.push_str(&format!("# Deleted:  {} files\n\n", diff.deleted.len()));
        
        if !diff.created.is_empty() {
            diff_content.push_str("## Created\n");
            for path in &diff.created {
                diff_content.push_str(&format!("+ {}\n", path.display()));
            }
            diff_content.push('\n');
        }
        
        if !diff.modified.is_empty() {
            diff_content.push_str("## Modified\n");
            for path in &diff.modified {
                diff_content.push_str(&format!("~ {}\n", path.display()));
            }
            diff_content.push('\n');
        }
        
        if !diff.deleted.is_empty() {
            diff_content.push_str("## Deleted\n");
            for path in &diff.deleted {
                diff_content.push_str(&format!("- {}\n", path.display()));
            }
            diff_content.push('\n');
        }
        
        if !uncovered.is_empty() {
            diff_content.push_str("## Uncovered (not in input declaration)\n");
            for path in &uncovered {
                diff_content.push_str(&format!("! {}\n", path.display()));
            }
        }
        
        std::fs::write(&diff_path, diff_content)?;
        
        println!();
        println!("💾 Snapshot files kept:");
        println!("   Before: {}", snap_before_path.display());
        println!("   After:  {}", snap_after_path.display());
        println!("   Diff:   {}", diff_path.display());
    } else {
        let _ = std::fs::remove_file(&snap_before_path);
        let _ = std::fs::remove_file(&snap_after_path);
    }
    
    result
}

