//! Telling you a session wants you while Kynex is in the background
//! (`kynex_core::attention`): an OS notification per session that newly
//! finishes or needs an answer, when that's turned on, and a dot on the app's
//! taskbar / Dock icon while any session wants you. Clicking a notification
//! brings Kynex up on that session.

use gpui::{Context, SystemNotification, Window};
use kynex_core::attention::{self, Why};
use kynex_core::live::Key;

use super::KynexApp;

/// A session's notification tag, which a click hands back.
fn tag(key: Key) -> String {
    format!("session-{key}")
}

impl KynexApp {
    /// Clicking a notification shows its session.
    pub(super) fn watch_notification_clicks(window: &Window, cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        let handle = window.window_handle();
        cx.on_system_notification_response(move |response, cx| {
            let Some(key) = response.tag.strip_prefix("session-").and_then(|k| k.parse::<Key>().ok()) else {
                return;
            };
            handle
                .update(cx, |_, window, cx| {
                    window.activate_window();
                    this.update(cx, |this, cx| {
                        this.show_from_background(cx);
                        this.go_to_session(key, window, cx)
                    })
                    .ok();
                })
                .ok();
        });
    }

    /// After sessions change: notifies about each session that newly wants
    /// you while Kynex isn't in front (if notifications are on), takes back
    /// those you've seen to, and sets the icon's dot. Only calls the OS on a change.
    pub(super) fn sync_attention(&mut self, cx: &mut Context<Self>) {
        let now: Vec<(Key, Why)> = self
            .chat
            .sessions
            .iter()
            .chain(self.code.values().flat_map(|l| l.sessions.iter()))
            .filter_map(|s| Some((s.uid, Why::of(s)?)))
            .collect();
        if !self.window_active && self.saved.notify {
            for &(key, why) in &now {
                if attention::news(self.wants.get(&key).copied(), Some(why)).is_some() {
                    let title = self.session_title(key).unwrap_or_else(|| "Kynex".into());
                    cx.show_system_notification(SystemNotification {
                        tag: tag(key).into(),
                        title: title.into(),
                        body: why.body().into(),
                        actions: Vec::new(),
                    });
                }
            }
        }
        for key in self.wants.keys().filter(|k| !now.iter().any(|(n, _)| n == *k)) {
            cx.dismiss_system_notification(&tag(*key));
        }
        self.wants = now.into_iter().collect();
        let badge = !self.wants.is_empty();
        if badge != self.badged {
            self.badged = badge;
            attention::set_badge(self.native_window, badge);
        }
    }
}

/// The main window's native handle, for the taskbar badge: its `HWND` on Windows.
pub(super) fn native_handle(window: &Window) -> isize {
    #[cfg(windows)]
    {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        match HasWindowHandle::window_handle(window).map(|h| h.as_raw()) {
            Ok(RawWindowHandle::Win32(h)) => h.hwnd.get(),
            _ => 0,
        }
    }
    #[cfg(not(windows))]
    {
        let _ = window;
        0
    }
}
