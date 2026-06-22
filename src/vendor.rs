//! SPDX-License-Identifier: MIT OR Apache-2.0

//! Self-contained in-tree modules.
//!
//! `parse` and `log` are vendored copies of upstream sources,
//! kept in-tree so `farm` can be built and shipped standalone
//! without pulling each one as a separate Cargo dependency.
//! `protocol` is the first-party schema for the orchestrator-to-
//! `farm` handoff; it sits here alongside the other self-contained
//! modules and carries its own contract tests.

pub mod log;
pub mod parse;
pub mod protocol;
