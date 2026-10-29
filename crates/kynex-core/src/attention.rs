//! Telling you a session wants you while Kynex is in the background: an OS
//! notification saying which session and why (the app posts it; opt-in), and
//! a "needs you" badge on the app's icon: an amber dot over the taskbar button
//! on Windows, a Dock badge on macOS. Linux has no common badge; nothing there.

use crate::session::Session;

/// Why a session wants you.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    /// A turn ended and you haven't looked at it.
    Finished,
    /// A permission prompt or a question waits for your answer.
    Answer,
    /// Claude is waiting for you to do something in the browser.
    Browser,
}

impl Why {
    /// What a session wants of you, if anything.
    pub fn of(s: &Session) -> Option<Self> {
        if s.waiting() {
            Some(Self::Answer)
        } else if !s.busy && crate::mcp::handoff(&s.items).is_some() {
            Some(Self::Browser)
        } else if s.finished {
            Some(Self::Finished)
        } else {
            None
        }
    }

    /// A notification's body: what to do about it.
    pub fn body(self) -> &'static str {
        match self {
            Self::Finished => "Finished",
            Self::Answer => "Needs your answer",
            Self::Browser => "Waiting for you in the browser",
        }
    }
}

/// What to notify about, going from `was` to `now`: only a new reason, once.
/// Back to nothing, or the same reason as before, says nothing.
pub fn news(was: Option<Why>, now: Option<Why>) -> Option<Why> {
    now.filter(|n| was != Some(*n))
}

/// The process-wide app identity: the Windows AppUserModelID, which the OS
/// groups taskbar buttons and notifications by. A dev build has its own, so
/// it doesn't stack with the installed one.
pub const APP_ID: &str = if cfg!(debug_assertions) { "Kynex.Kynex.Dev" } else { "Kynex.Kynex" };

/// Shows or clears the "needs you" badge on the app's icon. `window` is the
/// main window's native handle (an `HWND` on Windows; unused elsewhere).
/// Blocks briefly on the OS; call it when the state changes, not every frame.
pub fn set_badge(window: isize, on: bool) {
    #[cfg(windows)]
    crate::win_shell::set_overlay(window, on);
    #[cfg(target_os = "macos")]
    {
        let _ = window;
        mac::set_dock_badge(on);
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    let _ = (window, on);
}

#[cfg(target_os = "macos")]
mod mac {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;
    use objc2_foundation::NSString;

    /// A dot on the Dock icon, or none. Only from the main thread.
    pub fn set_dock_badge(on: bool) {
        let Some(mtm) = MainThreadMarker::new() else { return };
        let tile = NSApplication::sharedApplication(mtm).dockTile();
        let label = on.then(|| NSString::from_str("●"));
        tile.setBadgeLabel(label.as_deref());
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde_json::json;

    use super::*;
    use crate::session::{Item, Kind};

    #[test]
    fn says_why_a_session_wants_you() {
        let mut s = Session::new(Kind::Chat, PathBuf::new());
        assert_eq!(Why::of(&s), None);
        s.finished = true;
        assert_eq!(Why::of(&s), Some(Why::Finished));
        // A prompt outranks a finished turn.
        s.handle(json!({ "type": "control_request", "request_id": "r", "request": { "subtype": "can_use_tool", "tool_name": "Bash" } }));
        assert_eq!(Why::of(&s), Some(Why::Answer));
        assert!(matches!(s.items.last(), Some(Item::Permission { .. })));
    }

    #[test]
    fn only_a_new_reason_is_news() {
        use Why::*;
        assert_eq!(news(None, Some(Finished)), Some(Finished));
        assert_eq!(news(Some(Finished), Some(Finished)), None);
        assert_eq!(news(Some(Finished), Some(Answer)), Some(Answer));
        assert_eq!(news(Some(Answer), None), None);
        assert_eq!(Answer.body(), "Needs your answer");
    }
}
