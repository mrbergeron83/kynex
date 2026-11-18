//! `knx notify`: a toast in the window, or an OS notification while Kynex is in the background.
//! A click goes to the session or pane it names.

use std::sync::atomic::{AtomicU64, Ordering};

use gpui::{Context, SystemNotification};
use kynex_core::control::protocol::SessionRef;
use kynex_core::toasts::{Action, Toast};
use serde_json::{Value, json};

use super::KynexApp;
use super::attention::tag;

/// Numbers the notifications that go nowhere, so each has its own tag.
static LOOSE: AtomicU64 = AtomicU64::new(0);

impl KynexApp {
    /// Shows `title` and `body`: as an OS notification when the window isn't in front and
    /// notifications are on, else as a toast.
    pub(super) fn control_notify(
        &mut self,
        title: &str,
        body: Option<String>,
        pane: Option<u64>,
        session: Option<&SessionRef>,
        cx: &mut Context<Self>,
    ) -> Result<Value, String> {
        if title.trim().is_empty() {
            return Err("a notification needs a title".into());
        }
        // One place for a click to go.
        if pane.is_some() && session.is_some() {
            return Err("give --pane or --session, not both".into());
        }
        let key = session.map(|s| self.resolve_session(s)).transpose()?;
        if let Some(pane) = pane
            && !self.tiles_have(pane)
        {
            return Err(format!("no pane {pane} in the shown workspace"));
        }
        let body = body.unwrap_or_default();
        if !self.window_active && self.saved.notifications.any_on() {
            let tag = match (key, pane) {
                (Some(key), _) => tag(key),
                (None, Some(pane)) => format!("pane-{pane}"),
                (None, None) => format!("knx-notify-{}", LOOSE.fetch_add(1, Ordering::Relaxed)),
            };
            cx.show_system_notification(SystemNotification {
                tag: tag.into(),
                title: title.to_string().into(),
                body: body.into(),
                actions: Vec::new(),
            });
            return Ok(json!({ "shown": "os" }));
        }
        let mut toast = Toast::info(title).detail(body);
        if let Some(key) = key {
            // Not tied to the session with `.session`: looking at it would dismiss the toast at once.
            toast = toast.action(Action::GoToSession(key));
        } else if let Some(pane) = pane {
            toast = toast.action(Action::GoToPane(pane));
        }
        self.toast(toast);
        cx.notify();
        Ok(json!({ "shown": "toast" }))
    }
}
