//! SPDX-License-Identifier: MIT OR Apache-2.0

//! Farm Parser Library
//!
//! A parser for configuration files (Farmfile) that uses a TOML-like syntax
//! for defining operations, variant, and dependencies.
//!
//! # Example
//!
//! ```
//! use farm::vendor::parse::parse_plan;
//!
//! let content = concat!(
//!     "version: 1\n",
//!     "\n",
//!     "[variant]\n",
//!     "debug\n",
//!     "release\n",
//!     "\n",
//!     "[operation.build]\n",
//!     "work: echo 'Building project...'\n",
//!     "\n",
//!     "[operation.test]\n",
//!     "work: echo 'Running tests...'\n",
//!     "after: build\n"
//! );
//!
//! match parse_plan(content) {
//!     Ok(plan) => {
//!         assert_eq!(plan.operations.len(), 2);
//!         assert_eq!(plan.variants, vec!["debug", "release"]);
//!         assert_eq!(plan.operations[0].label, "build");
//!         assert_eq!(plan.operations[1].label, "test");
//!     }
//!     Err(e) => panic!("Parse failed: {}", e)
//! }
//! ```

/// Crate version from Cargo.toml
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub mod plan;
pub mod task;
pub mod parser;

pub use plan::{Plan, Operation, EnvValue, CacheConfig};
pub use task::Task;
pub use parser::{parse_plan, ParseError, ParseErrorKind};
