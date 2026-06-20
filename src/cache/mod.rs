//! SPDX-License-Identifier: MIT OR Apache-2.0

//! Cache module for target key computation and cache management
//!
//! Enables intelligent cache reuse by computing target keys from:
//! - Input file patterns (hashed content)
//! - Command string
//! - Declared environment variables

mod target_key;
mod lookup;
mod store;
pub mod analyze;

pub use target_key::{TargetKey, compute_target_key, hash_file_patterns};
pub use lookup::{CacheLookup, CacheResult};
pub use store::{CacheStore, CacheEntry, VcsMetadata};
pub use analyze::{
    check_dependency_declarations, 
    DependencyCheckResult, 
    DependencyWarning,
    WorkspaceSnapshot,
    SnapshotDiff,
    check_diff_coverage,
    prompt_continue,
};
