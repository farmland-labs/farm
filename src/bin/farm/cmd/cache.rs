//! SPDX-License-Identifier: MIT OR Apache-2.0

//! `farm cache` subcommands.

use clap::Subcommand;

#[derive(Subcommand, Clone)]
pub(crate) enum CacheCommands {
    /// List all cached targets
    List {
        /// Output format: short (default) or long (full target keys)
        #[arg(long, default_value = "short")]
        format: String,
    },
    
    /// Get a cache entry by target key (prefix or full)
    Get {
        /// Target key prefix (e.g., '1e16b85d') or full key
        key: String,
    },
    
    /// Show cache statistics
    Stats,
    
    /// Clear all cache entries
    Clear {
        /// Skip confirmation prompt
        #[arg(long, short = 'y')]
        yes: bool,
    },
    
    /// Remove cache entries by target key or operation name
    Remove {
        /// Target key prefix or full key (use `farm cache list` to see entries)
        #[arg(conflicts_with = "operation")]
        key: Option<String>,
        
        /// Remove by operation name (format: OP or OP:VARIANT)
        #[arg(long)]
        operation: Option<String>,
    },
    
    /// Check input/output declarations between dependent operations
    Check {
        /// Operations to check in dependency order (e.g., config build test)
        operations: Vec<String>,
    },
}

pub(crate) fn handle_cache_command(command: CacheCommands) -> Result<(), Box<dyn std::error::Error>> {
    use farm::cache::CacheStore;
    
    let workspace = std::env::current_dir()?;
    let cache_dir = workspace.join(".farm").join("cache");
    
    match command {
        CacheCommands::List { format } => {
            let long_format = format == "long";
            
            if !cache_dir.exists() {
                println!("📦 Cache is empty (no cache directory found)");
                return Ok(());
            }
            
            let store = CacheStore::new(&cache_dir)?;
            let entries = store.list_entries();
            
            if entries.is_empty() {
                println!("📦 Cache is empty");
                return Ok(());
            }
            
            println!("📦 Cached targets ({} entries):", entries.len());
            println!();
            
            let mut entries = entries;
            // Sort by stage name first, then by age (newest first within each stage)
            entries.sort_by(|a, b| {
                let stage_a = a.stage_name.as_deref().unwrap_or("");
                let stage_b = b.stage_name.as_deref().unwrap_or("");
                match stage_a.cmp(stage_b) {
                    std::cmp::Ordering::Equal => b.created_at.cmp(&a.created_at),
                    other => other,
                }
            });
            
            // Print header - key width depends on format
            let key_width = if long_format { 64 } else { 12 };
            println!("  {:<width$}   {:<20}   {:>8}   {:>8}   STATUS", "TARGET KEY", "OPERATION", "AGE", "DURATION", width = key_width);
            println!("  {}   {}   {}   {}   {}", "-".repeat(key_width), "-".repeat(20), "-".repeat(8), "-".repeat(8), "-".repeat(10));
            
            for entry in entries {
                let age = chrono::Utc::now().signed_duration_since(entry.created_at);
                let age_str = if age.num_days() > 0 {
                    format!("{}d ago", age.num_days())
                } else if age.num_hours() > 0 {
                    format!("{}h ago", age.num_hours())
                } else {
                    format!("{}m ago", age.num_minutes())
                };
                
                // Format operation as OP:VARIANT (omit :default)
                let op_info = match (&entry.stage_name, &entry.stage_variant) {
                    (Some(name), Some(variant)) if variant != "default" => format!("{}:{}", name, variant),
                    (Some(name), _) => name.clone(),
                    _ => "-".to_string(),
                };
                
                // Parse duration from content_hash (format: "stage:duration_ms")
                let duration_str = entry.content_hash
                    .split(':')
                    .next_back()
                    .and_then(|s| s.parse::<u64>().ok())
                    .map(|ms| format!("{}ms", ms))
                    .unwrap_or_else(|| "-".to_string());
                
                // Show short (12 char) or full key based on format
                let display_key = if long_format {
                    entry.target_key.clone()
                } else if entry.target_key.len() > 12 {
                    entry.target_key[..12].to_string()
                } else {
                    entry.target_key.clone()
                };
                
                // Status indicator
                let status = if entry.invalidated_at.is_some() {
                    "⚠️ invalid"
                } else {
                    "✓ valid"
                };
                
                println!("  {:<width$}   {:<20}   {:>8}   {:>8}   {}", display_key, op_info, age_str, duration_str, status, width = key_width);
            }
            println!();
            if !long_format {
                println!("Use `farm cache list --format=long` to see full target keys");
            }
            println!("Use `farm cache get <key>` to view an entry, `farm cache remove <key>` to remove");
        }
        
        CacheCommands::Get { key } => {
            if !cache_dir.exists() {
                return Err("Cache directory not found".into());
            }
            
            let store = CacheStore::new(&cache_dir)?;
            let entries = store.list_entries();
            
            // Find entries matching the key prefix
            let matching: Vec<_> = entries.iter()
                .filter(|e| e.target_key.starts_with(&key))
                .collect();
            
            match matching.len() {
                0 => {
                    return Err(format!("No cache entry found with key prefix '{}'", key).into());
                }
                1 => {
                    let entry = matching[0];
                    
                    // Show invalidation warning at top if applicable
                    if entry.invalidated_at.is_some() {
                        println!("⚠️  INVALIDATED Cache Entry (kept for post-mortem)");
                    } else {
                        println!("📦 Cache Entry");
                    }
                    println!();
                    println!("  Target Key:   {}", entry.target_key);
                    
                    let op_info = match (&entry.stage_name, &entry.stage_variant) {
                        (Some(name), Some(variant)) if variant != "default" => format!("{}:{}", name, variant),
                        (Some(name), _) => name.clone(),
                        _ => "-".to_string(),
                    };
                    println!("  Operation:    {}", op_info);
                    
                    let age = chrono::Utc::now().signed_duration_since(entry.created_at);
                    let age_str = if age.num_days() > 0 {
                        format!("{}d ago", age.num_days())
                    } else if age.num_hours() > 0 {
                        format!("{}h ago", age.num_hours())
                    } else {
                        format!("{}m ago", age.num_minutes())
                    };
                    println!("  Created:      {} ({})", entry.created_at.format("%Y-%m-%d %H:%M:%S UTC"), age_str);
                    
                    // Parse duration from content_hash
                    let duration_str = entry.content_hash
                        .split(':')
                        .next_back()
                        .and_then(|s| s.parse::<u64>().ok())
                        .map(|ms| format!("{}ms", ms))
                        .unwrap_or_else(|| "-".to_string());
                    println!("  Duration:     {}", duration_str);
                    
                    if let Some(ref parcel) = entry.parcel_ref {
                        println!("  Parcel:       {}", parcel);
                    }
                    println!("  Content Hash: {}", entry.content_hash);
                    
                    // Display VCS metadata if present
                    if let Some(ref vcs) = entry.vcs {
                        println!();
                        println!("  VCS ({}):", vcs.vcs_type);
                        if let Some(ref short) = vcs.revision_short {
                            if let Some(ref full) = vcs.revision {
                                println!("    Revision:   {} ({})", short, full);
                            } else {
                                println!("    Revision:   {}", short);
                            }
                        }
                        if let Some(ref branch) = vcs.branch {
                            println!("    Branch:     {}", branch);
                        }
                        if let Some(ref date) = vcs.commit_date {
                            println!("    Date:       {}", date);
                        }
                        if let Some(ref msg) = vcs.message {
                            // Truncate long messages
                            let display_msg = if msg.len() > 60 {
                                msg[..57].to_string()
                            } else {
                                msg.clone()
                            };
                            println!("    Message:    {}", display_msg);
                        }
                        if let Some(dirty) = vcs.dirty {
                            println!("    Dirty:      {}", if dirty { "yes ⚠️" } else { "no" });
                        }
                        if let Some(ref tag) = vcs.tag {
                            println!("    Tag:        {}", tag);
                        }
                    }
                    
                    // Display invalidation info if entry was soft-deleted
                    if let Some(ref invalidated_at) = entry.invalidated_at {
                        println!();
                        println!("  ⚠️  Invalidation:");
                        let age = chrono::Utc::now().signed_duration_since(*invalidated_at);
                        let age_str = if age.num_days() > 0 {
                            format!("{}d ago", age.num_days())
                        } else if age.num_hours() > 0 {
                            format!("{}h ago", age.num_hours())
                        } else {
                            format!("{}m ago", age.num_minutes())
                        };
                        println!("    When:       {} ({})", invalidated_at.format("%Y-%m-%d %H:%M:%S UTC"), age_str);
                        if let Some(ref reason) = entry.invalidation_reason {
                            println!("    Reason:     {}", reason);
                        }
                    }
                }
                _ => {
                    println!("Multiple entries match '{}':", key);
                    for e in matching {
                        let stage_info = match (&e.stage_name, &e.stage_variant) {
                            (Some(name), Some(variant)) if variant != "default" => format!("{}:{}", name, variant),
                            (Some(name), _) => name.clone(),
                            _ => "-".to_string(),
                        };
                        println!("  {}  {}", e.target_key, stage_info);
                    }
                    return Err("Please provide a more specific key prefix".into());
                }
            }
        }
        
        CacheCommands::Stats => {
            if !cache_dir.exists() {
                println!("📊 Cache Statistics");
                println!("   Entries:    0");
                println!("   Cache dir:  {} (not created yet)", cache_dir.display());
                return Ok(());
            }
            
            let store = CacheStore::new(&cache_dir)?;
            let entries = store.list_entries();
            
            // Get cache file size
            let cache_file = cache_dir.join("goal-cache.json");
            let cache_size = std::fs::metadata(&cache_file)
                .map(|m| m.len())
                .unwrap_or(0);
            
            let size_str = if cache_size > 1024 * 1024 {
                format!("{:.1} MB", cache_size as f64 / (1024.0 * 1024.0))
            } else if cache_size > 1024 {
                format!("{:.1} KB", cache_size as f64 / 1024.0)
            } else {
                format!("{} B", cache_size)
            };
            
            // Find oldest and newest entries
            let (oldest, newest) = if !entries.is_empty() {
                let mut sorted = entries.clone();
                sorted.sort_by(|a, b| a.created_at.cmp(&b.created_at));
                (Some(sorted.first().unwrap().created_at), Some(sorted.last().unwrap().created_at))
            } else {
                (None, None)
            };
            
            println!("📊 Cache Statistics");
            println!("   Entries:    {}", entries.len());
            println!("   Index size: {}", size_str);
            println!("   Cache dir:  {}", cache_dir.display());
            if let Some(oldest) = oldest {
                println!("   Oldest:     {}", oldest.format("%Y-%m-%d %H:%M:%S UTC"));
            }
            if let Some(newest) = newest {
                println!("   Newest:     {}", newest.format("%Y-%m-%d %H:%M:%S UTC"));
            }
        }
        
        CacheCommands::Clear { yes } => {
            if !cache_dir.exists() {
                println!("📦 Cache is already empty");
                return Ok(());
            }
            
            let store = CacheStore::new(&cache_dir)?;
            let count = store.len();
            
            if count == 0 {
                println!("📦 Cache is already empty");
                return Ok(());
            }
            
            if !yes {
                println!("⚠️  This will remove {} cache entries.", count);
                print!("Continue? [y/N] ");
                use std::io::{Write, BufRead};
                std::io::stdout().flush()?;
                
                let stdin = std::io::stdin();
                let mut input = String::new();
                stdin.lock().read_line(&mut input)?;
                
                if !input.trim().eq_ignore_ascii_case("y") {
                    println!("Aborted.");
                    return Ok(());
                }
            }
            
            store.clear()?;
            println!("✅ Cleared {} cache entries", count);
        }
        
        CacheCommands::Remove { key, operation } => {
            if !cache_dir.exists() {
                return Err("Cache directory not found".into());
            }
            
            let store = CacheStore::new(&cache_dir)?;
            let entries = store.list_entries();
            
            if let Some(op_spec) = operation {
                // Remove by operation name (OP or OP:VARIANT)
                let (op_name, variant_filter) = if let Some(pos) = op_spec.find(':') {
                    (&op_spec[..pos], Some(&op_spec[pos+1..]))
                } else {
                    (op_spec.as_str(), None)
                };
                
                let by_op: Vec<_> = entries.iter()
                    .filter(|e| {
                        let name_match = e.stage_name.as_deref() == Some(op_name);
                        let variant_match = variant_filter.is_none_or(|v| {
                            e.stage_variant.as_deref() == Some(v)
                        });
                        name_match && variant_match
                    })
                    .collect();
                
                if by_op.is_empty() {
                    return Err(format!("No cache entry found for operation '{}'", op_spec).into());
                }
                
                for entry in &by_op {
                    store.remove(&entry.target_key)?;
                }
                println!("✅ Removed {} cache entr{} for operation '{}'",
                    by_op.len(),
                    if by_op.len() == 1 { "y" } else { "ies" },
                    op_spec
                );
            } else if let Some(key_prefix) = key {
                // Remove by target key prefix
                let keys = store.list_keys();
                let matching: Vec<_> = keys.iter()
                    .filter(|k| k.starts_with(&key_prefix))
                    .collect();
                
                match matching.len() {
                    0 => {
                        return Err(format!("No cache entry found with key prefix '{}'", key_prefix).into());
                    }
                    1 => {
                        let full_key = matching[0];
                        store.remove(full_key)?;
                        println!("✅ Removed cache entry: {}", full_key);
                    }
                    _ => {
                        println!("Multiple entries match '{}':", key_prefix);
                        for k in matching {
                            println!("  {}", k);
                        }
                        return Err("Please provide a more specific key prefix".into());
                    }
                }
            } else {
                return Err("Either KEY or --operation OP is required".into());
            }
        }
        
        CacheCommands::Check { operations } => {
            use farm::cache::check_dependency_declarations;
            
            if operations.len() < 2 {
                return Err("At least two operations required (e.g., 'farm cache check config build')".into());
            }
            
            // Find and parse the farmfile
            let farmfile_path = workspace.join("Farmfile");
            if !farmfile_path.exists() {
                return Err(format!(
                    "No Farmfile found at {}. Use -f to specify a different file.",
                    farmfile_path.display()
                ).into());
            }
            
            let content = std::fs::read_to_string(&farmfile_path)?;
            let plan = farm::vendor::parse::parse_plan(&content)
                .map_err(|e| format!("Failed to parse Farmfile: {}", e))?;
            
            println!("🔍 Checking input/output declarations for operations: {}", operations.join(" → "));
            println!();
            
            let result = check_dependency_declarations(&plan, &operations);
            
            // Show info messages
            for msg in &result.info {
                println!("ℹ️  {}", msg);
            }
            if !result.info.is_empty() {
                println!();
            }
            
            // Show warnings
            if result.warnings.is_empty() {
                println!("✅ No issues found. Declaration coverage looks good!");
            } else {
                println!("⚠️  Found {} potential issue(s):", result.warnings.len());
                println!();
                
                for (i, warning) in result.warnings.iter().enumerate() {
                    println!("{}. {}", i + 1, warning.format());
                    println!();
                }
                
                println!("💡 Tip: These are warnings, not errors. If a downstream operation");
                println!("   doesn't actually read the upstream output, you can safely ignore");
                println!("   the warning. If it does, add the output to the downstream input.");
            }
        }
    }
    
    Ok(())
}

