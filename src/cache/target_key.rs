//! SPDX-License-Identifier: MIT OR Apache-2.0

//! Target key computation for cache lookups
//!
//! A target key uniquely identifies a build target's expected output based on:
//! - Input files (content hash of files matching glob patterns)
//! - Command to execute
//! - Declared environment variables
//!
//! File hashing is parallelized using rayon for performance with large file sets.
//! Determinism is guaranteed by sorting files before combining hashes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::io::Read;
use sha2::{Sha256, Digest};
use glob::glob;
use serde::{Deserialize, Serialize};
use crate::vendor::parse::EnvValue;
use crate::vendor::log::{trace, debug, warn};
use rayon::prelude::*;

/// A computed target key that uniquely identifies a build configuration
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct TargetKey {
    /// Hash of all input file contents
    pub inputs_hash: String,
    /// Hash of the command string
    pub command_hash: String,
    /// Hash of the environment variables
    pub env_hash: String,
    /// Combined key (hash of all above)
    pub key: String,
}

impl TargetKey {
    /// Get the short form of the key (first 12 chars)
    pub fn short(&self) -> &str {
        &self.key[..12.min(self.key.len())]
    }
}

impl std::fmt::Display for TargetKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.key)
    }
}

/// Compute a target key from operation inputs
///
/// # Arguments
/// * `workspace` - Base directory for resolving glob patterns
/// * `inputs` - Glob patterns for input files (e.g., "src/**", "Cargo.toml")
/// * `command` - The command to execute
/// * `declared_env` - Environment variables declared in the Farmfile
/// * `stage_name` - Name of the stage (for logging)
///
/// # Returns
/// - `Ok(Some(TargetKey))` - valid inputs, here's the key for cache lookup
/// - `Ok(None)` - inputs declared but 0 files matched (uncacheable)
/// - `Err(...)` - error computing key
pub fn compute_target_key(
    workspace: &Path,
    inputs: &[String],
    command: &str,
    declared_env: &HashMap<String, EnvValue>,
    stage_name: &str,
    variant: &str,
) -> Result<Option<TargetKey>, String> {
    debug!(stage = %stage_name, inputs = inputs.len(), variant = %variant, "Computing target key");
    trace!("  workspace: {}", workspace.display());
    trace!("  inputs: {:?}", inputs);
    trace!("  command: {}", command);
    trace!("  declared_env keys: {:?}", declared_env.keys().collect::<Vec<_>>());
    
    // Hash input files - returns None if patterns declared but 0 files matched
    let inputs_hash = match hash_file_patterns(workspace, inputs, stage_name)? {
        Some(hash) => hash,
        None => return Ok(None),  // Uncacheable - inputs declared but no files matched
    };
    trace!("  inputs_hash: {}", &inputs_hash[..16.min(inputs_hash.len())]);
    
    // Hash command
    let command_hash = sha256_string(command);
    trace!("  command_hash: {}", &command_hash[..16.min(command_hash.len())]);
    
    // Hash environment (sorted for determinism)
    let env_hash = hash_env_map(declared_env);
    trace!("  env_hash: {}", &env_hash[..16.min(env_hash.len())]);
    
    // Hash variant to ensure different variants get different cache keys
    let variant_hash = sha256_string(variant);
    trace!("  variant_hash: {}", &variant_hash[..16.min(variant_hash.len())]);
    
    // Combine all hashes (including variant)
    let combined = format!("{}{}{}{}", inputs_hash, command_hash, env_hash, variant_hash);
    let key = sha256_string(&combined);
    
    debug!(stage = %stage_name, key = %&key[..16.min(key.len())], "Computed target key");
    
    Ok(Some(TargetKey {
        inputs_hash,
        command_hash,
        env_hash,
        key,
    }))
}

/// Hash all files matching the given glob patterns
///
/// Files are hashed in parallel using rayon for performance.
/// Determinism is guaranteed by sorting files before combining hashes.
///
/// # Returns
/// - `Ok(Some(hash))` - files found and hashed
/// - `Ok(None)` - patterns declared but 0 files matched (uncacheable, warning logged)
/// - `Err(...)` - invalid glob pattern or I/O error
///
/// # Performance
/// - Parallel file I/O and hashing via rayon
/// - Threshold: uses parallel processing for 10+ files
pub fn hash_file_patterns(workspace: &Path, patterns: &[String], stage_name: &str) -> Result<Option<String>, String> {
    if patterns.is_empty() {
        trace!("No input patterns - returning empty hash");
        // No inputs declared = hash of empty string (still cacheable)
        return Ok(Some(sha256_string("")));
    }
    
    let mut all_files: Vec<PathBuf> = Vec::new();
    
    for pattern in patterns {
        // Make pattern relative to workspace
        let full_pattern = if pattern.starts_with('/') {
            pattern.to_string()
        } else {
            workspace.join(pattern).to_string_lossy().to_string()
        };
        
        trace!("Expanding glob pattern: {} -> {}", pattern, full_pattern);
        
        match glob(&full_pattern) {
            Ok(paths) => {
                let mut pattern_count = 0;
                for entry in paths {
                    match entry {
                        Ok(path) if path.is_file() => {
                            trace!("  matched file: {}", path.display());
                            all_files.push(path);
                            pattern_count += 1;
                        }
                        Ok(_) => {} // Skip directories
                        Err(e) => {
                            return Err(format!("Glob error for '{}': {}", pattern, e));
                        }
                    }
                }
                if pattern_count == 0 {
                    warn!("Input pattern '{}' matched 0 files", pattern);
                } else {
                    trace!("  pattern '{}' matched {} files", pattern, pattern_count);
                }
            }
            Err(e) => {
                return Err(format!("Invalid glob pattern '{}': {}", pattern, e));
            }
        }
    }
    
    // Sort and deduplicate for deterministic ordering
    // (multiple patterns can match the same file)
    all_files.sort();
    all_files.dedup();
    let file_count = all_files.len();
    
    // If inputs declared but 0 files matched, return None (uncacheable)
    if file_count == 0 {
        warn!("[{}] Input patterns declared but 0 files matched - cache disabled", stage_name);
        return Ok(None);
    }
    
    // Use parallel hashing for large file sets (threshold: 10 files)
    let use_parallel = file_count >= 10;
    
    let start = std::time::Instant::now();
    
    // Hash files (parallel or sequential) -> Vec<(relative_path, hash, bytes)>
    let file_hashes: Result<Vec<(String, String, u64)>, String> = if use_parallel {
        trace!("Using parallel hashing for {} files", file_count);
        all_files
            .par_iter()
            .map(|path| hash_single_file(workspace, path))
            .collect()
    } else {
        trace!("Using sequential hashing for {} files", file_count);
        all_files
            .iter()
            .map(|path| hash_single_file(workspace, path))
            .collect()
    };
    
    let file_hashes = file_hashes?;
    
    // Combine hashes in sorted order (deterministic!)
    // Files are already sorted by path, so file_hashes maintains that order
    let mut final_hasher = Sha256::new();
    let mut total_bytes: u64 = 0;
    
    for (rel_path, hash, bytes) in &file_hashes {
        trace!("  hashed: {} ({} bytes)", rel_path, bytes);
        final_hasher.update(rel_path.as_bytes());
        final_hasher.update(b"\0");
        final_hasher.update(hash.as_bytes());
        final_hasher.update(b"\0");
        total_bytes += bytes;
    }
    
    let elapsed = start.elapsed();
    debug!(
        stage = %stage_name,
        files = file_count,
        bytes = total_bytes,
        elapsed = ?elapsed,
        mode = if use_parallel { "parallel" } else { "sequential" },
        "Hashed files"
    );
    
    Ok(Some(hex::encode(final_hasher.finalize())))
}

/// Hash a single file and return (relative_path, hash, bytes_read)
fn hash_single_file(workspace: &Path, path: &Path) -> Result<(String, String, u64), String> {
    // Get relative path for deterministic key
    let rel_path_str = if let Ok(rel_path) = path.strip_prefix(workspace) {
        rel_path.to_string_lossy().to_string()
    } else {
        path.to_string_lossy().to_string()
    };
    
    // Hash file content
    let mut hasher = Sha256::new();
    let mut total_bytes: u64 = 0;
    
    match std::fs::File::open(path) {
        Ok(mut file) => {
            let mut buffer = [0u8; 8192];
            loop {
                match file.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(n) => {
                        hasher.update(&buffer[..n]);
                        total_bytes += n as u64;
                    }
                    Err(e) => {
                        return Err(format!("Failed to read '{}': {}", path.display(), e));
                    }
                }
            }
        }
        Err(e) => {
            return Err(format!("Failed to open '{}': {}", path.display(), e));
        }
    }
    
    Ok((rel_path_str, hex::encode(hasher.finalize()), total_bytes))
}

/// Hash a string using SHA-256
fn sha256_string(s: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(s.as_bytes());
    hex::encode(hasher.finalize())
}

/// Hash environment variables map
///
/// For EnvValue::Capture, we capture the current environment value.
/// Variables are sorted by key for determinism.
fn hash_env_map(env: &HashMap<String, EnvValue>) -> String {
    if env.is_empty() {
        return sha256_string("");
    }
    
    // Sort keys for determinism
    let mut keys: Vec<_> = env.keys().collect();
    keys.sort();
    
    let mut hasher = Sha256::new();
    
    for key in keys {
        let value = match env.get(key) {
            Some(EnvValue::Explicit(v)) => v.clone(),
            Some(EnvValue::Capture) => {
                // Capture from current environment
                std::env::var(key).unwrap_or_default()
            }
            None => String::new(),
        };
        
        hasher.update(key.as_bytes());
        hasher.update(b"=");
        hasher.update(value.as_bytes());
        hasher.update(b"\0");
    }
    
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;
    
    // ========================================================================
    // Basic Hash Tests
    // ========================================================================
    
    #[test]
    fn test_sha256_string() {
        let hash = sha256_string("hello");
        assert_eq!(hash.len(), 64);
        // SHA256 of "hello" is well-known
        assert_eq!(
            hash,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }
    
    #[test]
    fn test_target_key_short() {
        let key = TargetKey {
            inputs_hash: "abc123".to_string(),
            command_hash: "def456".to_string(),
            env_hash: "ghi789".to_string(),
            key: "0123456789abcdef".to_string(),
        };
        assert_eq!(key.short(), "0123456789ab");
    }
    
    // ========================================================================
    // Environment Hash Tests
    // ========================================================================
    
    #[test]
    fn test_hash_env_map_empty() {
        let env: HashMap<String, EnvValue> = HashMap::new();
        let hash = hash_env_map(&env);
        assert_eq!(hash, sha256_string(""));
    }
    
    #[test]
    fn test_hash_env_map_explicit() {
        let mut env = HashMap::new();
        env.insert("FOO".to_string(), EnvValue::Explicit("bar".to_string()));
        env.insert("BAZ".to_string(), EnvValue::Explicit("qux".to_string()));
        
        let hash1 = hash_env_map(&env);
        let hash2 = hash_env_map(&env);
        
        // Should be deterministic
        assert_eq!(hash1, hash2);
        assert_eq!(hash1.len(), 64);
    }
    
    #[test]
    fn test_hash_env_map_order_independent() {
        // Insert in different order, should get same hash (sorted internally)
        let mut env1 = HashMap::new();
        env1.insert("AAA".to_string(), EnvValue::Explicit("1".to_string()));
        env1.insert("ZZZ".to_string(), EnvValue::Explicit("2".to_string()));
        
        let mut env2 = HashMap::new();
        env2.insert("ZZZ".to_string(), EnvValue::Explicit("2".to_string()));
        env2.insert("AAA".to_string(), EnvValue::Explicit("1".to_string()));
        
        assert_eq!(hash_env_map(&env1), hash_env_map(&env2));
    }
    
    #[test]
    fn test_hash_env_map_value_change_changes_hash() {
        let mut env1 = HashMap::new();
        env1.insert("FOO".to_string(), EnvValue::Explicit("value1".to_string()));
        
        let mut env2 = HashMap::new();
        env2.insert("FOO".to_string(), EnvValue::Explicit("value2".to_string()));
        
        assert_ne!(hash_env_map(&env1), hash_env_map(&env2));
    }
    
    // ========================================================================
    // Empty/No Input Pattern Tests
    // ========================================================================
    
    #[test]
    fn test_hash_file_patterns_empty_patterns_is_cacheable() {
        // Empty patterns = no inputs declared = still cacheable with empty hash
        let workspace = Path::new("/tmp");
        let patterns: Vec<String> = vec![];
        
        let hash = hash_file_patterns(workspace, &patterns, "test").unwrap();
        // Empty patterns returns Some(hash of empty string), not None
        assert_eq!(hash, Some(sha256_string("")));
    }
    
    // ========================================================================
    // Zero Files Matched Tests (Critical for Cache Correctness!)
    // ========================================================================
    
    #[test]
    fn test_pattern_matching_zero_files_returns_none() {
        // Patterns declared but matching 0 files → None (uncacheable)
        let temp = TempDir::new().unwrap();
        // Don't create any files
        
        let patterns = vec!["*.rs".to_string()];
        let result = hash_file_patterns(temp.path(), &patterns, "test").unwrap();
        
        assert!(result.is_none(), "Should return None when patterns match 0 files");
    }
    
    #[test]
    fn test_double_star_alone_matches_directories_not_files() {
        // This is the critical glob gotcha: ** matches directories only
        let temp = TempDir::new().unwrap();
        
        // Create a directory structure with files
        let subdir = temp.path().join("scripts");
        fs::create_dir(&subdir).unwrap();
        fs::write(subdir.join("build.sh"), "#!/bin/bash").unwrap();
        fs::write(subdir.join("test.sh"), "#!/bin/bash").unwrap();
        
        // ** alone should match the 'scripts' directory, not the files inside
        // Since we only hash files and skip directories, this should return None
        let patterns = vec!["scripts/**".to_string()];
        let result = hash_file_patterns(temp.path(), &patterns, "test").unwrap();
        
        // ** matches only directories, not files inside them
        // So with just "scripts/**", we get 0 files → None
        assert!(result.is_none(), 
            "Pattern 'scripts/**' should match 0 files (only directories), returning None");
    }
    
    #[test]
    fn test_double_star_slash_star_matches_files_recursively() {
        // **/* matches all files recursively
        let temp = TempDir::new().unwrap();
        
        let subdir = temp.path().join("scripts");
        fs::create_dir(&subdir).unwrap();
        fs::write(subdir.join("build.sh"), "#!/bin/bash").unwrap();
        fs::write(subdir.join("test.sh"), "#!/bin/bash").unwrap();
        
        // **/* should match the files
        let patterns = vec!["scripts/**/*".to_string()];
        let result = hash_file_patterns(temp.path(), &patterns, "test").unwrap();
        
        assert!(result.is_some(), 
            "Pattern 'scripts/**/*' should match files recursively");
    }
    
    #[test]
    fn test_double_star_with_extension_matches_files() {
        // **/*.ext matches files with specific extension recursively
        let temp = TempDir::new().unwrap();
        
        let subdir = temp.path().join("src");
        fs::create_dir(&subdir).unwrap();
        fs::write(subdir.join("main.rs"), "fn main() {}").unwrap();
        fs::write(subdir.join("lib.rs"), "// lib").unwrap();
        fs::write(subdir.join("readme.txt"), "text file").unwrap(); // Should not match
        
        let patterns = vec!["src/**/*.rs".to_string()];
        let result = hash_file_patterns(temp.path(), &patterns, "test").unwrap();
        
        assert!(result.is_some(), 
            "Pattern 'src/**/*.rs' should match .rs files recursively");
    }
    
    #[test]
    fn test_nonexistent_path_pattern_returns_none() {
        let temp = TempDir::new().unwrap();
        
        // Pattern references path that doesn't exist
        let patterns = vec!["nonexistent_dir/**/*".to_string()];
        let result = hash_file_patterns(temp.path(), &patterns, "test").unwrap();
        
        assert!(result.is_none(), 
            "Pattern for non-existent path should return None");
    }
    
    #[test]
    fn test_empty_directory_returns_none() {
        let temp = TempDir::new().unwrap();
        
        // Create empty subdirectory
        let subdir = temp.path().join("empty");
        fs::create_dir(&subdir).unwrap();
        
        let patterns = vec!["empty/**/*".to_string()];
        let result = hash_file_patterns(temp.path(), &patterns, "test").unwrap();
        
        assert!(result.is_none(), 
            "Empty directory with **/* pattern should return None");
    }
    
    #[test]
    fn test_compute_target_key_returns_none_when_zero_files_match() {
        let temp = TempDir::new().unwrap();
        let env = HashMap::new();
        
        // Pattern that matches nothing
        let result = compute_target_key(
            temp.path(),
            &["*.nonexistent".to_string()],
            "echo hello",
            &env,
            "test-stage",
            "default",
        ).unwrap();
        
        assert!(result.is_none(), 
            "compute_target_key should return None when inputs match 0 files");
    }
    
    // ========================================================================
    // File Content and Path Tests
    // ========================================================================
    
    #[test]
    fn test_hash_file_patterns_single_file() {
        let temp = TempDir::new().unwrap();
        let file_path = temp.path().join("test.txt");
        fs::write(&file_path, "hello world").unwrap();
        
        let patterns = vec!["test.txt".to_string()];
        let hash = hash_file_patterns(temp.path(), &patterns, "test").unwrap().unwrap();
        
        assert_eq!(hash.len(), 64);
    }
    
    #[test]
    fn test_hash_file_patterns_content_change_changes_hash() {
        let temp = TempDir::new().unwrap();
        let file_path = temp.path().join("test.txt");
        
        fs::write(&file_path, "version 1").unwrap();
        let hash1 = hash_file_patterns(temp.path(), &["test.txt".to_string()], "test").unwrap().unwrap();
        
        fs::write(&file_path, "version 2").unwrap();
        let hash2 = hash_file_patterns(temp.path(), &["test.txt".to_string()], "test").unwrap().unwrap();
        
        assert_ne!(hash1, hash2, "Different content should produce different hash");
    }
    
    #[test]
    fn test_file_rename_changes_hash() {
        // File path is part of hash, so renaming changes the hash
        let temp = TempDir::new().unwrap();
        
        let file1 = temp.path().join("old_name.txt");
        fs::write(&file1, "content").unwrap();
        let hash1 = hash_file_patterns(temp.path(), &["*.txt".to_string()], "test").unwrap().unwrap();
        
        // Rename file
        let file2 = temp.path().join("new_name.txt");
        fs::rename(&file1, &file2).unwrap();
        let hash2 = hash_file_patterns(temp.path(), &["*.txt".to_string()], "test").unwrap().unwrap();
        
        assert_ne!(hash1, hash2, "Renaming a file should change the hash");
    }
    
    #[test]
    fn test_file_added_changes_hash() {
        let temp = TempDir::new().unwrap();
        
        fs::write(temp.path().join("file1.txt"), "content1").unwrap();
        let hash1 = hash_file_patterns(temp.path(), &["*.txt".to_string()], "test").unwrap().unwrap();
        
        // Add another file
        fs::write(temp.path().join("file2.txt"), "content2").unwrap();
        let hash2 = hash_file_patterns(temp.path(), &["*.txt".to_string()], "test").unwrap().unwrap();
        
        assert_ne!(hash1, hash2, "Adding a file should change the hash");
    }
    
    #[test]
    fn test_file_removed_changes_hash() {
        let temp = TempDir::new().unwrap();
        
        fs::write(temp.path().join("file1.txt"), "content1").unwrap();
        fs::write(temp.path().join("file2.txt"), "content2").unwrap();
        let hash1 = hash_file_patterns(temp.path(), &["*.txt".to_string()], "test").unwrap().unwrap();
        
        // Remove a file
        fs::remove_file(temp.path().join("file2.txt")).unwrap();
        let hash2 = hash_file_patterns(temp.path(), &["*.txt".to_string()], "test").unwrap().unwrap();
        
        assert_ne!(hash1, hash2, "Removing a file should change the hash");
    }
    
    #[test]
    fn test_empty_file_is_hashable() {
        let temp = TempDir::new().unwrap();
        let file_path = temp.path().join("empty.txt");
        fs::write(&file_path, "").unwrap();
        
        let result = hash_file_patterns(temp.path(), &["empty.txt".to_string()], "test").unwrap();
        assert!(result.is_some(), "Empty file should still produce a valid hash");
    }
    
    #[test]
    fn test_binary_file_is_hashable() {
        let temp = TempDir::new().unwrap();
        let file_path = temp.path().join("binary.bin");
        fs::write(&file_path, [0u8, 1, 2, 3, 255, 254, 253]).unwrap();
        
        let result = hash_file_patterns(temp.path(), &["binary.bin".to_string()], "test").unwrap();
        assert!(result.is_some(), "Binary file should produce a valid hash");
    }
    
    // ========================================================================
    // Multiple Pattern Tests
    // ========================================================================
    
    #[test]
    fn test_multiple_patterns() {
        let temp = TempDir::new().unwrap();
        
        fs::write(temp.path().join("src.rs"), "fn main() {}").unwrap();
        fs::write(temp.path().join("Cargo.toml"), "[package]").unwrap();
        
        let patterns = vec!["*.rs".to_string(), "Cargo.toml".to_string()];
        let result = hash_file_patterns(temp.path(), &patterns, "test").unwrap();
        
        assert!(result.is_some(), "Multiple patterns matching files should work");
    }
    
    #[test]
    fn test_overlapping_patterns_deduplicate() {
        // If two patterns match the same file, it should only be hashed once
        let temp = TempDir::new().unwrap();
        
        fs::write(temp.path().join("main.rs"), "fn main() {}").unwrap();
        
        // Both patterns match main.rs
        let patterns1 = vec!["main.rs".to_string()];
        let patterns2 = vec!["main.rs".to_string(), "*.rs".to_string()];
        
        let hash1 = hash_file_patterns(temp.path(), &patterns1, "test").unwrap().unwrap();
        let hash2 = hash_file_patterns(temp.path(), &patterns2, "test").unwrap().unwrap();
        
        // Hashes should be the same because main.rs is hashed once (deduplicated via sort)
        assert_eq!(hash1, hash2, 
            "Overlapping patterns should deduplicate files");
    }
    
    #[test]
    fn test_some_patterns_match_some_dont() {
        // If at least one pattern matches files, we get a hash
        let temp = TempDir::new().unwrap();
        
        fs::write(temp.path().join("exists.txt"), "content").unwrap();
        
        // First pattern matches, second doesn't
        let patterns = vec!["exists.txt".to_string(), "nonexistent.xyz".to_string()];
        let result = hash_file_patterns(temp.path(), &patterns, "test").unwrap();
        
        assert!(result.is_some(), 
            "Should return Some if at least one pattern matches files");
    }
    
    // ========================================================================
    // Command Hash Tests
    // ========================================================================
    
    #[test]
    fn test_command_change_changes_key() {
        let temp = TempDir::new().unwrap();
        fs::write(temp.path().join("src.rs"), "fn main() {}").unwrap();
        
        let env = HashMap::new();
        let inputs = vec!["src.rs".to_string()];
        
        let key1 = compute_target_key(temp.path(), &inputs, "cargo build", &env, "build", "default").unwrap().unwrap();
        let key2 = compute_target_key(temp.path(), &inputs, "cargo build --release", &env, "build", "default").unwrap().unwrap();
        
        assert_ne!(key1.key, key2.key, "Different command should produce different key");
        assert_ne!(key1.command_hash, key2.command_hash);
        // Inputs hash should be the same
        assert_eq!(key1.inputs_hash, key2.inputs_hash);
    }
    
    #[test]
    fn test_whitespace_in_command_matters() {
        let temp = TempDir::new().unwrap();
        fs::write(temp.path().join("src.rs"), "fn main() {}").unwrap();
        
        let env = HashMap::new();
        let inputs = vec!["src.rs".to_string()];
        
        let key1 = compute_target_key(temp.path(), &inputs, "echo hello", &env, "test", "default").unwrap().unwrap();
        let key2 = compute_target_key(temp.path(), &inputs, "echo  hello", &env, "test", "default").unwrap().unwrap();
        
        assert_ne!(key1.command_hash, key2.command_hash, 
            "Whitespace differences in command should produce different hash");
    }
    
    // ========================================================================
    // Target Key Composition Tests
    // ========================================================================
    
    #[test]
    fn test_compute_target_key() {
        let temp = TempDir::new().unwrap();
        let file_path = temp.path().join("src.rs");
        fs::write(&file_path, "fn main() {}").unwrap();
        
        let mut env = HashMap::new();
        env.insert("RUST_VERSION".to_string(), EnvValue::Explicit("1.75".to_string()));
        
        let key = compute_target_key(
            temp.path(),
            &["src.rs".to_string()],
            "cargo build",
            &env,
            "build",
            "default",
        ).unwrap().unwrap();
        
        assert_eq!(key.key.len(), 64);
        assert_eq!(key.inputs_hash.len(), 64);
        assert_eq!(key.command_hash.len(), 64);
        assert_eq!(key.env_hash.len(), 64);
    }
    
    #[test]
    fn test_compute_target_key_deterministic() {
        let temp = TempDir::new().unwrap();
        let file_path = temp.path().join("main.c");
        fs::write(&file_path, "#include <stdio.h>").unwrap();
        
        let env = HashMap::new();
        
        let key1 = compute_target_key(temp.path(), &["main.c".to_string()], "gcc", &env, "compile", "default").unwrap().unwrap();
        let key2 = compute_target_key(temp.path(), &["main.c".to_string()], "gcc", &env, "compile", "default").unwrap().unwrap();
        
        assert_eq!(key1, key2, "Same inputs should produce identical target keys");
    }
    
    #[test]
    fn test_env_change_changes_key() {
        let temp = TempDir::new().unwrap();
        fs::write(temp.path().join("src.rs"), "fn main() {}").unwrap();
        
        let mut env1 = HashMap::new();
        env1.insert("RUSTFLAGS".to_string(), EnvValue::Explicit("-O2".to_string()));
        
        let mut env2 = HashMap::new();
        env2.insert("RUSTFLAGS".to_string(), EnvValue::Explicit("-O3".to_string()));
        
        let inputs = vec!["src.rs".to_string()];
        let key1 = compute_target_key(temp.path(), &inputs, "cargo build", &env1, "build", "default").unwrap().unwrap();
        let key2 = compute_target_key(temp.path(), &inputs, "cargo build", &env2, "build", "default").unwrap().unwrap();
        
        assert_ne!(key1.key, key2.key, "Different env should produce different key");
        assert_ne!(key1.env_hash, key2.env_hash);
        // Inputs and command should be the same
        assert_eq!(key1.inputs_hash, key2.inputs_hash);
        assert_eq!(key1.command_hash, key2.command_hash);
    }
    
    // ========================================================================
    // Parallel Hashing Tests
    // ========================================================================
    
    #[test]
    fn test_parallel_hash_determinism() {
        // Create many files to trigger parallel hashing (threshold is 10)
        let temp = TempDir::new().unwrap();
        
        for i in 0..50 {
            let file_path = temp.path().join(format!("file_{:03}.txt", i));
            fs::write(&file_path, format!("content for file {}", i)).unwrap();
        }
        
        let patterns = vec!["*.txt".to_string()];
        
        // Run multiple times to ensure parallel execution is deterministic
        let hash1 = hash_file_patterns(temp.path(), &patterns, "test").unwrap().unwrap();
        let hash2 = hash_file_patterns(temp.path(), &patterns, "test").unwrap().unwrap();
        let hash3 = hash_file_patterns(temp.path(), &patterns, "test").unwrap().unwrap();
        
        assert_eq!(hash1, hash2);
        assert_eq!(hash2, hash3);
        assert_eq!(hash1.len(), 64);
    }
    
    #[test]
    fn test_parallel_vs_sequential_same_result() {
        // Test that parallel and sequential produce the same hash
        // We can't directly control the threshold, but we can verify
        // that a small set (sequential) and large set (parallel) are
        // both deterministic
        let temp = TempDir::new().unwrap();
        
        // Create exactly 9 files (below threshold - sequential)
        for i in 0..9 {
            fs::write(temp.path().join(format!("small_{}.txt", i)), format!("c{}", i)).unwrap();
        }
        let small_hash = hash_file_patterns(temp.path(), &["small_*.txt".to_string()], "test").unwrap().unwrap();
        
        // Verify sequential is deterministic
        let small_hash2 = hash_file_patterns(temp.path(), &["small_*.txt".to_string()], "test").unwrap().unwrap();
        assert_eq!(small_hash, small_hash2, "Sequential hashing should be deterministic");
        
        // Create 11 more files (total would be 20 if we used *)
        // But we create separate pattern to test parallel
        for i in 0..11 {
            fs::write(temp.path().join(format!("large_{}.txt", i)), format!("c{}", i)).unwrap();
        }
        let large_hash = hash_file_patterns(temp.path(), &["large_*.txt".to_string()], "test").unwrap().unwrap();
        let large_hash2 = hash_file_patterns(temp.path(), &["large_*.txt".to_string()], "test").unwrap().unwrap();
        assert_eq!(large_hash, large_hash2, "Parallel hashing should be deterministic");
    }
    
    // ========================================================================
    // Nested Directory Tests
    // ========================================================================
    
    #[test]
    fn test_nested_directories_with_glob() {
        let temp = TempDir::new().unwrap();
        
        // Create nested structure
        let src = temp.path().join("src");
        let src_utils = src.join("utils");
        fs::create_dir_all(&src_utils).unwrap();
        
        fs::write(src.join("main.rs"), "fn main() {}").unwrap();
        fs::write(src.join("lib.rs"), "// lib").unwrap();
        fs::write(src_utils.join("helpers.rs"), "// helpers").unwrap();
        
        // Should match all .rs files recursively
        let patterns = vec!["src/**/*.rs".to_string()];
        let result = hash_file_patterns(temp.path(), &patterns, "test").unwrap();
        
        assert!(result.is_some(), "Should match .rs files in nested directories");
    }
    
    #[test]
    fn test_deeply_nested_structure() {
        let temp = TempDir::new().unwrap();
        
        // Create deep nesting
        let deep = temp.path().join("a/b/c/d/e");
        fs::create_dir_all(&deep).unwrap();
        fs::write(deep.join("deep.txt"), "very deep").unwrap();
        
        let patterns = vec!["a/**/*".to_string()];
        let result = hash_file_patterns(temp.path(), &patterns, "test").unwrap();
        
        assert!(result.is_some(), "Should match files in deeply nested directories");
    }
    
    // ========================================================================
    // Edge Case Tests
    // ========================================================================
    
    #[test]
    fn test_file_with_special_characters_in_name() {
        let temp = TempDir::new().unwrap();
        
        // File with spaces and special chars
        fs::write(temp.path().join("file with spaces.txt"), "content").unwrap();
        fs::write(temp.path().join("file-with-dashes.txt"), "content").unwrap();
        fs::write(temp.path().join("file_with_underscores.txt"), "content").unwrap();
        
        let patterns = vec!["*.txt".to_string()];
        let result = hash_file_patterns(temp.path(), &patterns, "test").unwrap();
        
        assert!(result.is_some(), "Should handle files with special characters");
    }
    
    #[test]
    fn test_hidden_files() {
        let temp = TempDir::new().unwrap();
        
        fs::write(temp.path().join(".hidden"), "secret").unwrap();
        fs::write(temp.path().join(".gitignore"), "target/").unwrap();
        
        // .* should match hidden files
        let patterns = vec![".*".to_string()];
        let result = hash_file_patterns(temp.path(), &patterns, "test").unwrap();
        
        assert!(result.is_some(), "Should match hidden files with .* pattern");
    }
    
    #[test]
    fn test_pattern_order_does_not_affect_hash() {
        let temp = TempDir::new().unwrap();
        
        fs::write(temp.path().join("a.txt"), "a").unwrap();
        fs::write(temp.path().join("b.rs"), "b").unwrap();
        
        let patterns1 = vec!["*.txt".to_string(), "*.rs".to_string()];
        let patterns2 = vec!["*.rs".to_string(), "*.txt".to_string()];
        
        let hash1 = hash_file_patterns(temp.path(), &patterns1, "test").unwrap().unwrap();
        let hash2 = hash_file_patterns(temp.path(), &patterns2, "test").unwrap().unwrap();
        
        assert_eq!(hash1, hash2, 
            "Pattern order should not affect final hash (files are sorted)");
    }
}
