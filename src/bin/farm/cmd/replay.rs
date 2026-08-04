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
        eprintln!("   FARM_REPLAY_OF={} FARM_BUILD_DIR={} farm run {} --variant {} -f {}",
            work.build_id,
            build_dir.display(),
            work.goal,
            if work.variant.is_empty() { "default" } else { &work.variant },
            farmfile.display()
        );
        return Ok(());
    }

    // Execute farm run with environment variables set.
    //
    // `FARM_BUILD_ID` is deliberately *not* forwarded. Doing so would put the
    // replay in the source run's directory and overwrite the very run being
    // replayed — the clobbering ADR 0001 exists to prevent. A replay is a new
    // attempt: it gets its own run directory and its own entry in the history,
    // with `FARM_REPLAY_OF` recording where it came from.
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
        .env("FARM_REPLAY_OF", &work.build_id)
        .env("FARM_BUILD_DIR", &build_dir)
        .env_remove("FARM_BUILD_ID")
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
/// - Full path: `/path/to/.farm/run/build-id`
/// - Relative path: `.farm/run/build-id`
/// - A build ID: the matching `.farm/run/{build-id}`
/// - A goal name: the most recent run of that goal
///
/// The goal-name form is what keeps `farm replay test` working. Before ADR 0001
/// the local build ID *was* the goal name, so users typed the goal; now that run
/// directories are per-invocation, that has to be resolved through the manifests
/// rather than assumed from the directory name.
pub(crate) fn resolve_replay_build_dir(input: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let path = PathBuf::from(input);

    // If it's an absolute path or starts with . or contains path separators, use as-is
    if path.is_absolute() || input.starts_with('.') || input.contains('/') || input.contains('\\') {
        return Ok(path.canonicalize().unwrap_or(path));
    }

    let cwd = std::env::current_dir()?;
    let farm_dir = cwd.join(".farm");

    // Build ID first, then newest run of a goal by that name.
    if let Some(run) = farm::runs::resolve(&farm_dir, input, None) {
        return Ok(run.dir);
    }

    // Nothing matched. Name what *is* available rather than just failing —
    // build IDs are generated now, so the user cannot be expected to guess one.
    let available = farm::runs::list_runs(&farm_dir);
    if available.is_empty() {
        return Err(format!("No run found for '{}', and no runs are recorded yet", input).into());
    }

    let mut known: Vec<String> = available
        .iter()
        .filter_map(|run| run.goal().map(|g| g.to_string()))
        .collect();
    known.dedup();

    Err(format!(
        "No run found for '{}'. Recorded goals: {}",
        input,
        known.join(", ")
    )
    .into())
}

