//! SPDX-License-Identifier: MIT OR Apache-2.0

//! Vendored copies of external crates.
//!
//! Each child module here is a byte-for-byte copy of an upstream
//! source, kept in-tree so `farm` can be built and shipped
//! standalone without pulling each one as a separate Cargo
//! dependency. Drift between this copy and the upstream is a bug
//! — security-relevant modules (e.g. `protocol`) include their
//! own contract tests to catch it.
//!
//! Files in here are not the primary source of truth for the
//! types they hold; the corresponding workspace crate is. If you
//! need to change a type, change the workspace source first,
//! then re-sync.

pub mod log;
pub mod parse;
pub mod protocol;
