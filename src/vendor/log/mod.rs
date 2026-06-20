//! SPDX-License-Identifier: MIT OR Apache-2.0

//! # farm-log
//!
//! Directional logging library for Farmland distributed build system.
//!
//! Provides consistent logging across all Farmland binaries with visual
//! indicators for information flow direction.
//!
//! ## Example
//!
//! ```rust
//! use farm::vendor::log::{info, incoming_up, outgoing_down};
//!
//! // Initialize logging
//! farm::vendor::log::init();
//!
//! // Basic logging
//! info!("Server started");
//!
//! // Directional logging with manual trace_id
//! incoming_up!(trace_id = "abc123", "Received message: {}", "farm.build.request");
//! outgoing_down!(trace_id = "abc123", "Sending to plugin: {}", "payload");
//!
//! // Directional logging with CloudEvent (automatically extracts trace_id)
//! // incoming_up!(event = &cloud_event, "Received build request");
//! // outgoing_up!(event = &response_event, "Sending response");
//! ```

/// Crate version from Cargo.toml
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub mod direction;
pub mod formatter;

use formatter::FarmLogFormatter;
use tracing_subscriber::fmt;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

/// Initialize the farm logging system with RUST_LOG env var (defaults to "off")
pub fn init() {
    init_with_default("off")
}

/// Initialize the farm logging system with a custom default filter
/// 
/// The `default_filter` is used when RUST_LOG is not set.
/// If RUST_LOG is set, it completely overrides the default.
/// 
/// # Example
/// ```rust
/// // Initialize with service-specific defaults
/// farm::vendor::log::init_with_default("farm=info,warn");
/// ```
pub fn init_with_default(default_filter: &str) {
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(default_filter));
    let use_ansi = should_use_ansi();

    tracing_subscriber::registry()
        .with(env_filter)
        .with(
            fmt::layer()
                .event_format(FarmLogFormatter::new(use_ansi))
                .with_ansi(use_ansi)
                .with_writer(std::io::stderr),
        )
        .init();
}

/// Check if ANSI colors should be used
fn should_use_ansi() -> bool {
    // Disable colors if NO_COLOR is set
    if std::env::var("NO_COLOR").is_ok() {
        return false;
    }

    // Check FARM_LOG_COLORS setting
    match std::env::var("FARM_LOG_COLORS").as_deref() {
        Ok("never") => false,
        Ok("always") => true,
        _ => std::io::IsTerminal::is_terminal(&std::io::stderr()),
    }
}

// Re-export tracing macros for basic logging
pub use tracing::{debug, error, info, trace, warn};

// Re-export this module's own `#[macro_export]` macros (defined
// below) at the `crate::vendor::log::*` namespace too. Without these
// re-exports the macros are reachable only via the crate root
// (e.g. `crate::info_icon!`), which is inconvenient inside the
// vendored layout: most callers expect `crate::vendor::log::info_icon`
// to mirror the pre-vendor `farm::vendor::log::info_icon` shape.
pub use crate::{
    debug_icon, error_icon, exec_end, exec_start, horizontal_in, horizontal_out, incoming_down,
    incoming_up, info_icon, outgoing_down, outgoing_up, trace_icon, warn_icon,
};

/// Patterns to detect secret environment variables
const SECRET_ENV_PATTERNS: &[&str] = &[
    "PASSWORD", "SECRET", "TOKEN", "KEY", "PRIVATE",
    "API_KEY", "AUTH", "CREDENTIALS", "CERT", "PASSPHRASE",
];

/// Check if an environment variable name suggests it contains a secret
pub fn is_secret_var(name: &str) -> bool {
    let upper = name.to_uppercase();
    SECRET_ENV_PATTERNS.iter().any(|pattern| upper.contains(pattern))
}

/// Redact a value if the variable name suggests it's a secret
/// Shows first4...last4 for debugging while hiding the actual secret
pub fn redact_value(name: &str, value: &str) -> String {
    if is_secret_var(name) {
        if value.len() > 8 {
            format!("{}...{}", &value[..4], &value[value.len()-4..])
        } else if value.is_empty() {
            "***EMPTY***".to_string()
        } else {
            "***REDACTED***".to_string()
        }
    } else {
        value.to_string()
    }
}

/// Log incoming message from upstream service (↘️)
#[macro_export]
macro_rules! incoming_up {
    (event = $event:expr, $($arg:tt)+) => {
        {
            let trace_id = $event.traceparent().unwrap_or("unknown");
            $crate::info!(farm.direction = "incoming_up", trace_id = trace_id, $($arg)+)
        }
    };
    (trace_id = $trace_id:expr, $($arg:tt)+) => {
        $crate::info!(farm.direction = "incoming_up", trace_id = $trace_id, $($arg)+)
    };
    ($($arg:tt)+) => {
        $crate::info!(farm.direction = "incoming_up", $($arg)+)
    };
}

/// Log outgoing message to upstream service (↗️)
#[macro_export]
macro_rules! outgoing_up {
    (event = $event:expr, $($arg:tt)+) => {
        {
            let trace_id = $event.traceparent().unwrap_or("unknown");
            $crate::info!(farm.direction = "outgoing_up", trace_id = trace_id, $($arg)+)
        }
    };
    (trace_id = $trace_id:expr, $($arg:tt)+) => {
        $crate::info!(farm.direction = "outgoing_up", trace_id = $trace_id, $($arg)+)
    };
    ($($arg:tt)+) => {
        $crate::info!(farm.direction = "outgoing_up", $($arg)+)
    };
}

/// Log incoming message from downstream service (↖️)
#[macro_export]
macro_rules! incoming_down {
    (event = $event:expr, $($arg:tt)+) => {
        {
            let trace_id = $event.traceparent().unwrap_or("unknown");
            $crate::info!(farm.direction = "incoming_down", trace_id = trace_id, $($arg)+)
        }
    };
    (trace_id = $trace_id:expr, $($arg:tt)+) => {
        $crate::info!(farm.direction = "incoming_down", trace_id = $trace_id, $($arg)+)
    };
    ($($arg:tt)+) => {
        $crate::info!(farm.direction = "incoming_down", $($arg)+)
    };
}

/// Log outgoing message to downstream service (↙️)
#[macro_export]
macro_rules! outgoing_down {
    (event = $event:expr, $($arg:tt)+) => {
        {
            let trace_id = $event.traceparent().unwrap_or("unknown");
            $crate::info!(farm.direction = "outgoing_down", trace_id = trace_id, $($arg)+)
        }
    };
    (trace_id = $trace_id:expr, $($arg:tt)+) => {
        $crate::info!(farm.direction = "outgoing_down", trace_id = $trace_id, $($arg)+)
    };
    ($($arg:tt)+) => {
        $crate::info!(farm.direction = "outgoing_down", $($arg)+)
    };
}

/// Log horizontal outgoing message (➡️)
#[macro_export]
macro_rules! horizontal_out {
    (event = $event:expr, $($arg:tt)+) => {
        {
            let trace_id = $event.traceparent().unwrap_or("unknown");
            $crate::info!(farm.direction = "horizontal_out", trace_id = trace_id, $($arg)+)
        }
    };
    (trace_id = $trace_id:expr, $($arg:tt)+) => {
        $crate::info!(farm.direction = "horizontal_out", trace_id = $trace_id, $($arg)+)
    };
    ($($arg:tt)+) => {
        $crate::info!(farm.direction = "horizontal_out", $($arg)+)
    };
}

/// Log horizontal incoming message (⬅️)
#[macro_export]
macro_rules! horizontal_in {
    (event = $event:expr, $($arg:tt)+) => {
        {
            let trace_id = $event.traceparent().unwrap_or("unknown");
            $crate::info!(farm.direction = "horizontal_in", trace_id = trace_id, $($arg)+)
        }
    };
    (trace_id = $trace_id:expr, $($arg:tt)+) => {
        $crate::info!(farm.direction = "horizontal_in", trace_id = $trace_id, $($arg)+)
    };
    ($($arg:tt)+) => {
        $crate::info!(farm.direction = "horizontal_in", $($arg)+)
    };
}

/// Log execution start (⤵️)
#[macro_export]
macro_rules! exec_start {
    (event = $event:expr, $($arg:tt)+) => {
        {
            let trace_id = $event.traceparent().unwrap_or("unknown");
            $crate::info!(farm.direction = "exec_start", trace_id = trace_id, $($arg)+)
        }
    };
    (trace_id = $trace_id:expr, $($arg:tt)+) => {
        $crate::info!(farm.direction = "exec_start", trace_id = $trace_id, $($arg)+)
    };
    ($($arg:tt)+) => {
        $crate::info!(farm.direction = "exec_start", $($arg)+)
    };
}

/// Log execution end (⤴️)
#[macro_export]
macro_rules! exec_end {
    (event = $event:expr, $($arg:tt)+) => {
        {
            let trace_id = $event.traceparent().unwrap_or("unknown");
            $crate::info!(farm.direction = "exec_end", trace_id = trace_id, $($arg)+)
        }
    };
    (trace_id = $trace_id:expr, $($arg:tt)+) => {
        $crate::info!(farm.direction = "exec_end", trace_id = $trace_id, $($arg)+)
    };
    ($($arg:tt)+) => {
        $crate::info!(farm.direction = "exec_end", $($arg)+)
    };
}

// ============================================================================
// Custom Icon Macros
// ============================================================================
// These macros allow specifying a custom emoji icon that appears in the
// direction column. Usage:
//   info_icon!("🌾", "Connecting to Farmland");
//   warn_icon!("⚠️", url = %url, "Connection slow");

/// Log at INFO level with a custom icon
/// 
/// # Examples
/// ```ignore
/// info_icon!("🌾", "Connecting to cloud");
/// info_icon!("🔑", tenant = %tenant, "Got credentials");
/// ```
#[macro_export]
macro_rules! info_icon {
    ($icon:expr, $($arg:tt)+) => {
        $crate::info!(farm.icon = $icon, $($arg)+)
    };
}

/// Log at WARN level with a custom icon
#[macro_export]
macro_rules! warn_icon {
    ($icon:expr, $($arg:tt)+) => {
        $crate::warn!(farm.icon = $icon, $($arg)+)
    };
}

/// Log at ERROR level with a custom icon
#[macro_export]
macro_rules! error_icon {
    ($icon:expr, $($arg:tt)+) => {
        $crate::error!(farm.icon = $icon, $($arg)+)
    };
}

/// Log at DEBUG level with a custom icon
#[macro_export]
macro_rules! debug_icon {
    ($icon:expr, $($arg:tt)+) => {
        $crate::debug!(farm.icon = $icon, $($arg)+)
    };
}

/// Log at TRACE level with a custom icon
#[macro_export]
macro_rules! trace_icon {
    ($icon:expr, $($arg:tt)+) => {
        $crate::trace!(farm.icon = $icon, $($arg)+)
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_init() {
        // Inside the vendored test binary other tests share the
        // process; a sibling may have installed the global tracing
        // subscriber already (`SetLoggerError`). Swallowing the
        // panic keeps the test's original "init doesn't blow up
        // catastrophically" assertion without flaking on test
        // ordering. The original farm-log crate had its own test
        // binary where this couldn't happen.
        let _ = std::panic::catch_unwind(init);
    }
    
    #[test]
    fn test_is_secret_var() {
        // Should detect common secret patterns
        assert!(is_secret_var("AWS_SECRET_ACCESS_KEY"));
        assert!(is_secret_var("DATABASE_PASSWORD"));
        assert!(is_secret_var("API_TOKEN"));
        assert!(is_secret_var("PRIVATE_KEY"));
        assert!(is_secret_var("AUTH_TOKEN"));
        assert!(is_secret_var("MY_SECRET"));
        assert!(is_secret_var("SSL_CERT_KEY"));
        
        // Should not flag regular vars
        assert!(!is_secret_var("AWS_REGION"));
        assert!(!is_secret_var("DATABASE_HOST"));
        assert!(!is_secret_var("API_ENDPOINT"));
        assert!(!is_secret_var("LOG_LEVEL"));
        assert!(!is_secret_var("FARM_TARGET"));
    }
    
    #[test]
    fn test_redact_value() {
        // Long values show first4...last4
        assert_eq!(
            redact_value("AWS_SECRET_ACCESS_KEY", "abcdefghijklmnop"),
            "abcd...mnop"
        );
        
        // Short secrets fully redacted
        assert_eq!(
            redact_value("API_TOKEN", "secret"),
            "***REDACTED***"
        );
        
        // Empty secrets
        assert_eq!(
            redact_value("PASSWORD", ""),
            "***EMPTY***"
        );
        
        // Non-secret values passed through
        assert_eq!(
            redact_value("AWS_REGION", "us-east-1"),
            "us-east-1"
        );
        
        // Exact 8 chars shows first4...last4
        assert_eq!(
            redact_value("TOKEN", "12345678"),
            "***REDACTED***"
        );
        
        // 9 chars shows redaction
        assert_eq!(
            redact_value("SECRET_KEY", "123456789"),
            "1234...6789"
        );
    }
}
