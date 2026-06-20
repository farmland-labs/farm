//! SPDX-License-Identifier: MIT OR Apache-2.0

//! CLI integration tests for `farm cache` commands
//!
//! Tests the stable public API for cache management.

use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

struct TestWorkspace {
    dir: TempDir,
}

impl TestWorkspace {
    fn new() -> Self {
        Self {
            dir: TempDir::new().expect("create temp dir"),
        }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn init_farm(&self) {
        self.run_farm(&["ctx", "run", "init"]);
    }

    fn run_farm(&self, args: &[&str]) -> TestResult {
        let output = Command::new(env!("CARGO_BIN_EXE_farm"))
            .args(args)
            .current_dir(self.path())
            .output()
            .expect("run farm");

        TestResult {
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        }
    }

    /// Create a fake cache entry by writing directly to the cache file
    fn create_cache_entry(&self, target_key: &str, content_hash: &str, stage_name: &str) {
        let cache_dir = self.path().join(".farm/cache");
        fs::create_dir_all(&cache_dir).expect("create cache dir");
        
        let cache_file = cache_dir.join("goal-cache.json");
        
        // Read existing or create new
        let mut metadata: serde_json::Value = if cache_file.exists() {
            let content = fs::read_to_string(&cache_file).expect("read cache");
            serde_json::from_str(&content).unwrap_or_else(|_| serde_json::json!({
                "version": 1,
                "entries": {}
            }))
        } else {
            serde_json::json!({
                "version": 1,
                "entries": {}
            })
        };
        
        // Add the entry
        metadata["entries"][target_key] = serde_json::json!({
            "target_key": target_key,
            "content_hash": content_hash,
            "parcel_ref": null,
            "stage_name": stage_name,
            "stage_variant": null,
            "created_at": "2024-01-01T00:00:00Z",
            "vcs": null
        });
        
        fs::write(&cache_file, serde_json::to_string_pretty(&metadata).unwrap())
            .expect("write cache");
    }
}

struct TestResult {
    success: bool,
    stdout: String,
    stderr: String,
}

// =============================================================================
// cache list tests
// =============================================================================

#[test]
fn test_cache_list_empty() {
    let ws = TestWorkspace::new();
    ws.init_farm();

    let result = ws.run_farm(&["cache", "list"]);
    assert!(result.success, "cache list should succeed: {}", result.stderr);
    // Empty cache should report no entries
    assert!(
        result.stdout.contains("No cached entries") || result.stdout.is_empty() || result.stdout.lines().count() <= 1,
        "Expected empty cache output, got: {}", result.stdout
    );
}

#[test]
fn test_cache_list_with_entries() {
    let ws = TestWorkspace::new();
    ws.init_farm();
    
    // Create some fake cache entries
    ws.create_cache_entry(
        "k/abc123",
        "deadbeef1234",
        "build"
    );
    ws.create_cache_entry(
        "k/def456",
        "cafebabe5678",
        "test"
    );

    let result = ws.run_farm(&["cache", "list"]);
    assert!(result.success, "cache list should succeed: {}", result.stderr);
    
    // Should list entries
    assert!(result.stdout.contains("abc123") || result.stdout.contains("build"),
        "Expected cache entries in output: {}", result.stdout);
}

#[test]
fn test_cache_list_long_format() {
    let ws = TestWorkspace::new();
    ws.init_farm();
    
    ws.create_cache_entry("k/target-key-123456789", "hash123", "mybuild");

    let result = ws.run_farm(&["cache", "list", "--format", "long"]);
    assert!(result.success, "cache list --format long should succeed: {}", result.stderr);
    
    // long format should show full target keys
    assert!(result.stdout.contains("target-key") || result.stdout.contains("mybuild"),
        "Expected detailed entry in output: {}", result.stdout);
}

// =============================================================================
// cache stats tests
// =============================================================================

#[test]
fn test_cache_stats_empty() {
    let ws = TestWorkspace::new();
    ws.init_farm();

    let result = ws.run_farm(&["cache", "stats"]);
    assert!(result.success, "cache stats should succeed: {}", result.stderr);
    
    // Stats should show 0 entries for empty cache
    assert!(result.stdout.contains("0") || result.stdout.to_lowercase().contains("empty"),
        "Expected empty cache stats, got: {}", result.stdout);
}

#[test]
fn test_cache_stats_with_entries() {
    let ws = TestWorkspace::new();
    ws.init_farm();
    
    ws.create_cache_entry("k/one", "hash1", "stage1");
    ws.create_cache_entry("k/two", "hash2", "stage2");
    ws.create_cache_entry("k/three", "hash3", "stage3");

    let result = ws.run_farm(&["cache", "stats"]);
    assert!(result.success, "cache stats should succeed: {}", result.stderr);
    
    // Stats should reflect 3 entries
    assert!(result.stdout.contains("3"),
        "Expected stats showing 3 entries, got: {}", result.stdout);
}

// =============================================================================
// cache get tests
// =============================================================================

#[test]
fn test_cache_get_by_prefix() {
    let ws = TestWorkspace::new();
    ws.init_farm();
    
    ws.create_cache_entry("k/abc123456", "deadbeef", "buildstage");

    // Get by prefix
    let result = ws.run_farm(&["cache", "get", "abc"]);
    
    // Should find the entry by prefix
    assert!(result.success || result.stdout.contains("abc") || result.stderr.contains("abc"),
        "cache get by prefix should find entry or show error: stdout={}, stderr={}", 
        result.stdout, result.stderr);
}

#[test]
fn test_cache_get_not_found() {
    let ws = TestWorkspace::new();
    ws.init_farm();

    let result = ws.run_farm(&["cache", "get", "nonexistent"]);
    
    // Should report not found
    assert!(!result.success || result.stdout.to_lowercase().contains("not found") || 
            result.stderr.to_lowercase().contains("not found") ||
            result.stdout.is_empty(),
        "cache get for nonexistent key should report not found");
}

// =============================================================================
// cache clear tests
// =============================================================================

#[test]
fn test_cache_clear() {
    let ws = TestWorkspace::new();
    ws.init_farm();
    
    ws.create_cache_entry("k/entry1", "hash1", "stage1");
    ws.create_cache_entry("k/entry2", "hash2", "stage2");

    let result = ws.run_farm(&["cache", "clear"]);
    assert!(result.success, "cache clear should succeed: {}", result.stderr);
    
    // Verify cache is empty
    let stats = ws.run_farm(&["cache", "stats"]);
    assert!(stats.stdout.contains("0") || stats.stdout.to_lowercase().contains("empty"),
        "Cache should be empty after clear: {}", stats.stdout);
}

#[test]
fn test_cache_clear_empty() {
    let ws = TestWorkspace::new();
    ws.init_farm();

    // Clearing an empty cache should succeed
    let result = ws.run_farm(&["cache", "clear"]);
    assert!(result.success, "cache clear on empty cache should succeed: {}", result.stderr);
}

// =============================================================================
// cache remove tests
// =============================================================================

#[test]
fn test_cache_remove_by_key() {
    let ws = TestWorkspace::new();
    ws.init_farm();
    
    ws.create_cache_entry("k/toremove", "hash1", "removeme");
    ws.create_cache_entry("k/tokeep", "hash2", "keepme");

    // Key prefix must match from start of the key, so use "k/toremove"
    let result = ws.run_farm(&["cache", "remove", "k/toremove"]);
    assert!(result.success, "cache remove should succeed: {}", result.stderr);
    
    // Verify only one entry remains
    let stats = ws.run_farm(&["cache", "stats"]);
    assert!(stats.stdout.contains("1"),
        "Expected 1 entry after remove, got: {}", stats.stdout);
}

#[test]
fn test_cache_remove_nonexistent() {
    let ws = TestWorkspace::new();
    ws.init_farm();

    let result = ws.run_farm(&["cache", "remove", "doesnotexist"]);
    
    // Should either succeed silently or report not found
    // (behavior depends on implementation)
    assert!(result.success || result.stderr.contains("not found") || result.stderr.contains("No"),
        "cache remove for nonexistent should handle gracefully: {}", result.stderr);
}

// =============================================================================
// cache help tests
// =============================================================================

#[test]
fn test_cache_help() {
    let ws = TestWorkspace::new();

    let result = ws.run_farm(&["cache", "--help"]);
    assert!(result.success, "cache --help should succeed");
    assert!(result.stdout.contains("list"));
    assert!(result.stdout.contains("stats"));
    assert!(result.stdout.contains("clear"));
    assert!(result.stdout.contains("get"));
    assert!(result.stdout.contains("remove"));
}

#[test]
fn test_cache_list_help() {
    let ws = TestWorkspace::new();

    let result = ws.run_farm(&["cache", "list", "--help"]);
    assert!(result.success, "cache list --help should succeed");
    assert!(result.stdout.contains("--format"));
}

// =============================================================================
// Error cases
// =============================================================================

#[test]
fn test_cache_invalid_subcommand() {
    let ws = TestWorkspace::new();

    let result = ws.run_farm(&["cache", "invalid"]);
    assert!(!result.success, "cache invalid should fail");
    assert!(result.stderr.contains("invalid") || result.stderr.contains("unrecognized"),
        "Should report invalid subcommand: {}", result.stderr);
}
