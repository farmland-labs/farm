//! SPDX-License-Identifier: MIT OR Apache-2.0

use std::fmt;
use tracing::{Event, Subscriber};
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::{FmtContext, FormatEvent, FormatFields};
use tracing_subscriber::registry::LookupSpan;

use super::direction::Direction;

/// Custom formatter for farm-log that includes directional arrows
pub struct FarmLogFormatter {
    use_ansi: bool,
}

impl FarmLogFormatter {
    pub fn new(use_ansi: bool) -> Self {
        Self { use_ansi }
    }
}

impl<S, N> FormatEvent<S, N> for FarmLogFormatter
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        _ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        // Write timestamp (dimmed if ANSI is enabled)
        let now = chrono::Utc::now();
        let timestamp = format!("{}", now.format("%Y-%m-%dT%H:%M:%S%.6fZ"));
        if self.use_ansi {
            write!(writer, "\x1b[2m{}\x1b[0m ", timestamp)?; // Dim
        } else {
            write!(writer, "{} ", timestamp)?;
        }

        // Write level with padding and color
        let level = *event.metadata().level();
        if self.use_ansi {
            let level_str = format!("{:>5}", level);
            let colored_level = match level {
                tracing::Level::ERROR => format!("\x1b[31m{}\x1b[0m", level_str), // Red
                tracing::Level::WARN => format!("\x1b[33m{}\x1b[0m", level_str),  // Yellow
                tracing::Level::INFO => format!("\x1b[32m{}\x1b[0m", level_str),  // Green
                tracing::Level::DEBUG => format!("\x1b[34m{}\x1b[0m", level_str), // Blue
                tracing::Level::TRACE => format!("\x1b[35m{}\x1b[0m", level_str), // Magenta
            };
            write!(writer, "{} ", colored_level)?;
        } else {
            write!(writer, "{:>5} ", level)?;
        }

        // Write direction icon if present
        // Emojis are 2 columns wide in terminals, plus 1 space = 3 columns total
        let mut visitor = DirectionVisitor::default();
        event.record(&mut visitor);
        
        if let Some(icon) = visitor.get_icon() {
            write!(writer, "{} ", icon)?;
        } else {
            write!(writer, "   ")?;  // 3 spaces to match emoji width + space
        }

        // Write target (component name) with no space before (dimmed if ANSI is enabled)
        let target = event.metadata().target();
        if self.use_ansi {
            write!(writer, " \x1b[2m{}:\x1b[0m ", target)?; // Dim
        } else {
            write!(writer, " {}: ", target)?;
        }

        // Write the message and fields (but skip farm.direction unless TRACE level)
        let mut field_writer = FieldWriter {
            writer: writer.by_ref(),
            first: true,
            log_level: *event.metadata().level(),
            use_ansi: self.use_ansi,
        };
        event.record(&mut field_writer);

        writeln!(writer)
    }
}

/// Visitor to extract direction, custom icon, and trace ID from event fields
#[derive(Default)]
struct DirectionVisitor {
    direction: Option<Direction>,
    custom_icon: Option<String>,
    trace_id: Option<String>,
}

impl DirectionVisitor {
    /// Get the icon to display (custom icon takes priority over direction)
    fn get_icon(&self) -> Option<String> {
        if let Some(ref icon) = self.custom_icon {
            return Some(icon.clone());
        }
        self.direction.map(|d| d.icon().to_string())
    }
}

impl tracing::field::Visit for DirectionVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn fmt::Debug) {
        if field.name() == Direction::field_name() {
            // Parse direction from debug output
            let debug_str = format!("{:?}", value);
            self.direction = match debug_str.as_str() {
                "\"incoming_up\"" => Some(Direction::IncomingUp),
                "\"outgoing_up\"" => Some(Direction::OutgoingUp),
                "\"incoming_down\"" => Some(Direction::IncomingDown),
                "\"outgoing_down\"" => Some(Direction::OutgoingDown),
                "\"horizontal_out\"" => Some(Direction::HorizontalOut),
                "\"horizontal_in\"" => Some(Direction::HorizontalIn),
                "\"exec_start\"" => Some(Direction::ExecStart),
                "\"exec_end\"" => Some(Direction::ExecEnd),
                _ => None,
            };
        } else if field.name() == "farm.icon" {
            // Custom icon field - extract the icon string
            let debug_str = format!("{:?}", value);
            self.custom_icon = Some(debug_str.trim_matches('"').to_string());
        } else if field.name() == "trace_id" {
            // Extract trace ID
            let debug_str = format!("{:?}", value);
            // Remove quotes from debug output
            self.trace_id = Some(debug_str.trim_matches('"').to_string());
        }
    }
}

/// Custom field writer that skips farm.direction
struct FieldWriter<'a> {
    writer: Writer<'a>,
    first: bool,
    log_level: tracing::Level,
    use_ansi: bool,
}

impl<'a> tracing::field::Visit for FieldWriter<'a> {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn fmt::Debug) {
        // Skip internal fields unless we're at TRACE level
        if (field.name() == Direction::field_name() || field.name() == "farm.icon")
            && self.log_level != tracing::Level::TRACE
        {
            return;
        }

        // Handle the message field specially
        if field.name() == "message" {
            let _ = write!(self.writer, "{:?}", value);
            self.first = false;
        } else {
            // Other fields are appended as key=value with 3 spaces separator
            if self.first {
                let _ = write!(self.writer, "   ");
                self.first = false;
            } else {
                let _ = write!(self.writer, " ");
            }
            
            // Apply dimmed cyan color to metadata fields if ANSI is enabled
            if self.use_ansi {
                let _ = write!(self.writer, "\x1b[2;36m{}={:?}\x1b[0m", field.name(), value); // Dim + Cyan
            } else {
                let _ = write!(self.writer, "{}={:?}", field.name(), value);
            }
        }
    }
}
