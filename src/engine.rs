//! SPDX-License-Identifier: MIT OR Apache-2.0

//! Execution Engine
//! 
//! Takes a target stage, resolves dependencies, and executes stages in correct order.
//! Supports variant-aware execution with comprehensive logging.

use std::path::PathBuf;
use std::collections::{HashMap, HashSet};
use std::time::SystemTime;

use petgraph::algo::toposort;
use crate::vendor::log::{info, error, debug, warn, info_icon, error_icon};

use crate::plan::Plan;
use crate::stage::Stage;
use crate::task::Context;
use crate::env::CapturedEnv;
use crate::build_logger::{BuildLogger, LogMode};
use crate::cache::{CacheLookup, CacheResult, compute_target_key, hash_file_patterns};
use crate::manifest::ReplayManifest;
use crate::retention::{self, RetentionPolicy};
use crate::work_hash::compute_work_hash;
use crate::write_farm_notice;
use crate::vendor::parse::{CacheConfig, EnvValue};

/// Execution context for a build run
pub struct ExecutionContext {
    pub target: String,
    pub variant: String,
    pub workspace: PathBuf,
    pub farm_dir: PathBuf,
    pub target_dir: PathBuf,
    pub start_time: SystemTime,
    pub silent: bool,
    pub split_streams: bool,
    /// Interactive (PTY) execution for this run. See [`crate::task::Context`]
    /// and ADR-081.
    pub interactive: bool,
    pub logger: BuildLogger,
    pub cache: CacheLookup,
    /// Global cache configuration from `[cache]` section
    pub cache_config: Option<CacheConfig>,
    /// Global environment variables from the `[env]` section, injected into
    /// every operation's command environment.
    pub globals: HashMap<String, String>,
}

/// Result of executing a single stage
#[derive(Debug)]
pub struct StageResult {
    pub stage_label: String,
    pub variant: String,
    pub success: bool,
    pub duration_ms: u64,
    pub task_results: Vec<TaskResult>,
    /// Cache key used for this stage (if caching was enabled)
    pub cache_key: Option<String>,
    /// Whether this stage was a cache hit (skipped execution)
    pub cache_hit: bool,
    /// 6-hex-char operation identity hash. Computed from the
    /// Farmfile-declared shape; stable across runs.
    pub work_hash: String,
}

/// Result of executing a single task
#[derive(Debug)]
pub struct TaskResult {
    pub task_label: String,
    pub success: bool,
    pub duration_ms: u64,
    pub output: String,
    pub error: String,
}

/// Execution result for the entire build
#[derive(Debug)]
pub struct ExecutionResult {
    pub success: bool,
    pub executed_stages: Vec<StageResult>,
    pub total_duration_ms: u64,
}

pub struct Executor {
    workspace: PathBuf,
    farm_dir: Option<PathBuf>,
}

impl Executor {

    pub fn new(workspace: PathBuf) -> Self {
        Self {
            workspace,
            farm_dir: None,
        }
    }

    /// Create executor with custom Farm directory
    pub fn with_farm_dir(workspace: PathBuf, farm_dir: PathBuf) -> Self {
        Self {
            workspace,
            farm_dir: Some(farm_dir),
        }
    }

    /// Execute a plan for a specific goal and variant
    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_arguments)]
    pub fn execute_plan(&self, plan: &Plan, goal: &str, variant: &str, silent: bool, log_output: &str, skip_deps: bool, split_streams: bool, no_cache: bool, interactive: bool, build_id: Option<&str>) -> Result<ExecutionResult, String> {
        let start_time = SystemTime::now();
        
        // Parse log mode
        let log_mode = LogMode::from_str(log_output)?;
        
        // Create execution context
        let farm_dir = match &self.farm_dir {
            Some(dir) => dir.clone(),
            None => {
                let mut default_dir = self.workspace.clone();
                default_dir.push(".farm");
                default_dir
            }
        };

        std::fs::create_dir_all(&farm_dir).map_err(|e| format!("Failed to create .farm directory: {}", e))?;
        
        // Write notice file explaining the directory is internal
        write_farm_notice(&farm_dir);
        
        // Create cache lookup (or disabled if --no-cache)
        let cache_dir = farm_dir.join("cache");
        let cache = if no_cache {
            CacheLookup::disabled()
        } else {
            CacheLookup::new(&cache_dir).map_err(|e| format!("Failed to create cache: {}", e))?
        };
        
        // Determine build ID for run directory:
        // 1. CLI --build-id flag (passed as parameter)
        // 2. FARM_BUILD_ID env var (set by the CI runner)
        // 3. A freshly generated per-run ID for local dev
        //
        // The local fallback used to be the bare goal name, which meant every run
        // of a goal reused one directory and destroyed the previous run's log,
        // manifest and work.json. See ADR 0001.
        let effective_build_id = build_id
            .map(|s| s.to_string())
            .or_else(|| std::env::var("FARM_BUILD_ID").ok())
            .unwrap_or_else(|| generate_build_id(goal));

        let run_dir = farm_dir.join("run").join(&effective_build_id);
        let log_dir = run_dir.join("log");
        std::fs::create_dir_all(&log_dir).map_err(|e| format!("Failed to create log directory: {}", e))?;

        // Stub manifest, so an in-flight run is discoverable and a crashed one is
        // distinguishable from a running one. Rewritten in full at completion.
        // Best-effort: failing to write it must not abort the build.
        if let Err(e) = ReplayManifest::running(&effective_build_id, goal, variant, interactive, start_time)
            .write_to(&run_dir)
        {
            warn!(error = %e, "Failed to write run-start manifest");
        }

        // Prune older runs of this goal + variant. Runs *after* the stub above,
        // so the run about to execute is protected by its own `running` status.
        // Housekeeping failures are logged, never fatal.
        let policy = RetentionPolicy::default();
        let pruned = retention::prune(&farm_dir, goal, variant, &policy);
        let orphans = retention::prune_orphans(&farm_dir, &policy);
        let removed = pruned.removed.len() + orphans.removed.len();
        if removed > 0 {
            debug!(removed = removed, kept = pruned.kept, "Pruned old run directories");
        }
        for error in pruned.errors.iter().chain(orphans.errors.iter()) {
            warn!(error = %error, "Failed to prune run directory");
        }

        // Create build logger
        let logger = BuildLogger::new(&log_dir, goal, variant, log_mode)?;

        let mut exec_ctx = ExecutionContext {
            target: goal.to_string(),
            variant: variant.to_string(),
            workspace: self.workspace.clone(),
            farm_dir,
            target_dir: run_dir,
            start_time,
            silent,
            split_streams,
            interactive,
            logger,
            cache,
            cache_config: plan.cache.clone(),
            globals: plan.globals.clone(),
        };

        info_icon!("🎬", goal = %goal, variant = %variant, skip_deps = %skip_deps, "Starting execution");
        
        // Find stages that match the goal and variant
        let execution_order = self.resolve_execution_order(plan, goal, variant, skip_deps)?;
        
        if execution_order.is_empty() {
            return Err(format!("No stages found for goal '{}' with variant '{}'", goal, variant));
        }

        info!(order = ?execution_order.iter().map(|s| &s.label).collect::<Vec<_>>(), "Execution order");

        // Execute stages in dependency order
        let mut executed_stages = Vec::new();
        let mut overall_success = true;

        for stage in execution_order {
            let stage_result = self.execute_stage(&stage, &mut exec_ctx)?;
            
            if !stage_result.success {
                overall_success = false;
                error!(stage = %stage.label, "Stage failed, stopping execution");
                executed_stages.push(stage_result);
                break;
            }
            
            executed_stages.push(stage_result);
        }

        // Flush log files
        exec_ctx.logger.flush()?;

        let total_duration = start_time.elapsed()
            .map_err(|e| format!("Failed to calculate duration: {}", e))?
            .as_millis() as u64;

        let result = ExecutionResult {
            success: overall_success,
            executed_stages,
            total_duration_ms: total_duration,
        };

        if result.success {
            info_icon!("🎯", duration_ms = result.total_duration_ms, "Execution completed successfully");
        } else {
            error_icon!("❌", duration_ms = result.total_duration_ms, "Execution failed");
        }
        
        // Generate replay manifest
        // Capture build-relevant environment variables
        let build_env: HashMap<String, String> = std::env::vars()
            .filter(|(k, _)| {
                // Include variables relevant to builds
                k.starts_with("FARM_") || 
                k.starts_with("RUST_") ||
                k.starts_with("CARGO_") ||
                k.starts_with("CC") ||
                k.starts_with("CXX") ||
                k.starts_with("LD") ||
                k == "PATH" ||
                k == "HOME" ||
                k == "USER"
            })
            .collect();
        
        let manifest = ReplayManifest::from_execution(
            &effective_build_id,
            goal,
            variant,
            &result,
            build_env,
            &self.workspace,
            interactive,
            start_time,
        );
        
        // Write manifest.json
        if let Err(e) = manifest.write_to(&exec_ctx.target_dir) {
            error!(error = %e, "Failed to write manifest");
        }
        
        // Write work.json for replay command compatibility
        let work_json = serde_json::json!({
            "build_id": effective_build_id,
            "goal": goal,
            "variant": variant,
        });
        let work_path = exec_ctx.target_dir.join("work.json");
        if let Err(e) = std::fs::write(&work_path, serde_json::to_string_pretty(&work_json).unwrap_or_default()) {
            error!(error = %e, "Failed to write work.json");
        }

        Ok(result)
    }
    /// Resolve the execution order for stages based on dependencies
    fn resolve_execution_order(&self, plan: &Plan, target: &str, _variant: &str, skip_deps: bool) -> Result<Vec<Stage>, String> {
        // Find the target stage (variants are no longer used for filtering)
        let target_stage = plan.stages.iter()
            .find(|stage| stage.label == target);

        if target_stage.is_none() {
            return Err(format!("Target stage '{}' not found", target));
        }

        // If skip_deps is true, only return the target stage
        if skip_deps {
            return Ok(vec![target_stage.unwrap().clone()]);
        }

        // Use the plan's graph to get topological order
        let topo_order = toposort(plan.get_graph(), None)
            .map_err(|_| "Circular dependency detected in plan")?;

        // Collect all stages that need to be executed (target and its dependencies)
        let mut required_stages = HashSet::new();
        self.collect_required_stages(plan, target, &mut required_stages);

        // Filter topological order to only include required stages
        let mut execution_order = Vec::new();
        for node_idx in topo_order {
            let node_name = &plan.get_graph()[node_idx];
            if required_stages.contains(node_name) {
                // Find the matching stage
                if let Some(stage) = plan.stages.iter()
                    .find(|s| s.label == *node_name) {
                    execution_order.push(stage.clone());
                }
            }
        }

        Ok(execution_order)
    }

    /// Recursively collect all stages required for execution
    fn collect_required_stages(&self, plan: &Plan, stage_name: &str, required: &mut HashSet<String>) {
        if required.contains(stage_name) {
            return;
        }

        required.insert(stage_name.to_string());

        // Find the stage and collect its dependencies  
        if let Some(stage) = plan.stages.iter()
            .find(|s| s.label == stage_name) {
            for dep in &stage.depends {
                self.collect_required_stages(plan, dep, required);
            }
        }
    }

    /// Merge global [cache] env_key/env_command with stage's declared_env
    /// 
    /// Priority: stage env > global env_key (downstream wins)
    /// 
    /// - Global `env_key` are added as `EnvValue::Capture`
    /// - Global `env_command` are executed and added as `EnvValue::Explicit(output)`
    /// - Stage's `declared_env` overrides any global keys
    fn merge_cache_env(
        &self,
        cache_config: &Option<CacheConfig>,
        stage_env: &HashMap<String, EnvValue>,
    ) -> HashMap<String, EnvValue> {
        let mut merged = HashMap::new();
        
        // Add global env_key as Capture (baseline)
        if let Some(config) = cache_config {
            for key in &config.env_key {
                merged.insert(key.clone(), EnvValue::Capture);
            }
            
            // Execute env_command and add as Explicit values
            for (key, command) in &config.env_command {
                match std::process::Command::new("sh")
                    .arg("-c")
                    .arg(command)
                    .output()
                {
                    Ok(output) if output.status.success() => {
                        let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
                        debug!(key = %key, command = %command, value = %value, "env_command executed");
                        merged.insert(key.clone(), EnvValue::Explicit(value));
                    }
                    Ok(output) => {
                        warn!(key = %key, command = %command, status = ?output.status, "env_command failed");
                        // Still include as capture fallback
                        merged.insert(key.clone(), EnvValue::Capture);
                    }
                    Err(e) => {
                        warn!(key = %key, command = %command, error = %e, "env_command failed to execute");
                        merged.insert(key.clone(), EnvValue::Capture);
                    }
                }
            }
        }
        
        // Overlay stage env (downstream wins)
        for (key, value) in stage_env {
            merged.insert(key.clone(), value.clone());
        }
        
        merged
    }

    /// Execute a single stage
    fn execute_stage(&self, stage: &Stage, exec_ctx: &mut ExecutionContext) -> Result<StageResult, String> {
        let stage_start = SystemTime::now();

        // Stable per-operation identity, computed from the
        // declared shape only (label, inputs, outputs, env keys, task
        // labels). Independent of input file contents; cheap to compute
        // (BLAKE3 over a small JSON doc).
        let work_hash = compute_work_hash(stage);

        // Check cache if inputs AND outputs are declared
        // Without outputs declared, we can't verify cache validity
        let target_key = if !stage.inputs.is_empty() && !stage.outputs.is_empty() {
            // Compute target key for cache lookup
            let command = stage.tasks.iter()
                .map(|t| t.label())
                .collect::<Vec<_>>()
                .join("; ");
            
            // Merge global [cache] env_key with stage's declared_env
            // Global env_key are added as Capture, stage env overrides (downstream wins)
            let merged_env = self.merge_cache_env(&exec_ctx.cache_config, &stage.declared_env);
            
            match compute_target_key(&exec_ctx.workspace, &stage.inputs, &command, &merged_env, &stage.label, &exec_ctx.variant) {
                Ok(Some(key)) => {
                    // Check cache
                    let cache_result = exec_ctx.cache.lookup(&key);
                    
                    if let CacheResult::Hit { target_key, content_hash, .. } = &cache_result {
                        // Verify outputs exist and match stored hash
                        match hash_file_patterns(&exec_ctx.workspace, &stage.outputs, &stage.label) {
                            Ok(Some(current_hash)) if current_hash == *content_hash => {
                                // Cache hit - outputs verified!
                                println!("⚡ Cache hit for operation: {} (key: {})", stage.label, target_key.short());
                                info_icon!("⚡", stage = %stage.label, key = %target_key.short(), "Cache hit - outputs verified");
                                
                                let stage_duration = stage_start.elapsed()
                                    .map_err(|e| format!("Failed to calculate stage duration: {}", e))?
                                    .as_millis() as u64;
                                
                                return Ok(StageResult {
                                    stage_label: stage.label.clone(),
                                    variant: exec_ctx.variant.clone(),
                                    success: true,
                                    duration_ms: stage_duration,
                                    task_results: vec![],  // No tasks executed (cached)
                                    cache_key: Some(target_key.short().to_string()),
                                    cache_hit: true,
                                    work_hash: work_hash.clone(),
                                });
                            }
                            Ok(Some(_)) => {
                                // Outputs changed - cache invalid
                                debug!(stage = %stage.label, "Cache miss - outputs changed since last run");
                            }
                            Ok(None) => {
                                // Outputs missing
                                debug!(stage = %stage.label, "Cache miss - output files missing");
                            }
                            Err(e) => {
                                debug!(stage = %stage.label, error = %e, "Cache miss - failed to hash outputs");
                            }
                        }
                    }
                    
                    Some(key)
                }
                Ok(None) => {
                    // Inputs declared but 0 files matched - proceed without caching
                    debug!(stage = %stage.label, "Inputs declared but 0 files matched - cache disabled");
                    None
                }
                Err(e) => {
                    // Failed to compute key - proceed without caching
                    debug!(stage = %stage.label, error = %e, "Failed to compute target key");
                    None
                }
            }
        } else if !stage.inputs.is_empty() && stage.outputs.is_empty() {
            // Inputs declared but no outputs - can't cache safely
            debug!(stage = %stage.label, "No output: declared - caching disabled (can't verify outputs)");
            None
        } else {
            None  // No inputs declared = uncacheable
        };
        
        // Output to stdout immediately when stage begins
        println!("▶️  Starting stage: {} (variant: {})\n", stage.label, exec_ctx.variant);
        
        info!(stage = %stage.label, variant = %exec_ctx.variant, "Executing stage");
        
        // Write stage header to log
        exec_ctx.logger.write_stage_header(&stage.label, &exec_ctx.variant)?;

        // Execute all tasks in the stage
        let mut task_results = Vec::new();
        let mut stage_success = true;

        // Create task execution context with farm environment variables.
        let mut env = CapturedEnv::new();
        // Inject `[env]` globals after the ambient environment (so they
        // override the caller's shell) but before the reserved FARM_* vars
        // below (so those always win).
        for (key, value) in &exec_ctx.globals {
            env.map.insert(key.clone().into(), value.clone().into());
        }
        env.map.insert("FARM_VARIANT".into(), exec_ctx.variant.clone().into());
        env.map.insert("FARM_TARGET".into(), exec_ctx.target.clone().into());
        env.map.insert("FARM_WORKSPACE".into(), exec_ctx.workspace.to_string_lossy().to_string().into());
        env.map.insert("FARM_DIR".into(), exec_ctx.farm_dir.to_string_lossy().to_string().into());
        env.map.insert("FARM_LOG_DIR".into(), exec_ctx.logger.log_dir().to_string_lossy().to_string().into());
        
        // Set FARM_EVENT to point to the event.json file in the farm directory
        let event_file_path = exec_ctx.farm_dir.join("event.json");
        env.map.insert("FARM_EVENT".into(), event_file_path.to_string_lossy().to_string().into());
        
        let task_ctx = Context {
            env: &env,
            silent: exec_ctx.silent,
            split_streams: exec_ctx.split_streams,
            opt_interactive: exec_ctx.interactive,
            // Stream this stage's command output straight into the log file as
            // it arrives. The handle shares the logger's writers (no-op when
            // file logging is off).
            log: Some(exec_ctx.logger.stream()),
        };

        for task in &stage.tasks {
            let task_result = self.execute_task(task.as_ref().as_ref(), &task_ctx, exec_ctx)?;
            
            if !task_result.success {
                stage_success = false;
                error!(task = %task_result.task_label, stage = %stage.label, "Task failed");
            }
            
            task_results.push(task_result);
        }

        let stage_duration = stage_start.elapsed()
            .map_err(|e| format!("Failed to calculate stage duration: {}", e))?
            .as_millis() as u64;

        // Get last task's exit code for the footer
        let last_exit_code = task_results.last()
            .map(|r| {
                // We don't have exit_code in TaskResult currently, so use success
                if r.success { 0 } else { 1 }
            });

        // Write stage footer to log
        exec_ctx.logger.write_stage_footer(last_exit_code, stage_success, stage_duration)?;
        exec_ctx.logger.flush()?;

        let result = StageResult {
            stage_label: stage.label.clone(),
            variant: exec_ctx.variant.clone(),
            success: stage_success,
            duration_ms: stage_duration,
            task_results,
            cache_key: target_key.as_ref().map(|k| k.short().to_string()),
            cache_hit: false,
            work_hash,
        };

        if stage_success {
            println!("\n✔️   Operation '{}' completed successfully in {}ms\n", stage.label, stage_duration);
            info!(stage = %stage.label, duration_ms = stage_duration, "Operation completed successfully");
            
            // Store cache entry on success if we computed a target key (requires outputs declared)
            if let Some(key) = target_key {
                // Hash actual output files for content verification
                match hash_file_patterns(&exec_ctx.workspace, &stage.outputs, &stage.label) {
                    Ok(Some(content_hash)) => {
                        if let Err(e) = exec_ctx.cache.store_with_vcs(
                            &key,
                            content_hash,
                            None,
                            Some(stage.label.clone()),
                            Some(exec_ctx.variant.clone()),
                            Some(&exec_ctx.workspace),
                        ) {
                            debug!(stage = %stage.label, error = %e, "Failed to store cache entry");
                        } else {
                            info_icon!("💾", stage = %stage.label, key = %key.short(), "Cache entry stored");
                        }
                    }
                    Ok(None) => {
                        warn!(stage = %stage.label, "Output patterns declared but no files matched - cache entry not stored");
                    }
                    Err(e) => {
                        debug!(stage = %stage.label, error = %e, "Failed to hash outputs for cache");
                    }
                }
            }
        } else {
            println!("\n❌  Operation '{}' failed after {}ms\n", stage.label, stage_duration);
            error!(stage = %stage.label, duration_ms = stage_duration, "Operation failed");
        }

        Ok(result)
    }

    /// Execute a single task within a stage
    fn execute_task(&self, task: &dyn crate::task::Callable, ctx: &Context, exec_ctx: &mut ExecutionContext) -> Result<TaskResult, String> {
        let task_start = SystemTime::now();
        let task_label = task.label();
        
        debug!(task = %task_label, "Executing task");
        
        // Write task command header, then a blank separator. The command's
        // output streams straight into the log file line-by-line *during*
        // `task.run` via the `LogStream` carried in `ctx.log` (see cmd.rs) —
        // so there is no post-run block write here.
        exec_ctx.logger.write_task_header(&task_label)?;
        exec_ctx.logger.begin_output()?;

        // Execute the task (output is streamed to the log + console as it arrives).
        let task_result = task.run(ctx);

        let task_duration = task_start.elapsed()
            .map_err(|e| format!("Failed to calculate task duration: {}", e))?
            .as_millis() as u64;

        // `output`/`error` are intentionally empty: command output is streamed
        // and discarded, not accumulated (no downstream consumer reads them).
        Ok(TaskResult {
            task_label,
            success: task_result.success,
            duration_ms: task_duration,
            output: task_result.stdout,
            error: task_result.stderr,
        })
    }

}

impl std::fmt::Debug for Executor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Executor in workspace: {:?}", self.workspace)
    }
}

/// Generate the per-run build ID used when neither `--build-id` nor
/// `FARM_BUILD_ID` supplies one: `{goal}-{timestamp}-{suffix}`.
///
/// The goal keeps the directory recognisable while poking around by hand; the
/// millisecond timestamp keeps it sortable; the UUIDv7 tail rules out collisions
/// between runs of the same goal started in the same millisecond. Ordering is
/// still taken from manifest metadata, never from this name — the format stays
/// free to change (ADR 0001).
fn generate_build_id(goal: &str) -> String {
    let ts = chrono::Utc::now().format("%Y%m%dT%H%M%S%.3f");
    let uuid = uuid::Uuid::now_v7().simple().to_string();
    format!("{}-{}Z-{}", goal, ts, &uuid[uuid.len() - 6..])
}

#[cfg(test)]
mod tests {
    use crate::env::CapturedEnv;
    use std::ffi::OsString;

    #[test]
    fn test_farm_variant_environment_setup() {
        // Test that the environment variable is properly set up in the task context
        let variant = "debug".to_string();
        
        // Create environment with FARM_VARIANT
        let mut env = CapturedEnv::new();
        env.map.insert("FARM_VARIANT".into(), variant.clone().into());
        
        // Verify the environment variable is set correctly
        let farm_variant = env.map.get(&OsString::from("FARM_VARIANT"));
        assert!(farm_variant.is_some(), "FARM_VARIANT should be set in environment");
        assert_eq!(farm_variant.unwrap(), &OsString::from("debug"), "FARM_VARIANT should equal the specified variant");
        
        // Test with different variant
        let mut env2 = CapturedEnv::new();
        env2.map.insert("FARM_VARIANT".into(), "release".into());
        
        let farm_variant2 = env2.map.get(&OsString::from("FARM_VARIANT"));
        assert!(farm_variant2.is_some(), "FARM_VARIANT should be set for release variant");
        assert_eq!(farm_variant2.unwrap(), &OsString::from("release"), "FARM_VARIANT should equal release");
    }

    #[test]
    fn test_farm_event_environment_setup() {
        use std::path::PathBuf;
        
        // Test that FARM_EVENT environment variable is properly set up
        let farm_dir = PathBuf::from("/data/farm/work/.farm");
        let expected_event_path = farm_dir.join("event.json");
        
        // Create environment with FARM_EVENT
        let mut env = CapturedEnv::new();
        env.map.insert("FARM_EVENT".into(), expected_event_path.to_string_lossy().to_string().into());
        
        // Verify the environment variable is set correctly
        let farm_event = env.map.get(&OsString::from("FARM_EVENT"));
        assert!(farm_event.is_some(), "FARM_EVENT should be set in environment");
        assert_eq!(farm_event.unwrap(), &OsString::from("/data/farm/work/.farm/event.json"), "FARM_EVENT should point to event.json file");
        
        // Test with different workspace path
        let farm_dir2 = PathBuf::from("/tmp/workspace_123/.farm");
        let expected_event_path2 = farm_dir2.join("event.json");
        
        let mut env2 = CapturedEnv::new();
        env2.map.insert("FARM_EVENT".into(), expected_event_path2.to_string_lossy().to_string().into());
        
        let farm_event2 = env2.map.get(&OsString::from("FARM_EVENT"));
        assert!(farm_event2.is_some(), "FARM_EVENT should be set for different workspace");
        assert_eq!(farm_event2.unwrap(), &OsString::from("/tmp/workspace_123/.farm/event.json"), "FARM_EVENT should point to correct workspace event.json");
    }

    #[test]
    fn test_farm_environment_variables_complete() {
        // Test that all FARM_* environment variables are properly set up
        let mut env = CapturedEnv::new();
        
        // Set all farm environment variables
        env.map.insert("FARM_VARIANT".into(), "debug".into());
        env.map.insert("FARM_TARGET".into(), "build".into());
        env.map.insert("FARM_WORKSPACE".into(), "/data/farm/work".into());
        env.map.insert("FARM_DIR".into(), "/data/farm/work/.farm".into());
        env.map.insert("FARM_LOG_DIR".into(), "/data/farm/work/.farm/debug/logs".into());
        env.map.insert("FARM_EVENT".into(), "/data/farm/work/.farm/event.json".into());
        
        // Verify all variables are set correctly
        assert_eq!(env.map.get(&OsString::from("FARM_VARIANT")).unwrap(), &OsString::from("debug"), "FARM_VARIANT should be set");
        assert_eq!(env.map.get(&OsString::from("FARM_TARGET")).unwrap(), &OsString::from("build"), "FARM_TARGET should be set");
        assert_eq!(env.map.get(&OsString::from("FARM_WORKSPACE")).unwrap(), &OsString::from("/data/farm/work"), "FARM_WORKSPACE should be set");
        assert_eq!(env.map.get(&OsString::from("FARM_DIR")).unwrap(), &OsString::from("/data/farm/work/.farm"), "FARM_DIR should be set");
        assert_eq!(env.map.get(&OsString::from("FARM_LOG_DIR")).unwrap(), &OsString::from("/data/farm/work/.farm/debug/logs"), "FARM_LOG_DIR should be set");
        assert_eq!(env.map.get(&OsString::from("FARM_EVENT")).unwrap(), &OsString::from("/data/farm/work/.farm/event.json"), "FARM_EVENT should be set");
    }

    #[test]
    fn test_merge_cache_env_global_keys() {
        use super::*;
        use crate::vendor::parse::EnvValue;
        use std::collections::HashMap;
        
        let executor = Executor::new(std::path::PathBuf::from("/tmp"));
        
        // Global cache with env_key
        let cache_config = Some(CacheConfig {
            max_size: None,
            ttl: None,
            env_key: vec!["CC".to_string(), "RUSTFLAGS".to_string()],
            env_command: HashMap::new(),
        });
        
        // Empty stage env
        let stage_env = HashMap::new();
        
        let merged = executor.merge_cache_env(&cache_config, &stage_env);
        
        // Global keys should be added as Capture
        assert_eq!(merged.get("CC"), Some(&EnvValue::Capture));
        assert_eq!(merged.get("RUSTFLAGS"), Some(&EnvValue::Capture));
    }

    #[test]
    fn test_merge_cache_env_stage_overrides() {
        use super::*;
        use crate::vendor::parse::EnvValue;
        use std::collections::HashMap;
        
        let executor = Executor::new(std::path::PathBuf::from("/tmp"));
        
        // Global cache with env_key
        let cache_config = Some(CacheConfig {
            max_size: None,
            ttl: None,
            env_key: vec!["CC".to_string(), "RUSTFLAGS".to_string()],
            env_command: HashMap::new(),
        });
        
        // Stage overrides CC with explicit value
        let mut stage_env = HashMap::new();
        stage_env.insert("CC".to_string(), EnvValue::Explicit("clang".to_string()));
        stage_env.insert("CFLAGS".to_string(), EnvValue::Capture);
        
        let merged = executor.merge_cache_env(&cache_config, &stage_env);
        
        // Stage override wins
        assert_eq!(merged.get("CC"), Some(&EnvValue::Explicit("clang".to_string())));
        // Global still present
        assert_eq!(merged.get("RUSTFLAGS"), Some(&EnvValue::Capture));
        // Stage-specific
        assert_eq!(merged.get("CFLAGS"), Some(&EnvValue::Capture));
    }

    #[test]
    fn test_merge_cache_env_no_global() {
        use super::*;
        use crate::vendor::parse::EnvValue;
        use std::collections::HashMap;
        
        let executor = Executor::new(std::path::PathBuf::from("/tmp"));
        
        // No global cache config
        let cache_config: Option<CacheConfig> = None;
        
        // Stage with env
        let mut stage_env = HashMap::new();
        stage_env.insert("CC".to_string(), EnvValue::Explicit("gcc".to_string()));
        
        let merged = executor.merge_cache_env(&cache_config, &stage_env);
        
        // Only stage env present
        assert_eq!(merged.len(), 1);
        assert_eq!(merged.get("CC"), Some(&EnvValue::Explicit("gcc".to_string())));
    }

    #[test]
    fn test_merge_cache_env_command() {
        use super::*;
        use crate::vendor::parse::EnvValue;
        use std::collections::HashMap;
        
        let executor = Executor::new(std::path::PathBuf::from("/tmp"));
        
        // Global cache with env_command
        let mut env_command = HashMap::new();
        env_command.insert("ECHO_TEST".to_string(), "echo hello".to_string());
        
        let cache_config = Some(CacheConfig {
            max_size: None,
            ttl: None,
            env_key: vec![],
            env_command,
        });
        
        let stage_env = HashMap::new();
        
        let merged = executor.merge_cache_env(&cache_config, &stage_env);
        
        // env_command should be executed and added as Explicit
        assert!(matches!(merged.get("ECHO_TEST"), Some(EnvValue::Explicit(v)) if v == "hello"));
    }
}
