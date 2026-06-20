//! SPDX-License-Identifier: MIT OR Apache-2.0

//! `farm run` execution command.

use farm::vendor::log::{info, warn};

use crate::cli::Args;

/// Handle the `farm cache` command.
pub(crate) fn handle_execute_command(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    // Read and parse the Farmfile
    let content = std::fs::read_to_string(&args.farmfile)
        .map_err(|e| format!("❌ Failed to read config file {}: {}", args.farmfile.display(), e))?;
    
    let parsed_plan = match farm::vendor::parse::parse_plan(&content) {
        Ok(plan) => plan,
        Err(e) => {
            // Display our rich error message with line numbers and context
            eprintln!("❌ Failed to parse config file {}:", args.farmfile.display());
            eprintln!("{}", e);
            std::process::exit(1);
        }
    };

    let plan = farm::plan::Plan::from_parsed(parsed_plan);
    info!("Parsed plan with {} operations", plan.stages.len());
    
    // Resolve variant with precedence: CLI arg > env var > default
    let variant = if args.variant != "default" {
        // CLI argument was explicitly set (not default)
        if let Ok(env_variant) = std::env::var("FARM_VARIANT") {
            warn!("FARM_VARIANT environment variable set to '{}' but command-line argument '--variant {}' takes precedence", 
                  env_variant, args.variant);
        }
        args.variant.clone()
    } else if let Ok(env_variant) = std::env::var("FARM_VARIANT") {
        info!("Using variant from FARM_VARIANT environment variable: {}", env_variant);
        env_variant
    } else {
        args.variant.clone()
    };
    
    // Resolve target with precedence: CLI arg > env var > none
    let target = if args.target.is_some() {
        if let Ok(env_target) = std::env::var("FARM_TARGET") {
            warn!("FARM_TARGET environment variable set to '{}' but command-line argument '--target {}' takes precedence", 
                  env_target, args.target.as_ref().unwrap());
        }
        args.target
    } else if let Ok(env_target) = std::env::var("FARM_TARGET") {
        info!("Using target from FARM_TARGET environment variable: {}", env_target);
        Some(env_target)
    } else {
        None
    };
    
    // Check if target is specified
    let target = match target {
        Some(target) => target,
        None => {
            // No target specified - this is an identity/no-op operation
            info!("No target specified, performing identity operation");
            println!("✅ Farm operation completed successfully!");
            println!("   Farmfile: {}", args.farmfile.display());
            println!("   Target: (none)  Variant: {}  Duration: 0ms", variant);
            println!("   Farmfile validated successfully with {} operations available", plan.stages.len());
            println!("   Available operations:");
            for stage in &plan.stages {
                println!("     - {}", stage.label);
            }
            return Ok(());
        }
    };
    
    // Resolve target: if it's a goal name, map it to the operation label
    let resolved_target = plan.resolve_target(&target);
    if resolved_target != target {
        info!("Resolved goal '{}' to operation '{}'", target, resolved_target);
    }
    let target = resolved_target;
    
    // Create executor with current directory as workspace
    let workspace = std::env::current_dir()?;
    let executor = match args.farm_dir {
        Some(farm_dir) => farm::engine::Executor::with_farm_dir(workspace, farm_dir),
        None => farm::engine::Executor::new(workspace),
    };
    
    // Validate log_output argument
    if !["file", "file-split", "stdout", "none"].contains(&args.log_output.as_str()) {
        eprintln!("❌ Invalid --log-output value: '{}'. Must be 'file', 'file-split', 'stdout', or 'none'", args.log_output);
        std::process::exit(1);
    }
    
    // Execute the plan (skip_deps=true when --only/-1 is specified)
    match executor.execute_plan(&plan, &target, &variant, args.silent, &args.log_output, args.only, args.split_streams, args.no_cache, args.build_id.as_deref()) {
        Ok(result) => {
            if result.success {
                println!("✅ Farm operation completed successfully!");
                println!("   Farmfile: {}  Target: {}  Variant: {}", args.farmfile.display(), target, variant);
                println!("   Executed operations in {}ms", result.total_duration_ms);
                for stage_result in &result.executed_stages {
                    let key_info = stage_result.cache_key.as_ref()
                        .map(|k| {
                            if args.no_cache {
                                format!(" key={} (cache disabled)", k)
                            } else {
                                format!(" key={}", k)
                            }
                        })
                        .unwrap_or_default();
                    let status = if stage_result.cache_hit {
                        "⚡"
                    } else if stage_result.success {
                        "✓"
                    } else {
                        "✗"
                    };
                    println!("    - {} ({}): {} {}ms{}", 
                             stage_result.stage_label,
                             stage_result.variant,
                             status,
                             stage_result.duration_ms,
                             key_info);
                }
            } else {
                eprintln!("❌ Farm operation failed");
                eprintln!("   Farmfile: {}", args.farmfile.display());
                eprintln!("   Duration: {}ms", result.total_duration_ms);
                std::process::exit(1);
            }
        }
        Err(e) => {
            eprintln!("❌ Farm operation failed: {}", e);
            std::process::exit(1);
        }
    }

    Ok(())
}

