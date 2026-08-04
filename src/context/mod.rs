//! SPDX-License-Identifier: MIT OR Apache-2.0

//! Pipeline build context management.
//!
//! Provides named contexts for managing build state independently of VCS:
//! - `farm ctx create/switch/delete/list` — Context management
//! - `farm ctx env` — Export environment variables
//! - `farm ctx sync vcs` — Update VCS hint
//! - `farm ops init/show/upstream` — Build operations

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use chrono::{DateTime, Utc};
use crate::vendor::log::info;
use crate::vendor::protocol::ContextManifest;
use serde::{Deserialize, Serialize};

/// Error type for context operations.
#[derive(Debug, thiserror::Error)]
pub enum ContextError {
    #[error("Build context not found. Initialize with: farm ops init <stage>")]
    NoBuildDir,
    
    #[error("Context not found: {0}")]
    ContextNotFound(String),
    
    #[error("Context file not found: {0}")]
    ContextFileNotFound(PathBuf),
    
    #[error("Failed to read: {0}")]
    ReadError(#[from] std::io::Error),
    
    #[error("Failed to parse: {0}")]
    ParseError(#[from] serde_json::Error),

    /// `context.json` failed to load via the protocol crate's
    /// validation (schema_version policy, size cap, or
    /// `deny_unknown_fields`). The inner string is the protocol's
    /// own error message; the path is the file we tried to read.
    #[error("context.json invalid at {0}: {1}")]
    ManifestInvalid(PathBuf, String),
    
    #[error("Stage not found in context: {0}")]
    StageNotFound(String),
    
    #[error("Token not found: {0}:{1}")]
    TokenNotFound(String, String),
    
    #[error("Not in a pipeline build context")]
    NotPipelineBuild,
    
    #[error("Context already exists: {0}")]
    ContextExists(String),
    
    #[error("Cannot delete current context: {0}")]
    CannotDeleteCurrent(String),
    
    #[error("No workspace found")]
    NoWorkspace,
}

/// Result type for context operations.
pub type Result<T> = std::result::Result<T, ContextError>;

// ============================================================================
// VCS Detection
// ============================================================================

/// VCS hint captured at goal invocation
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VcsHint {
    #[serde(rename = "type")]
    pub vcs_type: String,
    pub branch: Option<String>,
    pub commit: Option<String>,
}

/// Goal specification stored in goal.json
/// Immutable after invocation - used for CI replay
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Goal {
    /// Target operation to achieve
    pub target: String,
    /// Optional variant
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
    /// Farmfile used
    pub farmfile: String,
    /// When the goal was invoked
    pub invoked_at: DateTime<Utc>,
    /// VCS state at invocation (for reproducibility)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vcs_hint: Option<VcsHint>,
}

impl Goal {
    pub fn new(target: &str, variant: Option<&str>, farmfile: &str, vcs_hint: Option<VcsHint>) -> Self {
        Self {
            target: target.to_string(),
            variant: variant.map(|s| s.to_string()),
            farmfile: farmfile.to_string(),
            invoked_at: Utc::now(),
            vcs_hint,
        }
    }
}

/// Current execution state stored in state.json
/// Tracks progress toward goal
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct State {
    /// Current operation being executed (moves toward goal)
    pub operation: String,
    /// Execution status
    pub status: ExecutionStatus,
    /// Last updated timestamp
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ExecutionStatus {
    Running,
    Completed,
    Failed,
}

impl State {
    pub fn new(operation: &str, status: ExecutionStatus) -> Self {
        Self {
            operation: operation.to_string(),
            status,
            updated_at: Utc::now(),
        }
    }
    
    pub fn running(operation: &str) -> Self {
        Self::new(operation, ExecutionStatus::Running)
    }
    
    pub fn completed(operation: &str) -> Self {
        Self::new(operation, ExecutionStatus::Completed)
    }
    
    pub fn failed(operation: &str) -> Self {
        Self::new(operation, ExecutionStatus::Failed)
    }
}

/// Detect current VCS state (best effort)
pub fn detect_vcs(workspace: &Path) -> Option<VcsHint> {
    // Try git first
    if let Some(hint) = detect_git(workspace) {
        return Some(hint);
    }
    // Could add svn, hg detection here
    None
}

fn detect_git(workspace: &Path) -> Option<VcsHint> {
    // Check if .git exists
    if !workspace.join(".git").exists() {
        return None;
    }
    
    // Get current branch
    let branch = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(workspace)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    
    // Get current commit
    let commit = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(workspace)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    
    Some(VcsHint {
        vcs_type: "git".to_string(),
        branch,
        commit,
    })
}

/// Check if VCS state has changed since goal was set
pub fn check_vcs_mismatch(workspace: &Path, goal: &Goal) -> Option<(VcsHint, VcsHint)> {
    let current = detect_vcs(workspace)?;
    let stored = goal.vcs_hint.as_ref()?;
    
    if current.branch != stored.branch || current.commit != stored.commit {
        Some((stored.clone(), current))
    } else {
        None
    }
}

/// Get the current git branch name
pub fn current_git_branch(workspace: &Path) -> Option<String> {
    detect_vcs(workspace).and_then(|h| h.branch)
}

/// List local git branch names. Empty when the workspace is not a git
/// repo or `git` isn't on PATH. Used by `farm ctx branch clean` to
/// distinguish orphan contexts from contexts that still have a backing
/// branch.
pub fn list_local_git_branches(workspace: &Path) -> Vec<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(workspace)
        .args(["for-each-ref", "--format=%(refname:short)", "refs/heads/"])
        .output();
    match output {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

/// Resolve context name, treating "." as the current git branch.
/// Returns None if "." is used but no git branch is detected.
pub fn resolve_context_name(workspace: &Path, name: &str) -> Option<String> {
    if name == "." {
        current_git_branch(workspace)
    } else {
        Some(name.to_string())
    }
}

/// Default branch / context name when no `--name` is given AND we can't
/// detect a git branch. Deliberately *not* `main` so it can never collide
/// with a real git branch — encountering `_main` always means
/// "no git context detected", never "you are on the main branch".
pub const FARM_DEFAULT_CTX_NAME: &str = "_main";

/// Sanitize a raw branch name (or any user-supplied context name) into a
/// filesystem-safe form. Rules:
/// - allowed character set is `[a-zA-Z0-9_-]+`
/// - any `/` is rewritten to `_` (so `feature/auth-redesign` becomes
///   `feature_auth-redesign`)
/// - any other character outside the allowed set is rejected
/// - leading `-` is rejected (would be parsed as a CLI flag)
/// - `..` is rejected (path-traversal guard)
/// - empty string or > 64 chars is rejected
///
/// Returns `Ok(sanitized)` or `Err(reason)`.
pub fn sanitize_branch_name(raw: &str) -> std::result::Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("branch name is empty".to_string());
    }
    if trimmed.len() > 64 {
        return Err(format!("branch name is {} chars, max 64", trimmed.len()));
    }
    if trimmed.contains("..") {
        return Err("branch name contains `..`".to_string());
    }
    if trimmed.starts_with('-') {
        return Err("branch name cannot start with `-`".to_string());
    }
    let normalized: String = trimmed.chars().map(|c| if c == '/' { '_' } else { c }).collect();
    for c in normalized.chars() {
        if !(c.is_ascii_alphanumeric() || c == '_' || c == '-') {
            return Err(format!(
                "branch name contains invalid character {:?}; allowed: [a-zA-Z0-9_-]",
                c
            ));
        }
    }
    Ok(normalized)
}

/// Auto-resolve the branch / context name when the user did not pass one.
/// Order: current git branch (sanitized) → `_main` fallback.
pub fn auto_resolve_branch_name(workspace: &Path) -> String {
    if let Some(branch) = current_git_branch(workspace) {
        if let Ok(sanitized) = sanitize_branch_name(&branch) {
            return sanitized;
        }
    }
    FARM_DEFAULT_CTX_NAME.to_string()
}

// ============================================================================
// Context Paths
// ============================================================================

/// Paths for a named context (branch-level namespace).
///
/// Carries only what's actually ctx-shared across builds: the
/// invocation snapshot (`goal.json`), execution state
/// (`state.json`), and the parcel mount cache (`mnt/`). Token
/// files and per-build state live under `run/{build_id}/` (see
/// [`RunPaths`]).
#[derive(Debug, Clone)]
pub struct ContextPaths {
    /// Context name
    pub name: String,
    /// Workspace root
    pub workspace: PathBuf,
    /// Context root (.farm/ctx/{name}/)
    pub ctx_dir: PathBuf,
    /// Mount directory (.farm/ctx/{name}/mnt/) — shared parcel cache
    pub mnt_dir: PathBuf,
}

impl ContextPaths {
    /// Create ContextPaths for a named context.
    pub fn new(workspace: &Path, name: &str) -> Self {
        let ctx_dir = workspace.join(".farm").join("ctx").join(name);
        Self {
            name: name.to_string(),
            workspace: workspace.to_path_buf(),
            ctx_dir: ctx_dir.clone(),
            mnt_dir: ctx_dir.join("mnt"),
        }
    }

    /// Path to goal.json (immutable after invocation).
    pub fn goal_json(&self) -> PathBuf {
        self.ctx_dir.join("goal.json")
    }

    /// Path to state.json (execution progress).
    pub fn state_json(&self) -> PathBuf {
        self.ctx_dir.join("state.json")
    }

    /// Check if context exists.
    pub fn exists(&self) -> bool {
        self.ctx_dir.exists()
    }

    /// Mount path for an upstream parcel (shared cache).
    pub fn mount_path(&self, operation: &str, name: &str, variant: Option<&str>) -> PathBuf {
        let dirname = match variant {
            Some(v) if !v.is_empty() => format!("{}_{}_{}", operation, name, v),
            _ => format!("{}_{}", operation, name),
        };
        self.mnt_dir.join(dirname)
    }
}

// ============================================================================
// Run Paths (per-build_id, isolated)
// ============================================================================

/// Paths for a pipeline run (per-build_id, isolated).
///
/// This structure contains paths for a single run, where input and output
/// tokens are isolated per-run to prevent accumulation across runs.
/// The mount directory is shared across runs (cached parcels).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunPaths {
    /// Workspace root
    pub workspace: PathBuf,
    /// Build ID
    pub build_id: String,
    /// Context name
    pub ctx_name: String,
    /// Run directory (.farm/run/{build_id}/)
    pub run_dir: PathBuf,
    /// Input tokens directory (.farm/run/{build_id}/in/)
    pub in_dir: PathBuf,
    /// Output tokens directory (.farm/run/{build_id}/out/)
    pub out_dir: PathBuf,
    /// Log directory (.farm/run/{build_id}/log/)
    pub log_dir: PathBuf,
    /// Mount directory (.farm/ctx/{ctx}/mnt/) - shared for caching
    pub mnt_dir: PathBuf,
    /// True when `init_run` had to create the branch context on the fly
    /// (the `.farm/ctx/{ctx}/` directory didn't exist). Surfaced to the
    /// caller so the human-readable output can show "auto-initialised".
    #[serde(default)]
    pub auto_initialized: bool,
}

impl RunPaths {
    /// Create RunPaths for a build_id within a context.
    pub fn new(workspace: &Path, build_id: &str, ctx_name: &str) -> Self {
        let run_dir = workspace.join(".farm").join("run").join(build_id);
        let mnt_dir = workspace.join(".farm").join("ctx").join(ctx_name).join("mnt");
        Self {
            workspace: workspace.to_path_buf(),
            build_id: build_id.to_string(),
            ctx_name: ctx_name.to_string(),
            run_dir: run_dir.clone(),
            in_dir: run_dir.join("in"),
            out_dir: run_dir.join("out"),
            log_dir: run_dir.join("log"),
            mnt_dir,
            auto_initialized: false,
        }
    }

    /// Check if run directory exists.
    pub fn exists(&self) -> bool {
        self.run_dir.exists()
    }

    /// Mount path for a parcel (in the shared ctx-level mnt/).
    pub fn mount_path(
        &self,
        op: &str,
        name: &str,
        variant: Option<&str>,
    ) -> PathBuf {
        let dirname = match variant {
            Some(v) if !v.is_empty() => format!("{}_{}_{}", op, name, v),
            _ => format!("{}_{}", op, name),
        };
        self.mnt_dir.join(dirname)
    }

    /// Path to `context.json` — the typed `ContextManifest`
    /// (identity-only; untrusted/external data lives in
    /// `external/<source>.json`).
    pub fn context_json(&self) -> PathBuf {
        self.run_dir.join("context.json")
    }

    /// Directory holding untrusted external data files
    /// (`external/<source>.json`). See
    /// `vendor::protocol::EXTERNAL_SUBDIR`.
    pub fn external_dir(&self) -> PathBuf {
        self.run_dir.join(crate::vendor::protocol::EXTERNAL_SUBDIR)
    }

    /// Path to a single external source file under `external/`.
    pub fn external_source_path(&self, source: &str) -> PathBuf {
        self.external_dir().join(format!("{}.json", source))
    }

    /// Path to `work.json` (the CI runner writes raw build metadata here
    /// so Farmfile scripts can inspect it).
    pub fn work_json(&self) -> PathBuf {
        self.run_dir.join("work.json")
    }
}

/// Initialize a pipeline run directory.
///
/// Creates:
/// - `.farm/run/{build_id}/`            — run directory
/// - `.farm/run/{build_id}/in/`         — downlink token files (per-source-op subdirs)
/// - `.farm/run/{build_id}/out/`        — uplink token files (per-producing-op subdirs)
/// - `.farm/run/{build_id}/log/`        — runtime logs
/// - `.farm/run/{build_id}/external/`   — untrusted data (one file per source)
/// - `.farm/ctx/{ctx}/mnt/`             — parcel mount cache (shared across builds)
///
/// Returns RunPaths with all the paths needed for the run.
pub fn init_run(workspace: &Path, build_id: &str, ctx_name: &str) -> Result<RunPaths> {
    let mut paths = RunPaths::new(workspace, build_id, ctx_name);

    // Auto-create the branch context if it's not there yet. Equivalent to
    // calling `farm ctx branch init --name {ctx_name}` first; lets the
    // `farm ctx run init` (and therefore `farm ctx init`) one-shot work
    // straight after a `git checkout`.
    let ctx_dir = workspace.join(".farm").join("ctx").join(ctx_name);
    if !ctx_dir.exists() {
        create_context(workspace, ctx_name)?;
        paths.auto_initialized = true;
    }

    // Create run directories
    fs::create_dir_all(&paths.run_dir)?;
    fs::create_dir_all(&paths.in_dir)?;
    fs::create_dir_all(&paths.out_dir)?;
    fs::create_dir_all(&paths.log_dir)?;
    fs::create_dir_all(paths.external_dir())?;
    fs::create_dir_all(&paths.mnt_dir)?;

    // Write notice file explaining the directory is internal
    crate::write_farm_notice(&workspace.join(".farm"));

    // Update .farm/ctx/current to point to this context
    let current_file = workspace.join(".farm").join("ctx").join("current");
    fs::create_dir_all(current_file.parent().unwrap())?;
    fs::write(&current_file, format!("{}\n", ctx_name))?;

    Ok(paths)
}

/// Clean up old run directories, keeping the most recent N runs.
///
/// Ordering comes from each run's manifest (falling back to directory mtime),
/// and the retention safety rules apply: a run that is still executing is never
/// removed, and the newest failure of each goal survives. Since ADR 0001 every
/// invocation has its own run directory, so a plain newest-N-by-mtime sweep
/// could delete the directory of a build that is running right now.
pub fn cleanup_old_runs(workspace: &Path, keep_count: usize) -> Result<usize> {
    let farm_dir = workspace.join(".farm");

    if !farm_dir.join("run").exists() {
        return Ok(0);
    }

    Ok(crate::retention::prune_all(&farm_dir, keep_count).removed.len())
}

// ============================================================================
// Context Resolution
// ============================================================================

/// Find workspace root. Requires .farm directory to exist.
/// Priority: FARM_WORKSPACE env var → current directory
pub fn find_workspace() -> Result<PathBuf> {
    // 1. Check FARM_WORKSPACE environment variable
    if let Ok(workspace_path) = std::env::var("FARM_WORKSPACE") {
        let path = PathBuf::from(&workspace_path);
        if path.join(".farm").exists() {
            info!("Using workspace from FARM_WORKSPACE: {}", path.display());
            return Ok(path);
        }
        // FARM_WORKSPACE is set but .farm doesn't exist there
        return Err(ContextError::NoWorkspace);
    }
    
    // 2. Use current directory (no walk-up to parent directories)
    let cwd = std::env::current_dir().map_err(ContextError::ReadError)?;
    if cwd.join(".farm").exists() {
        return Ok(cwd);
    }
    
    Err(ContextError::NoWorkspace)
}

/// Get workspace directory without requiring .farm to exist.
/// Used for initialization commands that create .farm.
pub fn workspace_dir() -> Result<PathBuf> {
    if let Ok(workspace_path) = std::env::var("FARM_WORKSPACE") {
        info!("Using workspace from FARM_WORKSPACE: {}", workspace_path);
        return Ok(PathBuf::from(workspace_path));
    }
    std::env::current_dir().map_err(ContextError::ReadError)
}

/// Get the current context name.
/// Priority: FARM_CTX env var → .farm/ctx/current file → current git branch
/// → `FARM_DEFAULT_CTX_NAME` ("_main").
pub fn current_context_name(workspace: &Path) -> String {
    // 1. FARM_CTX environment variable
    if let Ok(ctx) = std::env::var("FARM_CTX") {
        if !ctx.is_empty() {
            return ctx;
        }
    }

    // 2. .farm/ctx/current file
    let current_file = workspace.join(".farm").join("ctx").join("current");
    if let Ok(content) = fs::read_to_string(&current_file) {
        let name = content.trim();
        if !name.is_empty() {
            return name.to_string();
        }
    }

    // 3. Current git branch (auto-detect) → _main fallback
    auto_resolve_branch_name(workspace)
}

/// Get paths for the current context
pub fn resolve_context() -> Result<ContextPaths> {
    let workspace = find_workspace()?;
    let name = current_context_name(&workspace);
    Ok(ContextPaths::new(&workspace, &name))
}

/// Default build_id used by direct CLI invocations when no explicit
/// `--build-id` or `FARM_BUILD_ID` env var is set. Each `farm run`
/// overwrites this slot.
pub const DEFAULT_BUILD_ID: &str = "local";

/// Resolve build paths for the current invocation.
///
/// build_id precedence:
/// 1. `FARM_BUILD_ID` env var (orchestrator-set)
/// 2. [`DEFAULT_BUILD_ID`] (= `"local"`) — developer-default slot
///
/// Workspace defaults to `current_dir`; context name is auto-resolved
/// from the current git branch (or `_main`). The returned RunPaths
/// is ready-to-use: directories are created on-demand by `init_run`
/// (idempotent), so subsequent `farm info` / `farm upstream` commands
/// work even before the first `farm run`.
pub fn resolve_build_paths() -> Result<RunPaths> {
    let workspace = std::env::current_dir().map_err(ContextError::ReadError)?;
    let build_id = std::env::var("FARM_BUILD_ID")
        .unwrap_or_else(|_| DEFAULT_BUILD_ID.to_string());
    let ctx_name = current_context_name(&workspace);
    init_run(&workspace, &build_id, &ctx_name)
}

// ============================================================================
// Context Management
// ============================================================================

/// List all contexts in the workspace
/// Returns (name, goal) pairs - goal is None if no goal has been set yet
pub fn list_contexts(workspace: &Path) -> Result<Vec<(String, Option<Goal>)>> {
    let ctx_root = workspace.join(".farm").join("ctx");
    if !ctx_root.exists() {
        return Ok(Vec::new());
    }
    
    let mut contexts = Vec::new();
    for entry in fs::read_dir(&ctx_root)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            let name = entry.file_name().to_string_lossy().to_string();
            let goal_path = entry.path().join("goal.json");
            let goal = if goal_path.exists() {
                let content = fs::read_to_string(&goal_path)?;
                Some(serde_json::from_str(&content)?)
            } else {
                None
            };
            contexts.push((name, goal));
        }
    }
    contexts.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(contexts)
}

/// Create a new context
pub fn create_context(workspace: &Path, name: &str) -> Result<ContextPaths> {
    let ctx = ContextPaths::new(workspace, name);
    if ctx.exists() {
        return Err(ContextError::ContextExists(name.to_string()));
    }
    
    // Create directories only — goal.json is written when farm run
    // is invoked; token + log dirs live under run/{build_id}/.
    fs::create_dir_all(&ctx.ctx_dir)?;
    fs::create_dir_all(&ctx.mnt_dir)?;
    
    Ok(ctx)
}

/// Switch to a context (updates .farm/ctx/current)
pub fn switch_context(workspace: &Path, name: &str) -> Result<ContextPaths> {
    let ctx = ContextPaths::new(workspace, name);
    if !ctx.exists() {
        return Err(ContextError::ContextNotFound(name.to_string()));
    }
    
    // Update .farm/ctx/current (which context is active)
    let current_file = workspace.join(".farm").join("ctx").join("current");
    fs::create_dir_all(current_file.parent().unwrap())?;
    fs::write(&current_file, format!("{}\n", name))?;
    
    Ok(ctx)
}

/// Delete a context
pub fn delete_context(workspace: &Path, name: &str) -> Result<()> {
    // Check not deleting current
    let current = current_context_name(workspace);
    if current == name {
        return Err(ContextError::CannotDeleteCurrent(name.to_string()));
    }
    
    let ctx = ContextPaths::new(workspace, name);
    if !ctx.exists() {
        return Err(ContextError::ContextNotFound(name.to_string()));
    }
    
    fs::remove_dir_all(&ctx.ctx_dir)?;
    Ok(())
}

/// Read goal from context (None if no goal set yet)
pub fn read_goal(ctx: &ContextPaths) -> Result<Option<Goal>> {
    let goal_path = ctx.goal_json();
    if !goal_path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(&goal_path)?;
    Ok(Some(serde_json::from_str(&content)?))
}

/// Write goal to context (immutable after first write during a run)
pub fn write_goal(ctx: &ContextPaths, goal: &Goal) -> Result<()> {
    let goal_path = ctx.goal_json();
    fs::create_dir_all(goal_path.parent().unwrap())?;
    fs::write(&goal_path, serde_json::to_string_pretty(&goal)?)?;
    Ok(())
}

/// Read current execution state (None if not running)
pub fn read_state(ctx: &ContextPaths) -> Result<Option<State>> {
    let current_path = ctx.state_json();
    if !current_path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(&current_path)?;
    Ok(Some(serde_json::from_str(&content)?))
}

/// Write current execution state
pub fn write_state(ctx: &ContextPaths, state: &State) -> Result<()> {
    let current_path = ctx.state_json();
    fs::create_dir_all(current_path.parent().unwrap())?;
    fs::write(&current_path, serde_json::to_string_pretty(&state)?)?;
    Ok(())
}

// ============================================================================
// Operation Management
// ============================================================================

/// Read the typed context manifest from `context.json`.
///
/// Enforces the load-time policy in `ContextManifest::from_json_slice`:
/// byte-size cap, schema_version present-and-supported,
/// `deny_unknown_fields` on the typed shape.
///
/// Upstream tokens are NOT in the manifest — they live as
/// individual `in/{from_op}/{name}[_variant].token` files.
pub fn read_context(paths: &RunPaths) -> Result<ContextManifest> {
    let context_path = paths.context_json();
    if !context_path.exists() {
        return Err(ContextError::ContextFileNotFound(context_path));
    }

    let bytes = fs::read(&context_path)?;
    let manifest = ContextManifest::from_json_slice(&bytes)
        .map_err(|e| ContextError::ManifestInvalid(context_path.clone(), e.to_string()))?;
    Ok(manifest)
}

/// Write the typed context manifest to `context.json`.
pub fn write_context(paths: &RunPaths, manifest: &ContextManifest) -> Result<()> {
    let context_path = paths.context_json();
    if let Some(parent) = context_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let content = serde_json::to_string_pretty(manifest)?;
    fs::write(&context_path, content)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Hermetic test setup. The unit tests in this module operate on
    /// raw `TempDir` paths in-process (no subprocess spawn), so only
    /// `FARM_CTX` matters here — `current_context_name` consults it
    /// before any per-workspace state, and a developer shell that
    /// exports it would shadow the `.farm/ctx/current` file these tests
    /// write.
    ///
    /// (`FARM_WORKSPACE` only affects the spawned `farm` CLI's
    /// workspace discovery, which lives in the integration-test side.)
    ///
    /// Wrapped in a `Once` so concurrent test threads can't race on
    /// the env mutation; safe because no test in this crate ever
    /// *sets* FARM_CTX, so there's no writer to race with.
    fn isolate_env() {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            std::env::remove_var("FARM_CTX");
        });
    }

    #[test]
    fn test_init_run_local() {
        isolate_env();
        let temp = TempDir::new().unwrap();
        let workspace = temp.path();

        let paths = init_run(workspace, DEFAULT_BUILD_ID, FARM_DEFAULT_CTX_NAME).unwrap();

        assert!(paths.run_dir.exists());
        assert!(paths.in_dir.exists());
        assert!(paths.out_dir.exists());
        assert!(paths.log_dir.exists());
        assert!(paths.external_dir().exists());
        assert!(paths.mnt_dir.exists());

        // Default context name when no git branch is detected is
        // FARM_DEFAULT_CTX_NAME ("_main"). state.json is written
        // by the build engine, not by init_run, so we don't assert
        // on its existence here.
        assert_eq!(paths.ctx_name, FARM_DEFAULT_CTX_NAME);
        assert_eq!(paths.build_id, DEFAULT_BUILD_ID);
    }
    
    #[test]
    fn test_mount_path() {
        let ctx = ContextPaths::new(Path::new("/workspace"), "default");

        // Without variant
        assert_eq!(
            ctx.mount_path("build", "output", None),
            PathBuf::from("/workspace/.farm/ctx/default/mnt/build_output")
        );

        // With variant
        assert_eq!(
            ctx.mount_path("build", "output", Some("debug")),
            PathBuf::from("/workspace/.farm/ctx/default/mnt/build_output_debug")
        );
    }

    #[test]
    fn test_context_paths() {
        let ctx = ContextPaths::new(Path::new("/workspace"), "feature-branch");

        assert_eq!(ctx.name, "feature-branch");
        assert_eq!(ctx.ctx_dir, PathBuf::from("/workspace/.farm/ctx/feature-branch"));
        assert_eq!(ctx.mnt_dir, PathBuf::from("/workspace/.farm/ctx/feature-branch/mnt"));
    }
    
    #[test]
    fn test_context_name_resolution() {
        isolate_env();
        let temp = TempDir::new().unwrap();
        let workspace = temp.path();

        // No FARM_CTX, no .farm/ctx/current file, no .git in the
        // tempdir → falls through to FARM_DEFAULT_CTX_NAME
        // ("_main") rather than the literal "main", so an
        // unset context can never be confused with the actual
        // main branch.
        assert_eq!(current_context_name(workspace), FARM_DEFAULT_CTX_NAME);

        // .farm/ctx/current overrides the default.
        fs::create_dir_all(workspace.join(".farm").join("ctx")).unwrap();
        fs::write(workspace.join(".farm").join("ctx").join("current"), "my-context\n").unwrap();
        assert_eq!(current_context_name(workspace), "my-context");
    }
    
    #[test]
    fn test_create_and_switch_context() {
        isolate_env();
        let temp = TempDir::new().unwrap();
        let workspace = temp.path();
        
        // Create a context
        let ctx = create_context(workspace, "feature-x").unwrap();
        assert!(ctx.exists());
        // goal.json is not created until farm run is invoked
        assert!(!ctx.goal_json().exists());
        
        // Switch to it
        switch_context(workspace, "feature-x").unwrap();
        assert_eq!(current_context_name(workspace), "feature-x");
    }
    
    #[test]
    fn test_list_contexts() {
        let temp = TempDir::new().unwrap();
        let workspace = temp.path();
        
        // Initially empty
        let contexts = list_contexts(workspace).unwrap();
        assert!(contexts.is_empty());
        
        // Create some contexts
        create_context(workspace, "ctx-a").unwrap();
        create_context(workspace, "ctx-b").unwrap();
        
        let contexts = list_contexts(workspace).unwrap();
        assert_eq!(contexts.len(), 2);
        assert_eq!(contexts[0].0, "ctx-a");
        assert!(contexts[0].1.is_none()); // No goal set yet
        assert_eq!(contexts[1].0, "ctx-b");
    }
}
