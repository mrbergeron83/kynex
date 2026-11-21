//! Background OS notifications for `agent_cli` panes (`kynex_core::attention`).
//!
//! While Kynex is in the foreground, in-UI status lamps show every agent's state.
//! When Kynex is in the background and `background_notifications` is enabled, a
//! simple OS notification fires once per pane that newly finishes or needs input.
//! Clicking the notification focuses Kynex on that pane.

use gpui::{Context, SystemNotification, Window};
use kynex_core::agent_panes::AgentState;
use kynex_core::attention::{self, Alert};

use super::KynexApp;
use super::agent_cli::AgentId;

fn tag(id: AgentId) -> String {
    format!("agent-{id}")
}

impl KynexApp {
    /// Clicking an OS notification brings Kynex up and focuses (or swaps in) the agent.
    pub(super) fn watch_notification_clicks(window: &Window, cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        let handle = window.window_handle();
        cx.on_system_notification_response(move |response, cx| {
            let Some(id) = response
                .tag
                .strip_prefix("agent-")
                .or_else(|| response.tag.strip_prefix("pane-"))
                .and_then(|p| p.parse::<AgentId>().ok())
            else {
                return;
            };
            handle
                .update(cx, |_, window, cx| {
                    window.activate_window();
                    this.update(cx, |this, cx| {
                        this.go_to_agent(id, window, cx);
                    })
                    .ok();
                })
                .ok();
        });
    }

    /// Dismisses any pending OS notification for live instance `id`.
    pub(super) fn clear_agent_alert(&mut self, id: AgentId, cx: &Context<Self>) {
        if self.wants.remove(&id).is_some() {
            cx.dismiss_system_notification(&tag(id));
        }
    }

    /// Updates the alert state for live instance `id`. Fires a background OS
    /// notification when `background_notifications` is enabled and Kynex is not active;
    /// dismisses any previous notification when the alert clears.
    pub(super) fn sync_agent_alert(&mut self, id: AgentId, name: &str, state: AgentState, cx: &mut Context<Self>) {
        let prev = self.wants.get(&id).copied();
        let now = Alert::of(state);
        if let Some(alert) = attention::news(prev, now)
            && self.saved.background_notifications
            && !self.window_active
        {
            cx.show_system_notification(SystemNotification {
                tag: tag(id).into(),
                title: name.to_string().into(),
                body: alert.body().into(),
                actions: Vec::new(),
            });
        }
        match now {
            Some(alert) => {
                self.wants.insert(id, alert);
            }
            None => {
                self.clear_agent_alert(id, cx);
            }
        }
    }
}
