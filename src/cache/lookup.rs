//! SPDX-License-Identifier: MIT OR Apache-2.0

//! Cache lookup for checking if a target's outputs are already cached
//!
//! The lookup process:
//! 1. Compute target key from inputs/command/env
//! 2. Check local cache metadata for key
//! 3. If found, verify content_hash exists in parcel store
//! 4. Return cache hit with parcel reference, or miss

use std::path::Path;
use serde::{Deserialize, Serialize};

use super::TargetKey;
use super::store::{CacheStore, CacheEntry, VcsMetadata};

/// Result of a cache lookup
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CacheResult {
    /// Cache hit - outputs are available
    Hit {
        /// The target key that matched
        target_key: TargetKey,
        /// Content hash of the cached outputs
        content_hash: String,
        /// Parcel reference for retrieving outputs
        parcel_ref: Option<String>,
    },
    /// Cache miss - target needs to be executed
    Miss {
        /// The computed target key
        target_key: TargetKey,
        /// Reason for the miss
        reason: CacheMissReason,
    },
}

/// Reason for a cache miss
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CacheMissReason {
    /// No cache entry exists for this target key
    NotCached,
    /// Cache entry exists but content is no longer available
    ContentMissing,
    /// Cache was explicitly bypassed (--no-cache)
    Bypassed,
    /// No inputs/outputs declared (uncacheable)
    Uncacheable,
}

impl CacheResult {
    pub fn is_hit(&self) -> bool {
        matches!(self, CacheResult::Hit { .. })
    }
    
    pub fn is_miss(&self) -> bool {
        matches!(self, CacheResult::Miss { .. })
    }
}

/// Cache lookup service
pub struct CacheLookup {
    store: CacheStore,
    no_cache: bool,
}

impl CacheLookup {
    /// Create a new cache lookup service
    pub fn new(cache_dir: &Path) -> Result<Self, String> {
        Ok(Self {
            store: CacheStore::new(cache_dir)?,
            no_cache: false,
        })
    }
    
    /// Create a cache lookup that always returns miss (for --no-cache)
    pub fn disabled() -> Self {
        Self {
            store: CacheStore::in_memory(),
            no_cache: true,
        }
    }
    
    /// Bypass cache for this lookup instance
    pub fn with_no_cache(mut self, no_cache: bool) -> Self {
        self.no_cache = no_cache;
        self
    }
    
    /// Look up a target key in the cache
    pub fn lookup(&self, target_key: &TargetKey) -> CacheResult {
        // If caching is disabled, always return miss
        if self.no_cache {
            return CacheResult::Miss {
                target_key: target_key.clone(),
                reason: CacheMissReason::Bypassed,
            };
        }
        
        // Check local cache store
        match self.store.get(&target_key.key) {
            Some(entry) => {
                // Found a cache entry. Output content is verified by the caller
                // (the engine re-hashes the declared outputs against
                // `content_hash` before treating this as a hit).
                CacheResult::Hit {
                    target_key: target_key.clone(),
                    content_hash: entry.content_hash.clone(),
                    parcel_ref: entry.parcel_ref.clone(),
                }
            }
            None => {
                CacheResult::Miss {
                    target_key: target_key.clone(),
                    reason: CacheMissReason::NotCached,
                }
            }
        }
    }
    
    /// Store a cache entry after successful target execution
    pub fn store(
        &self,
        target_key: &TargetKey,
        content_hash: String,
        parcel_ref: Option<String>,
        stage_name: Option<String>,
        stage_variant: Option<String>,
    ) -> Result<(), String> {
        self.store_with_vcs(target_key, content_hash, parcel_ref, stage_name, stage_variant, None)
    }
    
    /// Store a cache entry with VCS metadata capture
    pub fn store_with_vcs(
        &self,
        target_key: &TargetKey,
        content_hash: String,
        parcel_ref: Option<String>,
        stage_name: Option<String>,
        stage_variant: Option<String>,
        workspace: Option<&std::path::Path>,
    ) -> Result<(), String> {
        if self.no_cache {
            return Ok(());  // Silently skip if caching disabled
        }
        
        // Capture VCS metadata if workspace provided
        let vcs = workspace.and_then(VcsMetadata::capture);
        
        let entry = CacheEntry {
            target_key: target_key.key.clone(),
            content_hash,
            parcel_ref,
            stage_name,
            stage_variant,
            created_at: chrono::Utc::now(),
            vcs,
            invalidated_at: None,
            invalidation_reason: None,
        };
        
        self.store.put(&target_key.key, entry)
    }
    
    /// Verify that parcel content matches the stored content hash.
    /// 
    /// Call this when retrieving a parcel from remote cache to detect
    /// cache poisoning or corruption.
    /// 
    /// Returns Ok(true) if content matches, Ok(false) if mismatch.
    /// On mismatch, the cache entry is marked invalid (soft delete) for post-mortem.
    pub fn verify_content(&self, target_key: &TargetKey, content: &[u8]) -> Result<bool, String> {
        use sha2::{Sha256, Digest};
        
        let entry = match self.store.get(&target_key.key) {
            Some(e) => e,
            None => return Ok(false), // No entry to verify against
        };
        
        // Compute SHA256 of actual content
        let mut hasher = Sha256::new();
        hasher.update(content);
        let computed_hash = hex::encode(hasher.finalize());
        
        // Compare with stored hash
        if computed_hash == entry.content_hash {
            Ok(true)
        } else {
            // Content mismatch - soft delete for post-mortem debugging
            let reason = format!(
                "content hash mismatch: expected {}, got {}",
                &entry.content_hash[..12],
                &computed_hash[..12]
            );
            eprintln!(
                "⚠️  Cache verification failed for {}: {}",
                target_key.short(),
                reason
            );
            self.store.mark_invalid(&target_key.key, &reason)?;
            Ok(false)
        }
    }
    
    /// Mark a cache entry as invalid (soft delete for post-mortem debugging)
    /// Entry remains until TTL/LRU cleanup but won't be used for cache hits
    pub fn invalidate(&self, target_key: &TargetKey, reason: &str) -> Result<(), String> {
        self.store.mark_invalid(&target_key.key, reason)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    
    fn make_test_key() -> TargetKey {
        TargetKey {
            inputs_hash: "inputs123".to_string(),
            command_hash: "cmd456".to_string(),
            env_hash: "env789".to_string(),
            key: "testkey000".to_string(),
        }
    }
    
    #[test]
    fn test_lookup_miss_when_not_cached() {
        let temp = TempDir::new().unwrap();
        let lookup = CacheLookup::new(temp.path()).unwrap();
        
        let key = make_test_key();
        let result = lookup.lookup(&key);
        
        assert!(result.is_miss());
        match result {
            CacheResult::Miss { reason, .. } => {
                assert!(matches!(reason, CacheMissReason::NotCached));
            }
            _ => panic!("Expected miss"),
        }
    }
    
    #[test]
    fn test_lookup_hit_after_store() {
        let temp = TempDir::new().unwrap();
        let lookup = CacheLookup::new(temp.path()).unwrap();
        
        let key = make_test_key();
        lookup.store(&key, "contenthash".to_string(), Some("parcel://test".to_string()), Some("build".to_string()), Some("release".to_string())).unwrap();
        
        let result = lookup.lookup(&key);
        
        assert!(result.is_hit());
        match result {
            CacheResult::Hit { content_hash, parcel_ref, .. } => {
                assert_eq!(content_hash, "contenthash");
                assert_eq!(parcel_ref, Some("parcel://test".to_string()));
            }
            _ => panic!("Expected hit"),
        }
    }
    
    #[test]
    fn test_no_cache_always_misses() {
        let lookup = CacheLookup::disabled();
        
        let key = make_test_key();
        let result = lookup.lookup(&key);
        
        assert!(result.is_miss());
        match result {
            CacheResult::Miss { reason, .. } => {
                assert!(matches!(reason, CacheMissReason::Bypassed));
            }
            _ => panic!("Expected miss"),
        }
    }
    
    #[test]
    fn test_no_cache_skips_store() {
        let lookup = CacheLookup::disabled();
        let key = make_test_key();
        
        // Store should succeed (no-op)
        lookup.store(&key, "hash".to_string(), None, None, None).unwrap();
        
        // But lookup still misses
        assert!(lookup.lookup(&key).is_miss());
    }
    
    #[test]
    fn test_verify_content_valid() {
        use sha2::{Sha256, Digest};
        
        let temp = TempDir::new().unwrap();
        let lookup = CacheLookup::new(temp.path()).unwrap();
        
        let key = make_test_key();
        let content = b"test parcel content";
        
        // Compute correct hash
        let mut hasher = Sha256::new();
        hasher.update(content);
        let content_hash = hex::encode(hasher.finalize());
        
        // Store with correct hash
        lookup.store(&key, content_hash, None, None, None).unwrap();
        
        // Verification should pass
        assert!(lookup.verify_content(&key, content).unwrap());
        assert!(lookup.lookup(&key).is_hit()); // Entry should still exist
    }
    
    #[test]
    fn test_verify_content_invalid_soft_deletes_entry() {
        use sha2::{Sha256, Digest};
        
        let temp = TempDir::new().unwrap();
        let lookup = CacheLookup::new(temp.path()).unwrap();
        
        let key = make_test_key();
        let content = b"test parcel content";
        let tampered_content = b"tampered content";
        
        // Compute hash of original content
        let mut hasher = Sha256::new();
        hasher.update(content);
        let content_hash = hex::encode(hasher.finalize());
        
        // Store with original hash
        lookup.store(&key, content_hash, None, None, None).unwrap();
        assert!(lookup.lookup(&key).is_hit());
        
        // Verify with tampered content - should fail and soft-delete entry
        assert!(!lookup.verify_content(&key, tampered_content).unwrap());
        assert!(lookup.lookup(&key).is_miss()); // Entry should appear as miss
        
        // But entry still exists for post-mortem (soft delete)
        let entry = lookup.store.get_including_invalid(&key.key);
        assert!(entry.is_some());
        let entry = entry.unwrap();
        assert!(entry.invalidated_at.is_some());
        assert!(entry.invalidation_reason.as_ref().unwrap().contains("content hash mismatch"));
    }
}
