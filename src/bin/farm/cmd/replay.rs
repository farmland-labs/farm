//! SPDX-License-Identifier: MIT OR Apache-2.0

//! `farm replay` command.

use std::path::PathBuf;

/// Minimal work.json structure for replay command.
/// We only need goal and variant to replay a build.
#[derive(serde::Deserialize)]
struct ReplayWorkJson {
    goal: String,
    #[serde(default)]
    variant: String,
    build_id: String,
}

/// Handle the `farm replay` command.
///
/// Re-runs a build from a preserved ops directory by:
/// 1. Parsing work.json to get target and variant
/// 2. Reading manifest.json for validation (if available)
/// 3. Setting FARM_BUILD_DIR and FARM_BUILD_ID environment variables
/// 4. Executing `farm run {target} --variant {variant}`
pub(crate) fn handle_replay_command(
    build_dir_arg: &str, 
    dry_run: bool,
    farmfile_override: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    use farm::manifest::ReplayManifest;
    
    // Resolve build directory path
    // Support: full path, relative path, or just build-id
    let build_dir = resolve_replay_build_dir(build_dir_arg)?;
    
    // Validate build directory exists
    if !build_dir.exists() {
        return Err(format!("Build directory not found: {}", build_dir.display()).into());
    }
    
    // Read work.json
    let work_json_path = build_dir.join("work.json");
    if !work_json_path.exists() {
        return Err(format!(
            "work.json not found in {}. Is this a valid build directory?",
            build_dir.display()
        ).into());
    }
    
    let work_json_content = std::fs::read_to_string(&work_json_path)?;
    let work: ReplayWorkJson = serde_json::from_str(&work_json_content)
        .map_err(|e| format!("Failed to parse work.json: {}", e))?;
    
    // Determine Farmfile path
    // Priority: --farmfile flag > workspace/Farmfile > current dir Farmfile
    let workspace = build_dir
        .parent() // ops
        .and_then(|p| p.parent()) // .farm
        .and_then(|p| p.parent()) // workspace
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    
    let farmfile = farmfile_override.unwrap_or_else(|| {
        let workspace_farmfile = workspace.join("Farmfile");
        if workspace_farmfile.exists() {
            workspace_farmfile
        } else {
            PathBuf::from("farmfile")
        }
    });
    
    // Show replay info
    eprintln!("🔄 Replaying build");
    eprintln!("   Build ID:   {}", work.build_id);
    eprintln!("   Farmfile:   {}", farmfile.display());
    eprintln!("   Goal:       {}", work.goal);
    eprintln!("   Variant:    {}", if work.variant.is_empty() { "default" } else { &work.variant });
    eprintln!("   Build dir:  {}", build_dir.display());
    
    // Read and display manifest info if available
    if let Ok(manifest) = ReplayManifest::read_from(&build_dir) {
        eprintln!();
        eprintln!("   📋 Manifest info:");
        if let Some(ref vcs_ref) = manifest.vcs_ref {
            match vcs_ref {
                farm::manifest::VcsRef::Git { commit, ref_name } => {
                    let short_commit = &commit[..7.min(commit.len())];
                    eprintln!("      VCS:        git @ {}", short_commit);
                    if let Some(ref name) = ref_name {
                        eprintln!("      Ref:        {}", name);
                    }
                }
            }
        }
        eprintln!("      Status:     {}", if manifest.success { "✅ SUCCESS" } else { "❌ FAILED" });
        eprintln!("      Duration:   {}ms", manifest.duration_ms);
        eprintln!("      Operations: {}", manifest.operations.len());
        eprintln!("      Created:    {}", manifest.created_at);
        
        // Validate environment and show warnings
        let warnings = manifest.validate_environment(&workspace);
        if !warnings.is_empty() {
            eprintln!();
            eprintln!("   ⚠️  Environment warnings:");
            for warning in warnings.iter().take(5) {
                eprintln!("      - {}", warning);
            }
            if warnings.len() > 5 {
                eprintln!("      ... and {} more", warnings.len() - 5);
            }
        }
    }
    
    // Check for context.json (pipeline build indicator)
    let context_json_path = build_dir.join("context.json");
    if context_json_path.exists() {
        eprintln!();
        eprintln!("   📦 Pipeline context available (context.json found)");
    }
    eprintln!();
    
    if dry_run {
        eprintln!("🔍 Dry run - would execute:");
        eprintln!("   FARM_BUILD_ID={} FARM_BUILD_DIR={} farm run {} --variant {} -f {}",
            work.build_id,
            build_dir.display(),
            work.goal,
            if work.variant.is_empty() { "default" } else { &work.variant },
            farmfile.display()
        );
        return Ok(());
    }
    
    // Execute farm run with environment variables set
    use std::process::Command;
    
    let status = Command::new(std::env::current_exe()?)
        .args([
            "run",
            &work.goal,
            "--variant",
            if work.variant.is_empty() { "default" } else { &work.variant },
            "-f",
            farmfile.to_str().unwrap_or("Farmfile"),
        ])
        .env("FARM_BUILD_ID", &work.build_id)
        .env("FARM_BUILD_DIR", &build_dir)
        .current_dir(&workspace)
        .status()
        .map_err(|e| format!("Failed to execute farm run: {}", e))?;
    
    if !status.success() {
        return Err(format!("Build replay failed with exit code: {:?}", status.code()).into());
    }
    
    eprintln!("✅ Replay completed successfully");
    Ok(())
}

/// Resolve the build directory path from user input.
///
/// Supports:
/// - Full path: /path/to/.farm/ops/build-id
/// - Relative path: .farm/ops/build-id
/// - Just build-id: looks in .farm/ops/{build-id} in current directory
pub(crate) fn resolve_replay_build_dir(input: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let path = PathBuf::from(input);
    
    // If it's an absolute path or starts with . or contains path separators, use as-is
    if path.is_absolute() || input.starts_with('.') || input.contains('/') || input.contains('\\') {
        return Ok(path.canonicalize().unwrap_or(path));
    }
    
    // Otherwise, treat as build-id and look in .farm/ops/
    let cwd = std::env::current_dir()?;
    let ops_path = cwd.join(".farm").join("ops").join(input);
    
    if ops_path.exists() {
        return Ok(ops_path);
    }
    
    // Fall back to treating as relative path
    Ok(path)
}

