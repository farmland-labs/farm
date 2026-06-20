//! SPDX-License-Identifier: MIT OR Apache-2.0

//! `farm plan` command.

use std::path::PathBuf;
use farm::vendor::log::info;
use colored::*;

pub(crate) fn handle_plan_command(farmfile: PathBuf, variant: Option<String>, format: String, verbose: bool) -> Result<(), Box<dyn std::error::Error>> {
    // Read and parse the Farmfile
    let content = std::fs::read_to_string(&farmfile)
        .map_err(|e| format!("❌ Failed to read config file {}: {}", farmfile.display(), e))?;
    
    info!("Reading Farmfile: {}", farmfile.display());
    
    let plan = match farm::vendor::parse::parse_plan(&content) {
        Ok(plan) => plan,
        Err(e) => {
            // Display our rich error message with line numbers and context
            eprintln!("❌ Failed to parse config file {}:", farmfile.display());
            eprintln!("{}", e);
            std::process::exit(1);
        }
    };

    info!("Parsed plan with {} operations and {} variants", 
                   plan.operations.len(), plan.variants.len());
    
    if let Some(ref variant_filter) = variant {
        info!("Filtering by variant: {}", variant_filter);
    }
    
    info!("Displaying plan in '{}' format (verbose: {})", format, verbose);

    // Convert parser plan to runtime plan to get proper dependency graph
    let runtime_plan = farm::plan::Plan::from_parsed(plan.clone());
    
    // Build goals lookup: operation -> goal name
    let goals_by_operation: std::collections::HashMap<&str, &str> = plan.goals.iter()
        .map(|(goal, op)| (op.as_str(), goal.as_str()))
        .collect();
    
    // Check if we should use colors (TTY check)
    let use_colors = atty::is(atty::Stream::Stdout);
    
    match format.as_str() {
        "tree" => {
            // Show header information from parser plan
            let variants_str = if plan.variants.is_empty() { 
                "default".to_string() 
            } else { 
                plan.variants.join(", ") 
            };
            
            // Format values with bold if terminal supports it
            let farmfile_val = if use_colors { farmfile.display().to_string().bold().to_string() } else { farmfile.display().to_string() };
            let variants_val = if use_colors { variants_str.bold().to_string() } else { variants_str };
            let ops_val = if use_colors { plan.operations.len().to_string().bold().to_string() } else { plan.operations.len().to_string() };
            let goals_val = if use_colors { plan.goals.len().to_string().bold().to_string() } else { plan.goals.len().to_string() };
            
            println!("📋 Farmland Plan:");
            println!();
            println!("   Farmfile:   {}", farmfile_val);
            println!("   Variants:   {}", variants_val);
            println!("   Operations: {}", ops_val);
            println!("   Goals:      {}", goals_val);
            println!();
            
            if verbose {
                println!("{}", runtime_plan.pretty_print_graph_verbose_colored(use_colors));
            } else {
                println!("{}", runtime_plan.pretty_print_graph_colored(use_colors));
            }
        }
        "list" => {
            let variants_str = if plan.variants.is_empty() { 
                "default".to_string() 
            } else { 
                plan.variants.join(", ") 
            };
            
            // Format values with bold if terminal supports it
            let farmfile_val = if use_colors { farmfile.display().to_string().bold().to_string() } else { farmfile.display().to_string() };
            let variants_val = if use_colors { variants_str.bold().to_string() } else { variants_str };
            let ops_val = if use_colors { plan.operations.len().to_string().bold().to_string() } else { plan.operations.len().to_string() };
            let goals_val = if use_colors { plan.goals.len().to_string().bold().to_string() } else { plan.goals.len().to_string() };
            
            println!("📋 Farmland Plan:");
            println!();
            println!("   Farmfile:   {}", farmfile_val);
            println!("   Variants:   {}", variants_val);
            println!("   Operations: {}", ops_val);
            println!("   Goals:      {}", goals_val);
            println!();
            for (i, stage) in plan.operations.iter().enumerate() {
                let depends_str = if stage.depends.is_empty() {
                    "none".to_string()
                } else {
                    stage.depends.join(", ")
                };
                
                // Print operation with goal indicator if applicable (bold)
                if let Some(goal_name) = goals_by_operation.get(stage.label.as_str()) {
                    if use_colors {
                        println!("  {}. {} → {} (depends: {})", i + 1, stage.label, goal_name.yellow().bold(), depends_str);
                    } else {
                        println!("  {}. {} → {} (depends: {})", i + 1, stage.label, goal_name, depends_str);
                    }
                } else {
                    println!("  {}. {} (depends: {})", i + 1, stage.label, depends_str);
                }
                if !stage.tasks.is_empty() {
                    println!("     work: {} task(s)", stage.tasks.len());
                }
                println!();
            }
        }
        "dot" => {
            use petgraph::dot::{Dot, Config};
            println!("{:?}", Dot::with_config(plan.get_graph(), &[Config::EdgeNoLabel]));
        }
        "json" => {
            let json = serde_json::to_string_pretty(&plan)
                .map_err(|e| format!("Failed to serialize plan to JSON: {}", e))?;
            println!("{}", json);
        }
        "structure" => {
            // Safe structural output: only labels, dependencies, variants, and goals.
            // Deliberately omits tasks/commands which may contain secrets or
            // embedded environment variables. Designed for machine consumption
            // (e.g., CI announce, UI discovery).
            //
            // Goals map user-facing names to internal operations. Only operations
            // with a goal are triggerable from the UI.
            let structure = serde_json::json!({
                "variants": plan.variants,
                "operations": plan.operations.iter().map(|op| {
                    serde_json::json!({
                        "label": op.label,
                        "depends": op.depends,
                        "task_count": op.tasks.len(),
                        "goal": goals_by_operation.get(op.label.as_str()),
                    })
                }).collect::<Vec<_>>(),
            });
            let json = serde_json::to_string_pretty(&structure)
                .map_err(|e| format!("Failed to serialize structure to JSON: {}", e))?;
            println!("{}", json);
        }
        _ => {
            return Err(format!("Unknown format: {}. Use 'tree', 'list', 'dot', 'json', or 'structure'", format).into());
        }
    }
    
    Ok(())
}

