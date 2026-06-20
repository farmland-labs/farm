//! SPDX-License-Identifier: MIT OR Apache-2.0

//! Cache analysis tools for validating input/output declarations
//!
//! Provides two types of analysis:
//! 1. Static check: Compare declared inputs/outputs between dependent stages
//! 2. Runtime analysis: Track actual file changes during stage execution

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter};
use std::path::{Path, PathBuf};
use sha2::{Sha256, Digest};
use glob::glob;
use crate::vendor::log::{warn, debug};
use crate::vendor::parse::Plan;

/// Result of static dependency check
#[derive(Debug)]
pub struct DependencyCheckResult {
    /// Warnings about potential issues
    pub warnings: Vec<DependencyWarning>,
    /// Info messages
    pub info: Vec<String>,
}

#[derive(Debug)]
pub enum DependencyWarning {
    /// Upstream output not declared in downstream input
    MissingUpstreamOutput {
        upstream_stage: String,
        downstream_stage: String,
        missing_outputs: Vec<String>,
    },
    /// Upstream has no output declared, can't verify
    NoOutputDeclared {
        upstream_stage: String,
        downstream_stage: String,
    },
    /// Downstream has no input declared (always runs, uncacheable)
    NoInputDeclared {
        stage: String,
    },
}

impl DependencyWarning {
    pub fn format(&self) -> String {
        match self {
            DependencyWarning::MissingUpstreamOutput { upstream_stage, downstream_stage, missing_outputs } => {
                let outputs = missing_outputs.join(", ");
                format!(
                    "Stage '{}' declares output: {}\n    \
                     Stage '{}' (depends on '{}') does NOT include these in its input.\n    \
                     If '{}' reads these files, add them to its input declaration.",
                    upstream_stage, outputs,
                    downstream_stage, upstream_stage,
                    downstream_stage
                )
            }
            DependencyWarning::NoOutputDeclared { upstream_stage, downstream_stage } => {
                format!(
                    "Stage '{}' has no output declared.\n    \
                     Cannot verify if '{}' correctly declares its dependencies on '{}'.\n    \
                     Consider adding 'output:' to '{}' for better cache validation.",
                    upstream_stage,
                    downstream_stage, upstream_stage,
                    upstream_stage
                )
            }
            DependencyWarning::NoInputDeclared { stage } => {
                format!(
                    "Stage '{}' has no input declared.\n    \
                     This stage is uncacheable and will always execute.",
                    stage
                )
            }
        }
    }
}

/// Perform static check of input/output declarations between stages
pub fn check_dependency_declarations(plan: &Plan, stages: &[String]) -> DependencyCheckResult {
    let mut warnings = Vec::new();
    let mut info = Vec::new();
    
    // Build a map of stage labels to their operations
    let stage_map: HashMap<&str, &crate::vendor::parse::Operation> = plan.operations
        .iter()
        .map(|op| (op.label.as_str(), op))
        .collect();
    
    // Create a synthetic "root" operation only if not explicitly defined in Farmfile
    // Users can define root explicitly with `[operation]` (no label = root)
    let synthetic_root_op = crate::vendor::parse::Operation {
        label: "root".to_string(),
        tasks: vec![],
        depends: vec![],
        inputs: vec![],
        outputs: vec![],
        declared_env: std::collections::HashMap::new(),
        plugin_metadata: std::collections::HashMap::new(),
    };
    
    // For each consecutive pair of stages in the check list
    for window in stages.windows(2) {
        let upstream_label = &window[0];
        let downstream_label = &window[1];
        
        // Look up upstream - use synthetic root only if not in plan
        let upstream: &crate::vendor::parse::Operation = match stage_map.get(upstream_label.as_str()) {
            Some(op) => op,
            None if upstream_label == "root" => &synthetic_root_op,
            None => {
                info.push(format!("Stage '{}' not found in plan", upstream_label));
                continue;
            }
        };
        
        let downstream = match stage_map.get(downstream_label.as_str()) {
            Some(op) => op,
            None => {
                info.push(format!("Stage '{}' not found in plan", downstream_label));
                continue;
            }
        };
        
        // Check if downstream actually depends on upstream (skip for root - everything implicitly depends on root)
        if upstream_label != "root" && !downstream.depends.contains(upstream_label) {
            info.push(format!(
                "Note: '{}' does not have '{}' in its 'after:' declaration. \
                 They may run in parallel or in any order.",
                downstream_label, upstream_label
            ));
        }
        
        // Check for missing input declaration
        if downstream.inputs.is_empty() {
            warnings.push(DependencyWarning::NoInputDeclared {
                stage: downstream_label.clone(),
            });
        }
        
        // Check if upstream has outputs declared
        // Skip NoOutputDeclared warning for root (it typically doesn't have outputs)
        if upstream.outputs.is_empty() {
            if upstream_label != "root" {
                warnings.push(DependencyWarning::NoOutputDeclared {
                    upstream_stage: upstream_label.clone(),
                    downstream_stage: downstream_label.clone(),
                });
            }
            // No outputs to check coverage for
            continue;
        }
        
        // Check if downstream inputs include upstream outputs
        let downstream_inputs: HashSet<&str> = downstream.inputs.iter().map(|s| s.as_str()).collect();
        let missing: Vec<String> = upstream.outputs
            .iter()
            .filter(|output| !downstream_inputs.contains(output.as_str()))
            .cloned()
            .collect();
        
        if !missing.is_empty() {
            warnings.push(DependencyWarning::MissingUpstreamOutput {
                upstream_stage: upstream_label.clone(),
                downstream_stage: downstream_label.clone(),
                missing_outputs: missing,
            });
        }
    }
    
    DependencyCheckResult { warnings, info }
}

/// A snapshot of file hashes, stored on disk for memory efficiency
pub struct WorkspaceSnapshot {
    /// Path to the snapshot file
    snapshot_path: PathBuf,
    /// Number of files in snapshot
    file_count: usize,
}

/// File hash entry (path and hash)
#[derive(Debug, Clone)]
pub struct FileHashEntry {
    pub path: PathBuf,
    pub hash: String,
    pub size: u64,
}

/// Difference between two snapshots
#[derive(Debug, Default)]
pub struct SnapshotDiff {
    pub created: Vec<PathBuf>,
    pub modified: Vec<PathBuf>,
    pub deleted: Vec<PathBuf>,
}

impl SnapshotDiff {
    pub fn is_empty(&self) -> bool {
        self.created.is_empty() && self.modified.is_empty() && self.deleted.is_empty()
    }
    
    pub fn total_changes(&self) -> usize {
        self.created.len() + self.modified.len() + self.deleted.len()
    }
}

impl WorkspaceSnapshot {
    /// Capture files matching patterns and write hashes to a file
    pub fn capture(
        workspace: &Path,
        patterns: &[String],
        output_path: &Path,
    ) -> Result<Self, String> {
        Self::capture_with_progress(workspace, patterns, output_path, false)
    }
    
    /// Capture files matching patterns with optional progress output
    pub fn capture_with_progress(
        workspace: &Path,
        patterns: &[String],
        output_path: &Path,
        show_progress: bool,
    ) -> Result<Self, String> {
        use std::io::Write;
        
        let mut file_count = 0;
        let file = File::create(output_path)
            .map_err(|e| format!("Failed to create snapshot file: {}", e))?;
        let mut writer = BufWriter::new(file);
        
        // Collect and sort files for determinism
        let mut all_files: Vec<PathBuf> = Vec::new();
        
        if show_progress {
            print!("   Collecting files ");
            std::io::stdout().flush().ok();
        }
        
        let mut glob_count = 0usize;
        for pattern in patterns {
            let full_pattern = if pattern.starts_with('/') {
                pattern.to_string()
            } else {
                workspace.join(pattern).to_string_lossy().to_string()
            };
            
            match glob(&full_pattern) {
                Ok(paths) => {
                    for entry in paths.flatten() {
                        glob_count += 1;
                        if show_progress && glob_count.is_multiple_of(1000) {
                            if glob_count.is_multiple_of(10000) {
                                print!("{}k", glob_count / 1000);
                            } else {
                                print!(".");
                            }
                            std::io::stdout().flush().ok();
                        }
                        if entry.is_file() {
                            all_files.push(entry);
                        }
                    }
                }
                Err(e) => {
                    warn!("Invalid glob pattern '{}': {}", pattern, e);
                }
            }
        }
        
        if show_progress {
            println!(" ({} entries)", glob_count);
            print!("   Sorting...");
            std::io::stdout().flush().ok();
        }
        
        all_files.sort();
        all_files.dedup();
        
        if show_progress {
            println!(" {} unique files", all_files.len());
            print!("   Hashing ");
            std::io::stdout().flush().ok();
        }
        
        // Hash each file and write to snapshot file
        for (i, path) in all_files.iter().enumerate() {
            if show_progress && (i + 1) % 1000 == 0 {
                if (i + 1) % 10000 == 0 {
                    print!("{}k", (i + 1) / 1000);
                } else {
                    print!(".");
                }
                std::io::stdout().flush().ok();
            }
            
            match hash_file(path) {
                Ok((hash, size)) => {
                    let rel_path = path.strip_prefix(workspace).unwrap_or(path);
                    writeln!(writer, "{}\t{}\t{}", hash, size, rel_path.display())
                        .map_err(|e| format!("Failed to write snapshot: {}", e))?;
                    file_count += 1;
                }
                Err(e) => {
                    debug!("Failed to hash file {}: {}", path.display(), e);
                }
            }
        }
        
        if show_progress {
            println!(" done");
        }
        
        writer.flush().map_err(|e| format!("Failed to flush snapshot: {}", e))?;
        
        Ok(Self {
            snapshot_path: output_path.to_path_buf(),
            file_count,
        })
    }
    
    /// Capture pre-enumerated files (skip glob, just hash)
    pub fn capture_files(
        workspace: &Path,
        files: &[PathBuf],
        output_path: &Path,
        show_progress: bool,
    ) -> Result<Self, String> {
        use std::io::Write;
        
        let mut file_count = 0;
        let file = File::create(output_path)
            .map_err(|e| format!("Failed to create snapshot file: {}", e))?;
        let mut writer = BufWriter::new(file);
        
        if show_progress {
            print!("   Hashing {} files ", files.len());
            std::io::stdout().flush().ok();
        }
        
        // Hash each file and write to snapshot file
        for (i, path) in files.iter().enumerate() {
            if show_progress && (i + 1) % 1000 == 0 {
                if (i + 1) % 10000 == 0 {
                    print!("{}k", (i + 1) / 1000);
                } else {
                    print!(".");
                }
                std::io::stdout().flush().ok();
            }
            
            // Convert relative path to absolute for hashing
            let abs_path = if path.is_absolute() {
                path.clone()
            } else {
                workspace.join(path)
            };
            
            match hash_file(&abs_path) {
                Ok((hash, size)) => {
                    let rel_path = abs_path.strip_prefix(workspace).unwrap_or(&abs_path);
                    writeln!(writer, "{}\t{}\t{}", hash, size, rel_path.display())
                        .map_err(|e| format!("Failed to write snapshot: {}", e))?;
                    file_count += 1;
                }
                Err(e) => {
                    debug!("Failed to hash file {}: {}", abs_path.display(), e);
                }
            }
        }
        
        if show_progress {
            println!(" done");
        }
        
        writer.flush().map_err(|e| format!("Failed to flush snapshot: {}", e))?;
        
        Ok(Self {
            snapshot_path: output_path.to_path_buf(),
            file_count,
        })
    }
    
    /// Capture entire workspace (all files, respecting .gitignore if possible)
    pub fn capture_workspace(workspace: &Path, output_path: &Path) -> Result<Self, String> {
        // Use **/* to match all files recursively
        Self::capture(workspace, &["**/*".to_string()], output_path)
    }
    
    /// Load snapshot from file into a HashMap for comparison
    fn load_as_map(&self) -> Result<HashMap<PathBuf, (String, u64)>, String> {
        let file = File::open(&self.snapshot_path)
            .map_err(|e| format!("Failed to open snapshot: {}", e))?;
        let reader = BufReader::new(file);
        let mut map = HashMap::new();
        
        for line in reader.lines() {
            let line = line.map_err(|e| format!("Failed to read snapshot line: {}", e))?;
            let parts: Vec<&str> = line.splitn(3, '\t').collect();
            if parts.len() == 3 {
                let hash = parts[0].to_string();
                let size: u64 = parts[1].parse().unwrap_or(0);
                let path = PathBuf::from(parts[2]);
                map.insert(path, (hash, size));
            }
        }
        
        Ok(map)
    }
    
    /// Compare two snapshots and return the diff
    pub fn diff(&self, newer: &WorkspaceSnapshot) -> Result<SnapshotDiff, String> {
        let old_map = self.load_as_map()?;
        let new_map = newer.load_as_map()?;
        
        let mut diff = SnapshotDiff::default();
        
        // Find created and modified files
        for (path, (new_hash, _)) in &new_map {
            match old_map.get(path) {
                None => diff.created.push(path.clone()),
                Some((old_hash, _)) if old_hash != new_hash => diff.modified.push(path.clone()),
                _ => {}
            }
        }
        
        // Find deleted files
        for path in old_map.keys() {
            if !new_map.contains_key(path) {
                diff.deleted.push(path.clone());
            }
        }
        
        // Sort for consistent output
        diff.created.sort();
        diff.modified.sort();
        diff.deleted.sort();
        
        Ok(diff)
    }
    
    pub fn file_count(&self) -> usize {
        self.file_count
    }
    
    pub fn path(&self) -> &Path {
        &self.snapshot_path
    }
}

/// Hash a single file
fn hash_file(path: &Path) -> Result<(String, u64), String> {
    let content = fs::read(path)
        .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
    let size = content.len() as u64;
    
    let mut hasher = Sha256::new();
    hasher.update(&content);
    let hash = hex::encode(hasher.finalize());
    
    Ok((hash, size))
}

/// Check if files from a diff are declared in a stage's inputs
pub fn check_diff_coverage(
    diff: &SnapshotDiff,
    stage_inputs: &[String],
    workspace: &Path,
) -> Vec<PathBuf> {
    // Expand input patterns to get actual files
    let declared_files: HashSet<PathBuf> = stage_inputs
        .iter()
        .flat_map(|pattern| {
            let full_pattern = if pattern.starts_with('/') {
                pattern.to_string()
            } else {
                workspace.join(pattern).to_string_lossy().to_string()
            };
            
            glob(&full_pattern)
                .ok()
                .into_iter()
                .flatten()
                .filter_map(|e| e.ok())
                .filter(|p| p.is_file())
                .map(|p| p.strip_prefix(workspace).unwrap_or(&p).to_path_buf())
                .collect::<Vec<_>>()
        })
        .collect();
    
    // Find files that changed but aren't declared
    let mut uncovered = Vec::new();
    
    for path in diff.created.iter().chain(diff.modified.iter()) {
        let rel_path = path.strip_prefix(workspace).unwrap_or(path);
        if !declared_files.contains(rel_path) {
            uncovered.push(rel_path.to_path_buf());
        }
    }
    
    uncovered.sort();
    uncovered
}

/// Interactive prompt to continue
pub fn prompt_continue(message: &str) -> Result<bool, String> {
    use std::io::{stdin, stdout, Write};
    
    print!("{} [y/N]: ", message);
    stdout().flush().map_err(|e| e.to_string())?;
    
    let mut input = String::new();
    stdin().read_line(&mut input).map_err(|e| e.to_string())?;
    
    Ok(input.trim().eq_ignore_ascii_case("y") || input.trim().eq_ignore_ascii_case("yes"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use std::fs;
    
    #[test]
    fn test_snapshot_capture_and_diff() {
        let temp = TempDir::new().unwrap();
        let workspace = temp.path();
        
        // Create initial files
        fs::write(workspace.join("file1.txt"), "content1").unwrap();
        fs::write(workspace.join("file2.txt"), "content2").unwrap();
        
        let snap1_path = workspace.join(".snap1");
        let snap1 = WorkspaceSnapshot::capture(
            workspace,
            &["*.txt".to_string()],
            &snap1_path,
        ).unwrap();
        
        assert_eq!(snap1.file_count(), 2);
        
        // Modify file1, create file3
        fs::write(workspace.join("file1.txt"), "modified").unwrap();
        fs::write(workspace.join("file3.txt"), "new").unwrap();
        
        let snap2_path = workspace.join(".snap2");
        let snap2 = WorkspaceSnapshot::capture(
            workspace,
            &["*.txt".to_string()],
            &snap2_path,
        ).unwrap();
        
        let diff = snap1.diff(&snap2).unwrap();
        
        assert_eq!(diff.created.len(), 1);
        assert_eq!(diff.modified.len(), 1);
        assert_eq!(diff.deleted.len(), 0);
    }
    
    #[test]
    fn test_snapshot_deleted_files() {
        let temp = TempDir::new().unwrap();
        let workspace = temp.path();
        
        fs::write(workspace.join("file1.txt"), "content1").unwrap();
        fs::write(workspace.join("file2.txt"), "content2").unwrap();
        
        let snap1_path = workspace.join(".snap1");
        let snap1 = WorkspaceSnapshot::capture(
            workspace,
            &["*.txt".to_string()],
            &snap1_path,
        ).unwrap();
        
        // Delete file2
        fs::remove_file(workspace.join("file2.txt")).unwrap();
        
        let snap2_path = workspace.join(".snap2");
        let snap2 = WorkspaceSnapshot::capture(
            workspace,
            &["*.txt".to_string()],
            &snap2_path,
        ).unwrap();
        
        let diff = snap1.diff(&snap2).unwrap();
        
        assert_eq!(diff.deleted.len(), 1);
        assert!(diff.deleted[0].to_string_lossy().contains("file2.txt"));
    }
}
