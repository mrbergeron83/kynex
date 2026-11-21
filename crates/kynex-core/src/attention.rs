//! Minimal background OS notifications and "next pane that needs you" cycling.
//!
//! In-UI status lamps handle attention while Kynex is in front; when Kynex is in the background
//! and notifications are enabled, an OS notification fires once per pane that newly needs you
//! or finishes. No sounds, no per-provider notification rules, no taskbar/Dock badge.

use crate::agent_panes::AgentState;

/// Of panes in a stable order, each with whether it wants you: the first one after `current`
/// that does, wrapping round and never `current` itself.
pub fn next_waiting<T: PartialEq + Copy>(order: &[(T, bool)], current: Option<T>) -> Option<T> {
    let at = current.and_then(|c| order.iter().position(|(k, _)| *k == c));
    let n = order.len();
    let start = at.map_or(0, |a| a + 1);
    (0..n).map(|i| &order[(start + i) % n]).find(|(k, waiting)| *waiting && Some(*k) != current).map(|(k, _)| *k)
}

/// Why an `agent_cli` pane triggers a background OS notification.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Alert {
    Finished,
    NeedsYou,
}

impl Alert {
    pub fn of(state: AgentState) -> Option<Self> {
        match state {
            AgentState::NeedsYou => Some(Self::NeedsYou),
            AgentState::Done => Some(Self::Finished),
            AgentState::Working | AgentState::Idle => None,
        }
    }

    pub fn body(self) -> &'static str {
        match self {
            Self::Finished => "Finished",
            Self::NeedsYou => "Needs your input",
        }
    }
}

/// Only a new reason triggers a notification.
pub fn news(was: Option<Alert>, now: Option<Alert>) -> Option<Alert> {
    now.filter(|n| was != Some(*n))
}

/// The process-wide app identity (Windows AppUserModelID, required for OS notifications).
pub const APP_ID: &str = if cfg!(debug_assertions) { "Kynex.Kynex.Dev" } else { "Kynex.Kynex" };

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_new_alerts_are_news() {
        assert_eq!(news(None, Some(Alert::Finished)), Some(Alert::Finished));
        assert_eq!(news(Some(Alert::Finished), Some(Alert::Finished)), None);
        assert_eq!(news(Some(Alert::Finished), Some(Alert::NeedsYou)), Some(Alert::NeedsYou));
        assert_eq!(news(Some(Alert::NeedsYou), None), None);
    }

    #[test]
    fn next_waiting_wraps_and_skips_current() {
        let order = [(1, false), (2, true), (3, false), (4, true)];
        assert_eq!(next_waiting(&order, Some(2)), Some(4));
        assert_eq!(next_waiting(&order, Some(4)), Some(2));
        assert_eq!(next_waiting(&order, Some(1)), Some(2));
        assert_eq!(next_waiting(&order, None), Some(2));
        assert_eq!(next_waiting(&[(1, true)], Some(1)), None);
    }
}
