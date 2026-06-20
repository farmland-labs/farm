//! SPDX-License-Identifier: MIT OR Apache-2.0

//! Farm - Build operations runner

use std::path::Path;

/// The version of the farm crate (from Cargo.toml)
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Write the internal notice file to the .farm directory.
/// This is called when the .farm directory is first created.
pub fn write_farm_notice(farm_dir: &Path) {
    let notice_path = farm_dir.join("INTERNAL-DO-NOT-RELY.md");
    // Always rewrite so the version + command surface stays in sync with the
    // binary that wrote this directory. Cheap (sub-1KB) and idempotent.
    let content = format!(
        r#"# .farm Directory - Internal Use Only

**WARNING: This directory structure is NOT a stable API.**

Created by farm v{version}

The contents and structure of `.farm/` are internal implementation details
of the farm build system. They may change between versions without notice.

## Do Not Rely On

- Directory names or hierarchy
- File formats or naming conventions
- The existence of any particular file or directory

## For Introspection

Use the `farm` CLI to inspect the context / farm-dir state instead of
reading these files directly:

If you are building tooling that needs to interact with farm state,
please use the CLI's JSON output format (--format json) which provides
a stable interface.

This directory is safe to delete - it will be recreated as needed.
"#,
        version = VERSION
    );
    let _ = std::fs::write(&notice_path, content);
}
pub mod cmd;
pub mod env;
pub mod task;
pub mod stage;
pub mod plan;
pub mod build_logger;
pub mod context;
pub mod engine;
// Re-export tracing's base log macros at the crate root so the
// `#[macro_export]` macros in `vendor/log/mod.rs` (e.g.
// `info_icon!`, `incoming_up!`) can refer to them via
// `$crate::info!` without each macro having to spell out
// `$crate::vendor::log::info!`. This matches the pre-vendor
// `farm-log` crate's behavior, where the same `pub use tracing::*`
// was at the crate's root.
pub use tracing::{debug, error, info, trace, warn};
pub mod vendor;
pub mod cache;
pub mod manifest;
pub mod work_hash;

// Re-export parser functionality from farm-parse
pub mod parser {
    pub use crate::vendor::parse::parse_plan;
    // Re-export parser types under different names to avoid conflicts
    pub use crate::vendor::parse::{Plan as ParsedPlan, Operation as ParsedOperation, Task as ParsedTask};
}