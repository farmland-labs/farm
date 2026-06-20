//! SPDX-License-Identifier: MIT OR Apache-2.0

/// Direction of information flow in the system
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Data/events from upstream services (↘️)
    IncomingUp,
    /// Data/events to upstream services (↗️)
    OutgoingUp,
    /// Data/events from downstream services (↖️)
    IncomingDown,
    /// Data/events to downstream services (↙️)
    OutgoingDown,
    /// Data/events to peer services (➡️)
    HorizontalOut,
    /// Data/events from peer services (⬅️)
    HorizontalIn,
    /// Local execution begins (⤵️)
    ExecStart,
    /// Local execution completes (⤴️)
    ExecEnd,
    /// Internal state/logic (no icon)
    Internal,
}

impl Direction {
    /// Get the icon for this direction
    pub fn icon(&self) -> &'static str {
        if std::env::var("FARM_LOG_ICONS").map(|v| v == "false").unwrap_or(false) {
            return self.ascii_icon();
        }

        match self {
            Direction::IncomingUp => "↘️",
            Direction::OutgoingUp => "↗️",
            Direction::IncomingDown => "↖️",
            Direction::OutgoingDown => "↙️",
            Direction::HorizontalOut => "➡️",
            Direction::HorizontalIn => "⬅️",
            Direction::ExecStart => "⤵️",
            Direction::ExecEnd => "⤴️",
            Direction::Internal => " ",
        }
    }

    /// Get ASCII fallback icon
    pub fn ascii_icon(&self) -> &'static str {
        match self {
            Direction::IncomingUp => "\\",
            Direction::OutgoingUp => "/",
            Direction::IncomingDown => "/",
            Direction::OutgoingDown => "\\",
            Direction::HorizontalOut => ">",
            Direction::HorizontalIn => "<",
            Direction::ExecStart => "v",
            Direction::ExecEnd => "^",
            Direction::Internal => " ",
        }
    }

    /// Get the tracing field name for this direction
    pub const fn field_name() -> &'static str {
        "farm.direction"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_icons() {
        assert_eq!(Direction::IncomingUp.icon(), "↘️");
        assert_eq!(Direction::OutgoingUp.icon(), "↗️");
        assert_eq!(Direction::IncomingDown.icon(), "↖️");
        assert_eq!(Direction::OutgoingDown.icon(), "↙️");
        assert_eq!(Direction::HorizontalOut.icon(), "➡️");
        assert_eq!(Direction::HorizontalIn.icon(), "⬅️");
        assert_eq!(Direction::ExecStart.icon(), "⤵️");
        assert_eq!(Direction::ExecEnd.icon(), "⤴️");
        assert_eq!(Direction::Internal.icon(), " ");
    }

    #[test]
    fn test_ascii_fallback() {
        assert_eq!(Direction::IncomingUp.ascii_icon(), "\\");
        assert_eq!(Direction::OutgoingUp.ascii_icon(), "/");
        assert_eq!(Direction::HorizontalOut.ascii_icon(), ">");
        assert_eq!(Direction::HorizontalIn.ascii_icon(), "<");
    }
}
