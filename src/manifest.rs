// SPDX-License-Identifier: AGPL-3.0-only

//! Replay Manifest System
//!
//! Generates manifests capturing build execution state for reproducible replay.
//! The manifest includes git info, environment, target results, and timing data.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::engine::{ExecutionResult, StageResult, TaskResult};

/// VCS reference info at build time.
/// 
/// Abstracted to support different VCS types (git, mercurial, etc.)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum VcsRef {
    /// Git repository
    Git {
        /// Commit hash
        commit: String,
        /// Branch or tag name (if available, not detached HEAD)
        #[serde(skip_serializing_if = "Option::is_none")]
        ref_name: Option<String>,
    },
    // Future: Mercurial, SVN, etc.
}

/// Lifecycle state of a run.
///
/// The manifest is written twice: a stub at run start carrying `Running`, and
/// the full manifest at completion carrying `Ok` or `Fail`. Without the stub a
/// crashed run and a currently-executing run are indistinguishable — both are
/// just a run directory with no manifest — which retention and `farm log` both
/// need to tell apart. See ADR 0001.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunStatus {
    /// The run is executing (or died before it could finish).
    Running,
    /// The run completed successfully.
    Ok,
    /// The run completed with a failure.
    Fail,
}

/// Replay manifest capturing complete build execution state.
///
/// Used to reproduce builds with identical environment and validate
/// replay compatibility.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayManifest {
    /// Unique build identifier
    pub build_id: String,
    
    /// VCS reference at build time (if in a VCS-managed workspace)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vcs_ref: Option<VcsRef>,
    
    /// Goal that was executed
    pub goal: String,
    
    /// Variant used for execution
    pub variant: String,
    
    /// Results for each executed operation
    pub operations: Vec<OperationManifest>,
    
    /// Captured environment variables relevant to the build (sorted)
    pub environment: BTreeMap<String, String>,
    
    /// Whether the overall build succeeded
    pub success: bool,
    
    /// Total execution duration in milliseconds
    pub duration_ms: u64,
    
    /// ISO 8601 timestamp when the build was created
    pub created_at: String,

    /// Lifecycle state. `None` in schema v2 manifests, which predate the field
    /// and were only ever written after completion — resolve those through
    /// [`ReplayManifest::run_status`], which falls back to `success`.
    #[serde(default)]
    pub status: Option<RunStatus>,

    /// True when the run executed through a PTY. Recorded because a PTY exposes
    /// a single stream, so stdout and stderr are merged and the resulting log is
    /// not structurally comparable to a non-interactive run of the same goal.
    #[serde(default)]
    pub interactive: bool,

    /// Manifest schema version for forward compatibility
    pub schema_version: u32,
}

/// Manifest entry for a single executed operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationManifest {
    /// Operation label/name
    pub label: String,

    /// Variant used for this operation
    pub variant: String,

    /// Whether the operation succeeded
    pub success: bool,

    /// Operation execution duration in milliseconds.
    ///
    /// For cache hits this is just the lookup+verify time, not the
    /// runtime that *would* have been spent; use `cache_hit` to
    /// distinguish the two cases.
    pub duration_ms: u64,

    /// Number of tasks executed
    pub task_count: usize,

    /// Task execution results (summarized)
    pub tasks: Vec<TaskManifest>,

    /// True if the operation was satisfied by the cache (no tasks ran).
    #[serde(default)]
    pub cache_hit: bool,

    /// Short cache key (12 hex chars) when caching was active. `None`
    /// if the operation was uncacheable (no inputs/outputs declared).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_key: Option<String>,

    /// 6-hex-char operation identity hash. Stable across runs of the
    /// same Farmfile-declared work.
    #[serde(default)]
    pub work_hash: String,
}

/// Manifest entry for a single executed task
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskManifest {
    /// Task label
    pub label: String,
    
    /// Whether the task succeeded
    pub success: bool,
    
    /// Task execution duration in milliseconds
    pub duration_ms: u64,
}

impl ReplayManifest {
    /// Current schema version.
    ///
    /// v3 added `status` and `interactive`. Both carry `#[serde(default)]`, so
    /// v2 manifests still deserialize.
    pub const SCHEMA_VERSION: u32 = 3;

    /// Create the run-start stub, recording only what is known before any work
    /// happens. Rewritten in full by [`ReplayManifest::from_execution`] when the
    /// run completes; a stub left behind with `Running` marks a crashed run.
    ///
    /// VCS detection is deliberately skipped here — it spawns a `git` process,
    /// and paying that at run start to fill a field the final write overwrites
    /// anyway is not worth it.
    pub fn running(
        build_id: &str,
        goal: &str,
        variant: &str,
        interactive: bool,
        started_at: std::time::SystemTime,
    ) -> Self {
        Self {
            build_id: build_id.to_string(),
            vcs_ref: None,
            goal: goal.to_string(),
            variant: variant.to_string(),
            operations: Vec::new(),
            environment: BTreeMap::new(),
            success: false,
            duration_ms: 0,
            created_at: chrono::DateTime::<chrono::Utc>::from(started_at).to_rfc3339(),
            status: Some(RunStatus::Running),
            interactive,
            schema_version: Self::SCHEMA_VERSION,
        }
    }

    /// Lifecycle state, resolving v2 manifests that predate the `status` field.
    /// Those were only ever written after completion, so `success` carries the
    /// true outcome.
    pub fn run_status(&self) -> RunStatus {
        self.status.unwrap_or(if self.success {
            RunStatus::Ok
        } else {
            RunStatus::Fail
        })
    }

    /// Create a new manifest from execution result
    #[allow(clippy::too_many_arguments)]
    pub fn from_execution(
        build_id: &str,
        goal: &str,
        variant: &str,
        result: &ExecutionResult,
        environment: std::collections::HashMap<String, String>,
        workspace: &Path,
        interactive: bool,
        started_at: std::time::SystemTime,
    ) -> Self {
        let vcs_ref = detect_vcs_ref(workspace);
        // Run *start*, not completion: the stub written at run start carries the
        // same instant, so `created_at` orders runs consistently whether they are
        // still running or already finished.
        let created_at = chrono::DateTime::<chrono::Utc>::from(started_at).to_rfc3339();
        
        let operations = result.executed_stages.iter()
            .map(OperationManifest::from_result)
            .collect();
        
        // Convert to BTreeMap for sorted output
        let environment: BTreeMap<String, String> = environment.into_iter().collect();
        
        Self {
            build_id: build_id.to_string(),
            vcs_ref,
            goal: goal.to_string(),
            variant: variant.to_string(),
            operations,
            environment,
            success: result.success,
            duration_ms: result.total_duration_ms,
            created_at,
            status: Some(if result.success {
                RunStatus::Ok
            } else {
                RunStatus::Fail
            }),
            interactive,
            schema_version: Self::SCHEMA_VERSION,
        }
    }
    
    /// Write manifest to the build directory
    pub fn write_to(&self, build_dir: &Path) -> Result<(), String> {
        let manifest_path = build_dir.join("manifest.json");
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize manifest: {}", e))?;
        
        std::fs::write(&manifest_path, json)
            .map_err(|e| format!("Failed to write manifest to {}: {}", manifest_path.display(), e))?;
        
        Ok(())
    }
    
    /// Read manifest from a build directory
    pub fn read_from(build_dir: &Path) -> Result<Self, String> {
        let manifest_path = build_dir.join("manifest.json");
        
        if !manifest_path.exists() {
            return Err(format!("Manifest not found at {}", manifest_path.display()));
        }
        
        let json = std::fs::read_to_string(&manifest_path)
            .map_err(|e| format!("Failed to read manifest: {}", e))?;
        
        serde_json::from_str(&json)
            .map_err(|e| format!("Failed to parse manifest: {}", e))
    }
    
    /// Check if current environment is compatible with manifest
    pub fn validate_environment(&self, workspace: &Path) -> Vec<ManifestWarning> {
        let mut warnings = Vec::new();
        
        // Check VCS commit if recorded
        if let Some(ref manifest_vcs) = self.vcs_ref {
            if let Some(current_vcs) = detect_vcs_ref(workspace) {
                match (manifest_vcs, &current_vcs) {
                    (VcsRef::Git { commit: expected, .. }, VcsRef::Git { commit: actual, .. }) => {
                        if expected != actual {
                            warnings.push(ManifestWarning::VcsCommitMismatch {
                                expected: expected.clone(),
                                actual: actual.clone(),
                            });
                        }
                    }
                }
            }
        }
        
        // Check critical environment variables
        for (key, expected_value) in &self.environment {
            // Skip non-critical variables
            if key.starts_with("_") || key == "PWD" || key == "OLDPWD" {
                continue;
            }
            
            if let Ok(actual_value) = std::env::var(key) {
                if &actual_value != expected_value {
                    warnings.push(ManifestWarning::EnvVarMismatch {
                        key: key.clone(),
                        expected: expected_value.clone(),
                        actual: actual_value,
                    });
                }
            } else {
                warnings.push(ManifestWarning::EnvVarMissing {
                    key: key.clone(),
                    expected: expected_value.clone(),
                });
            }
        }
        
        warnings
    }
    
    /// Get a summary of the execution for display
    pub fn summary(&self) -> String {
        let operation_status: Vec<_> = self.operations.iter()
            .map(|s| format!("{}: {}", s.label, if s.success { "✓" } else { "✗" }))
            .collect();
        
        format!(
            "Build {} ({}) - {} in {}ms\nOperations: {}",
            self.build_id,
            self.variant,
            if self.success { "SUCCESS" } else { "FAILED" },
            self.duration_ms,
            operation_status.join(", ")
        )
    }
}

impl OperationManifest {
    /// Create from stage result
    pub fn from_result(result: &StageResult) -> Self {
        let tasks = result.task_results.iter()
            .map(TaskManifest::from_result)
            .collect();

        Self {
            label: result.stage_label.clone(),
            variant: result.variant.clone(),
            success: result.success,
            duration_ms: result.duration_ms,
            task_count: result.task_results.len(),
            tasks,
            cache_hit: result.cache_hit,
            cache_key: result.cache_key.clone(),
            work_hash: result.work_hash.clone(),
        }
    }
}

impl TaskManifest {
    /// Create from task result
    pub fn from_result(result: &TaskResult) -> Self {
        Self {
            label: result.task_label.clone(),
            success: result.success,
            duration_ms: result.duration_ms,
        }
    }
}

/// Warnings generated during manifest validation
#[derive(Debug, Clone)]
pub enum ManifestWarning {
    /// VCS commit differs from manifest
    VcsCommitMismatch {
        expected: String,
        actual: String,
    },
    /// Environment variable differs from manifest
    EnvVarMismatch {
        key: String,
        expected: String,
        actual: String,
    },
    /// Environment variable missing that was present at build time
    EnvVarMissing {
        key: String,
        expected: String,
    },
    /// Schema version is newer than supported
    SchemaVersionNewer {
        manifest_version: u32,
        supported_version: u32,
    },
}

impl std::fmt::Display for ManifestWarning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ManifestWarning::VcsCommitMismatch { expected, actual } => {
                write!(f, "VCS commit mismatch: expected {}, found {}", 
                    &expected[..7.min(expected.len())], 
                    &actual[..7.min(actual.len())])
            }
            ManifestWarning::EnvVarMismatch { key, expected, actual } => {
                write!(f, "Environment variable {} differs: expected '{}', found '{}'", 
                    key, expected, actual)
            }
            ManifestWarning::EnvVarMissing { key, expected } => {
                write!(f, "Environment variable {} missing (was '{}')", key, expected)
            }
            ManifestWarning::SchemaVersionNewer { manifest_version, supported_version } => {
                write!(f, "Manifest schema v{} is newer than supported v{}", 
                    manifest_version, supported_version)
            }
        }
    }
}

/// Detect VCS reference from workspace
fn detect_vcs_ref(workspace: &Path) -> Option<VcsRef> {
    // Try git first
    if let Some(vcs_ref) = detect_git_ref(workspace) {
        return Some(vcs_ref);
    }
    
    // Future: try mercurial, svn, etc.
    None
}

/// Get git commit and ref from workspace
fn detect_git_ref(workspace: &Path) -> Option<VcsRef> {
    let commit = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(workspace)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())?;
    
    let ref_name = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(workspace)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|r| r != "HEAD"); // Detached HEAD returns "HEAD"
    
    Some(VcsRef::Git { commit, ref_name })
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_manifest_serialization() {
        let manifest = ReplayManifest {
            build_id: "test-build-123".to_string(),
            vcs_ref: Some(VcsRef::Git {
                commit: "abc123def".to_string(),
                ref_name: Some("main".to_string()),
            }),
            goal: "build".to_string(),
            variant: "debug".to_string(),
            operations: vec![
                OperationManifest {
                    label: "compile".to_string(),
                    variant: "debug".to_string(),
                    success: true,
                    duration_ms: 1500,
                    task_count: 2,
                    tasks: vec![
                        TaskManifest {
                            label: "cargo build".to_string(),
                            success: true,
                            duration_ms: 1200,
                        },
                        TaskManifest {
                            label: "copy assets".to_string(),
                            success: true,
                            duration_ms: 300,
                        },
                    ],
                    cache_hit: false,
                    cache_key: None,
                    work_hash: String::new(),
                },
            ],
            environment: BTreeMap::from([
                ("RUST_BACKTRACE".to_string(), "1".to_string()),
            ]),
            success: true,
            duration_ms: 1500,
            created_at: "2025-01-15T10:30:00Z".to_string(),
            status: Some(RunStatus::Ok),
            interactive: false,
            schema_version: ReplayManifest::SCHEMA_VERSION,
        };

        let json = serde_json::to_string_pretty(&manifest).unwrap();
        let parsed: ReplayManifest = serde_json::from_str(&json).unwrap();
        
        assert_eq!(parsed.build_id, "test-build-123");
        assert_eq!(parsed.operations.len(), 1);
        assert_eq!(parsed.operations[0].tasks.len(), 2);
        
        // Verify VCS ref structure
        match &parsed.vcs_ref {
            Some(VcsRef::Git { commit, ref_name }) => {
                assert_eq!(commit, "abc123def");
                assert_eq!(ref_name.as_deref(), Some("main"));
            }
            _ => panic!("Expected Git VCS ref"),
        }
    }
    
    #[test]
    fn test_manifest_summary() {
        let manifest = ReplayManifest {
            build_id: "build-456".to_string(),
            vcs_ref: None,
            goal: "test".to_string(),
            variant: "default".to_string(),
            operations: vec![
                OperationManifest {
                    label: "unit-tests".to_string(),
                    variant: "default".to_string(),
                    success: true,
                    duration_ms: 500,
                    task_count: 1,
                    tasks: vec![],
                    cache_hit: false,
                    cache_key: None,
                    work_hash: String::new(),
                },
                OperationManifest {
                    label: "integration".to_string(),
                    variant: "default".to_string(),
                    success: false,
                    duration_ms: 200,
                    task_count: 1,
                    tasks: vec![],
                    cache_hit: false,
                    cache_key: None,
                    work_hash: String::new(),
                },
            ],
            environment: BTreeMap::new(),
            success: false,
            duration_ms: 700,
            created_at: "2025-01-15T10:30:00Z".to_string(),
            status: Some(RunStatus::Fail),
            interactive: false,
            schema_version: ReplayManifest::SCHEMA_VERSION,
        };

        let summary = manifest.summary();
        assert!(summary.contains("FAILED"));
        assert!(summary.contains("unit-tests: ✓"));
        assert!(summary.contains("integration: ✗"));
    }

    /// v2 manifests have no `status` field. They were only ever written after a
    /// run finished, so `run_status` must resolve them through `success` rather
    /// than reporting them as still running.
    #[test]
    fn legacy_v2_manifest_resolves_status_from_success() {
        let v2 = r#"{
            "build_id": "old-build",
            "goal": "test",
            "variant": "default",
            "operations": [],
            "environment": {},
            "success": false,
            "duration_ms": 42,
            "created_at": "2025-01-15T10:30:00Z",
            "schema_version": 2
        }"#;

        let parsed: ReplayManifest = serde_json::from_str(v2).unwrap();
        assert_eq!(parsed.status, None);
        assert_eq!(parsed.run_status(), RunStatus::Fail);
        assert!(!parsed.interactive);
    }

    #[test]
    fn running_stub_reports_running() {
        let stub = ReplayManifest::running(
            "b-1",
            "test",
            "default",
            true,
            std::time::SystemTime::UNIX_EPOCH,
        );

        assert_eq!(stub.run_status(), RunStatus::Running);
        assert!(stub.interactive);
        assert!(!stub.success);
        assert_eq!(stub.created_at, "1970-01-01T00:00:00+00:00");
    }
}
