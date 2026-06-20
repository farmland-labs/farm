//! SPDX-License-Identifier: MIT OR Apache-2.0

//! `work_hash` — stable per-operation identity.
//!
//! A 6-hex-char hash derived from the *Farmfile-declared shape* of an
//! operation. Same operation across runs → same hash, even if input file
//! contents change. This is **not** the cache key (which is keyed on
//! input *contents*); it is a stable identifier for "the same operation"
//! across runs, independent of input file contents.
//!
//! ## Encoding
//!
//! 1. Build a canonical JSON document of the declared work — sorted
//!    map keys, declaration-order arrays for inputs/outputs/tasks
//!    (already user-ordered in the Farmfile), env *keys only* (values
//!    omitted, since values change without changing the operation's
//!    identity).
//! 2. BLAKE3-256 of the canonical bytes.
//! 3. Take the low 24 bits → 6 hex chars.
//!
//! 24 bits gives ~16M distinct values. The lookup space is per
//! `(tenant, goal)` so collisions inside that scope are vanishingly
//! rare; if a tenant ever hits one, the worst case is one operation
//! borrows another's baseline duration estimates. No
//! correctness invariant depends on this hash.
//!
//! ## What's intentionally *not* hashed
//!
//! - Input file contents (that's `target_key` / cache key).
//! - Env values (only env *keys* go in — values change without
//!   identity changing).
//! - Workspace path, build_id, timestamps, host.
//! - Variant — variants pick a different operation graph; the
//!   resulting Stage is what changes, not work_hash semantics.

use serde::Serialize;

use crate::stage::Stage;

/// Canonical input for hashing. Field declaration order is the
/// canonical order; serde_json preserves struct-field order on
/// serialization, which combined with sorted keys in the env map gives
/// a deterministic byte sequence without pulling in a JCS crate.
#[derive(Serialize)]
struct CanonicalWork<'a> {
    label: &'a str,
    inputs: &'a [String],
    outputs: &'a [String],
    depends: &'a [String],
    /// Env *keys* only, sorted. Values are intentionally excluded.
    env_keys: Vec<&'a str>,
    /// Task labels, in declaration order.
    tasks: Vec<String>,
}

/// Compute the 6-hex-char `work_hash` for an operation.
pub fn compute_work_hash(stage: &Stage) -> String {
    // Sort env keys so HashMap iteration order doesn't leak into the hash.
    let mut env_keys: Vec<&str> =
        stage.declared_env.keys().map(String::as_str).collect();
    env_keys.sort_unstable();

    let tasks: Vec<String> = stage.tasks.iter().map(|t| t.label()).collect();

    let canonical = CanonicalWork {
        label: &stage.label,
        inputs: &stage.inputs,
        outputs: &stage.outputs,
        depends: &stage.depends,
        env_keys,
        tasks,
    };

    let bytes = serde_json::to_vec(&canonical)
        .expect("CanonicalWork serializes infallibly: only strings + arrays");

    let hash = blake3::hash(&bytes);
    let full_hex = hash.to_hex();
    full_hex[..6].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vendor::parse::EnvValue;
    use std::collections::HashMap;

    fn stage(label: &str) -> Stage {
        Stage::new(label.to_string())
    }

    #[test]
    fn deterministic_across_calls() {
        let mut s = stage("compile");
        s.inputs.push("src/**".to_string());
        s.outputs.push("target/release/farm".to_string());
        s.declared_env
            .insert("RUSTFLAGS".to_string(), EnvValue::Capture);

        let a = compute_work_hash(&s);
        let b = compute_work_hash(&s);
        assert_eq!(a, b);
        assert_eq!(a.len(), 6);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn changes_when_inputs_change() {
        let mut a = stage("compile");
        a.inputs.push("src/**".to_string());
        let mut b = stage("compile");
        b.inputs.push("src/lib.rs".to_string());
        assert_ne!(compute_work_hash(&a), compute_work_hash(&b));
    }

    #[test]
    fn stable_across_env_value_changes() {
        // Env *values* must not affect work_hash — only keys do.
        let mut a = stage("test");
        a.declared_env.insert(
            "FLAG".to_string(),
            EnvValue::Explicit("on".to_string()),
        );
        let mut b = stage("test");
        b.declared_env.insert(
            "FLAG".to_string(),
            EnvValue::Explicit("off".to_string()),
        );
        assert_eq!(compute_work_hash(&a), compute_work_hash(&b));
    }

    #[test]
    fn changes_when_env_key_added() {
        let mut a = stage("test");
        a.declared_env.insert("A".to_string(), EnvValue::Capture);
        let mut b = stage("test");
        b.declared_env.insert("A".to_string(), EnvValue::Capture);
        b.declared_env.insert("B".to_string(), EnvValue::Capture);
        assert_ne!(compute_work_hash(&a), compute_work_hash(&b));
    }

    #[test]
    fn env_key_order_does_not_matter() {
        // HashMap iteration order is non-deterministic. The canonical
        // form must sort keys so insertion order doesn't leak.
        let mut a_env = HashMap::new();
        a_env.insert("Z".to_string(), EnvValue::Capture);
        a_env.insert("A".to_string(), EnvValue::Capture);
        a_env.insert("M".to_string(), EnvValue::Capture);

        let mut b_env = HashMap::new();
        b_env.insert("A".to_string(), EnvValue::Capture);
        b_env.insert("M".to_string(), EnvValue::Capture);
        b_env.insert("Z".to_string(), EnvValue::Capture);

        let mut a = stage("test");
        a.declared_env = a_env;
        let mut b = stage("test");
        b.declared_env = b_env;

        assert_eq!(compute_work_hash(&a), compute_work_hash(&b));
    }

    #[test]
    fn changes_when_label_changes() {
        let a = stage("build");
        let b = stage("test");
        assert_ne!(compute_work_hash(&a), compute_work_hash(&b));
    }
}
