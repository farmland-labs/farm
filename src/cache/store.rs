//! SPDX-License-Identifier: MIT OR Apache-2.0

//! Local cache metadata storage
//!
//! Stores target_key → content_hash mappings in a simple JSON file.
//! This is the local index that points to actual parcel content.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::RwLock;
use std::process::Command;
use serde::{Deserialize, Serialize};
use chrono::{DateTime, Utc};

/// VCS metadata for traceability (NOT part of cache key)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VcsMetadata {
    /// VCS type: "git", "hg", "svn", "p4", "none"
    pub vcs_type: String,
    /// Short revision/commit (e.g., "a1b2c3d")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision_short: Option<String>,
    /// Full revision/commit SHA
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    /// Branch/bookmark name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// Whether working directory has uncommitted changes
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dirty: Option<bool>,
    /// Tag description (e.g., "v1.2.3-5-ga1b2c3d")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    /// Commit date (ISO 8601 format)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit_date: Option<String>,
    /// Commit message (first line / subject)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl VcsMetadata {
    /// Capture VCS metadata from the current working directory
    pub fn capture(workspace: &Path) -> Option<Self> {
        // Try Git first
        if workspace.join(".git").exists() || Self::is_git_worktree(workspace) {
            return Self::capture_git(workspace);
        }
        
        // Try Mercurial
        if workspace.join(".hg").exists() {
            return Self::capture_hg(workspace);
        }
        
        // Try SVN
        if workspace.join(".svn").exists() {
            return Self::capture_svn(workspace);
        }
        
        // No VCS detected
        None
    }
    
    fn is_git_worktree(workspace: &Path) -> bool {
        Command::new("git")
            .args(["rev-parse", "--git-dir"])
            .current_dir(workspace)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
    
    fn capture_git(workspace: &Path) -> Option<Self> {
        let run = |args: &[&str]| -> Option<String> {
            Command::new("git")
                .args(args)
                .current_dir(workspace)
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .filter(|s| !s.is_empty())
        };
        
        let revision = run(&["rev-parse", "HEAD"]);
        let revision_short = run(&["rev-parse", "--short", "HEAD"]);
        let branch = run(&["rev-parse", "--abbrev-ref", "HEAD"])
            .map(|b| if b == "HEAD" { "(detached)".to_string() } else { b });
        let dirty = Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(workspace)
            .output()
            .ok()
            .map(|o| !o.stdout.is_empty());
        let tag = run(&["describe", "--tags", "--always"]);
        let commit_date = run(&["log", "-1", "--format=%cI"]); // ISO 8601 format
        let message = run(&["log", "-1", "--format=%s"]);      // Subject line
        
        Some(VcsMetadata {
            vcs_type: "git".to_string(),
            revision,
            revision_short,
            branch,
            dirty,
            tag,
            commit_date,
            message,
        })
    }
    
    fn capture_hg(workspace: &Path) -> Option<Self> {
        let run = |args: &[&str]| -> Option<String> {
            Command::new("hg")
                .args(args)
                .current_dir(workspace)
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .filter(|s| !s.is_empty())
        };
        
        let revision = run(&["id", "-i"]);
        let revision_short = revision.as_ref().map(|r| r.chars().take(12).collect());
        let branch = run(&["branch"]);
        let dirty = revision.as_ref().map(|r| r.ends_with('+'));
        let commit_date = run(&["log", "-r", ".", "--template", "{date|isodate}"]);
        let message = run(&["log", "-r", ".", "--template", "{desc|firstline}"]);
        
        Some(VcsMetadata {
            vcs_type: "hg".to_string(),
            revision,
            revision_short,
            branch,
            dirty,
            tag: None,
            commit_date,
            message,
        })
    }
    
    fn capture_svn(workspace: &Path) -> Option<Self> {
        let run = |args: &[&str]| -> Option<String> {
            Command::new("svn")
                .args(args)
                .current_dir(workspace)
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .filter(|s| !s.is_empty())
        };
        
        let revision = run(&["info", "--show-item", "revision"]);
        let dirty = Command::new("svn")
            .args(["status"])
            .current_dir(workspace)
            .output()
            .ok()
            .map(|o| !o.stdout.is_empty());
        let commit_date = run(&["info", "--show-item", "last-changed-date"]);
        
        Some(VcsMetadata {
            vcs_type: "svn".to_string(),
            revision: revision.clone(),
            revision_short: revision,
            branch: None, // SVN uses path-based branches
            dirty,
            tag: None,
            commit_date,
            message: None, // SVN log requires network access, skip for now
        })
    }
}

/// A single cache entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEntry {
    /// The target key this entry belongs to
    pub target_key: String,
    /// Content hash of the cached outputs
    pub content_hash: String,
    /// Optional parcel reference for retrieving outputs
    pub parcel_ref: Option<String>,
    /// Stage name for human-readable identification
    #[serde(default)]
    pub stage_name: Option<String>,
    /// Variant used for this cache entry
    #[serde(default)]
    pub stage_variant: Option<String>,
    /// When this entry was created
    pub created_at: DateTime<Utc>,
    /// VCS metadata at time of cache (for traceability, NOT cache key)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vcs: Option<VcsMetadata>,
    /// When this entry was invalidated (soft delete for post-mortem)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invalidated_at: Option<DateTime<Utc>>,
    /// Reason for invalidation
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invalidation_reason: Option<String>,
}

/// Cache metadata stored on disk
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct CacheMetadata {
    /// Version for future schema migrations
    version: u32,
    /// Map of target_key → cache entry
    entries: HashMap<String, CacheEntry>,
}

impl CacheMetadata {
    fn new() -> Self {
        Self {
            version: 1,
            entries: HashMap::new(),
        }
    }
}

/// Local cache store for target key metadata
pub struct CacheStore {
    /// Path to the cache metadata file
    cache_file: Option<PathBuf>,
    /// In-memory cache (for tests or disabled mode)
    memory: RwLock<CacheMetadata>,
}

impl CacheStore {
    /// Create a new file-backed cache store
    pub fn new(cache_dir: &Path) -> Result<Self, String> {
        // Ensure cache directory exists
        fs::create_dir_all(cache_dir)
            .map_err(|e| format!("Failed to create cache directory: {}", e))?;
        
        let cache_file = cache_dir.join("goal-cache.json");
        
        // Load existing cache or create new
        let metadata = if cache_file.exists() {
            let content = fs::read_to_string(&cache_file)
                .map_err(|e| format!("Failed to read cache file: {}", e))?;
            serde_json::from_str(&content)
                .map_err(|e| format!("Failed to parse cache file: {}", e))?
        } else {
            CacheMetadata::new()
        };
        
        Ok(Self {
            cache_file: Some(cache_file),
            memory: RwLock::new(metadata),
        })
    }
    
    /// Create an in-memory only cache store (for tests or --no-cache)
    pub fn in_memory() -> Self {
        Self {
            cache_file: None,
            memory: RwLock::new(CacheMetadata::new()),
        }
    }
    
    /// Get a cache entry by target key (skips invalidated entries)
    pub fn get(&self, target_key: &str) -> Option<CacheEntry> {
        let metadata = self.memory.read().unwrap();
        metadata.entries.get(target_key)
            .filter(|e| e.invalidated_at.is_none())
            .cloned()
    }
    
    /// Get a cache entry including invalidated ones (for debugging)
    pub fn get_including_invalid(&self, target_key: &str) -> Option<CacheEntry> {
        let metadata = self.memory.read().unwrap();
        metadata.entries.get(target_key).cloned()
    }
    
    /// Store a cache entry
    pub fn put(&self, target_key: &str, entry: CacheEntry) -> Result<(), String> {
        {
            let mut metadata = self.memory.write().unwrap();
            metadata.entries.insert(target_key.to_string(), entry);
        }
        
        // Persist to disk if file-backed
        self.save()
    }
    
    /// Remove a cache entry
    pub fn remove(&self, target_key: &str) -> Result<Option<CacheEntry>, String> {
        let entry = {
            let mut metadata = self.memory.write().unwrap();
            metadata.entries.remove(target_key)
        };
        
        self.save()?;
        Ok(entry)
    }
    
    /// Mark a cache entry as invalid (soft delete for post-mortem debugging)
    /// Entry remains for TTL/LRU cleanup but won't be used for cache hits
    pub fn mark_invalid(&self, target_key: &str, reason: &str) -> Result<(), String> {
        {
            let mut metadata = self.memory.write().unwrap();
            if let Some(entry) = metadata.entries.get_mut(target_key) {
                entry.invalidated_at = Some(Utc::now());
                entry.invalidation_reason = Some(reason.to_string());
            }
        }
        self.save()
    }
    
    /// List all cached target keys
    pub fn list_keys(&self) -> Vec<String> {
        let metadata = self.memory.read().unwrap();
        metadata.entries.keys().cloned().collect()
    }
    
    /// List all cache entries
    pub fn list_entries(&self) -> Vec<CacheEntry> {
        let metadata = self.memory.read().unwrap();
        metadata.entries.values().cloned().collect()
    }

    /// Get the number of cached entries
    pub fn len(&self) -> usize {
        let metadata = self.memory.read().unwrap();
        metadata.entries.len()
    }
    
    /// Check if the cache is empty
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    
    /// Clear all cache entries
    pub fn clear(&self) -> Result<(), String> {
        {
            let mut metadata = self.memory.write().unwrap();
            metadata.entries.clear();
        }
        self.save()
    }
    
    /// Save cache metadata to disk
    fn save(&self) -> Result<(), String> {
        if let Some(cache_file) = &self.cache_file {
            let metadata = self.memory.read().unwrap();
            let content = serde_json::to_string_pretty(&*metadata)
                .map_err(|e| format!("Failed to serialize cache: {}", e))?;
            fs::write(cache_file, content)
                .map_err(|e| format!("Failed to write cache file: {}", e))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    
    fn make_entry(key: &str) -> CacheEntry {
        CacheEntry {
            target_key: key.to_string(),
            content_hash: format!("hash-{}", key),
            parcel_ref: Some(format!("parcel://{}", key)),
            stage_name: Some(format!("stage-{}", key)),
            stage_variant: Some("default".to_string()),
            created_at: Utc::now(),
            vcs: None,
            invalidated_at: None,
            invalidation_reason: None,
        }
    }
    
    #[test]
    fn test_in_memory_store() {
        let store = CacheStore::in_memory();
        
        assert!(store.is_empty());
        assert!(store.get("key1").is_none());
        
        store.put("key1", make_entry("key1")).unwrap();
        
        assert_eq!(store.len(), 1);
        assert!(store.get("key1").is_some());
    }
    
    #[test]
    fn test_file_backed_store() {
        let temp = TempDir::new().unwrap();
        
        // Create store and add entry
        {
            let store = CacheStore::new(temp.path()).unwrap();
            store.put("key1", make_entry("key1")).unwrap();
            assert_eq!(store.len(), 1);
        }
        
        // Reopen store and verify persistence
        {
            let store = CacheStore::new(temp.path()).unwrap();
            assert_eq!(store.len(), 1);
            let entry = store.get("key1").unwrap();
            assert_eq!(entry.content_hash, "hash-key1");
        }
    }
    
    #[test]
    fn test_store_remove() {
        let store = CacheStore::in_memory();
        
        store.put("key1", make_entry("key1")).unwrap();
        store.put("key2", make_entry("key2")).unwrap();
        assert_eq!(store.len(), 2);
        
        let removed = store.remove("key1").unwrap();
        assert!(removed.is_some());
        assert_eq!(store.len(), 1);
        assert!(store.get("key1").is_none());
        assert!(store.get("key2").is_some());
    }
    
    #[test]
    fn test_store_clear() {
        let store = CacheStore::in_memory();
        
        store.put("key1", make_entry("key1")).unwrap();
        store.put("key2", make_entry("key2")).unwrap();
        
        store.clear().unwrap();
        
        assert!(store.is_empty());
    }
    
    #[test]
    fn test_list_keys() {
        let store = CacheStore::in_memory();
        
        store.put("alpha", make_entry("alpha")).unwrap();
        store.put("beta", make_entry("beta")).unwrap();
        store.put("gamma", make_entry("gamma")).unwrap();
        
        let mut keys = store.list_keys();
        keys.sort();
        
        assert_eq!(keys, vec!["alpha", "beta", "gamma"]);
    }
    
    #[test]
    fn test_mark_invalid_soft_deletes() {
        let store = CacheStore::in_memory();
        
        store.put("key1", make_entry("key1")).unwrap();
        assert!(store.get("key1").is_some()); // Valid entry visible
        
        // Soft delete
        store.mark_invalid("key1", "content hash mismatch").unwrap();
        
        // Entry not visible through normal get
        assert!(store.get("key1").is_none());
        
        // But still accessible for debugging
        let entry = store.get_including_invalid("key1");
        assert!(entry.is_some());
        
        let entry = entry.unwrap();
        assert!(entry.invalidated_at.is_some());
        assert_eq!(entry.invalidation_reason, Some("content hash mismatch".to_string()));
    }
}
