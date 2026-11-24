//! The composer's thinking switch and effort: project settings (Claude settings),
//! live on the project's sessions.

use gpui::{AnyElement, Context, prelude::*};
use kynex_core::toasts::Toast;

use super::ClaudePane;
use super::keys::ToggleThinking;
use crate::claude_chat::ClaudeChatEvent;
use crate::theme::theme;
use crate::ui::{Tooltip, ghost_button};

impl ClaudePane {
    /// Alt+T: thinking on or off in the pane's project.
    pub(super) fn toggle_thinking(&mut self, cx: &mut Context<Self>) {
        let off = self.claude.thinking_off;
        self.set_thinking(off, cx);
    }

    /// Turns thinking on or off for the project's sessions, the running ones live.
    fn set_thinking(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.claude.thinking_off == !on {
            return;
        }
        self.claude.thinking_off = !on;
        self.save_claude_settings(cx);
    }

    /// Sets the project's effort, from the composer's picker or `/effort`.
    pub(super) fn set_effort(&mut self, id: String, cx: &mut Context<Self>) {
        if self.claude.effort != id {
            self.claude.effort = id;
            self.save_claude_settings(cx);
        }
    }

    /// Saves the project's Claude settings and brings its sessions up to them.
    fn save_claude_settings(&mut self, cx: &mut Context<Self>) {
        let dir = self.session.settings_folder().to_path_buf();
        if let Err(e) = self.claude.save(&dir) {
            self.toast(Toast::error(e), cx);
        }
        self.apply_project_settings();
        cx.emit(ClaudeChatEvent::ProjectSettingsChanged(dir));
        cx.notify();
    }

    /// The composer's thinking chip, beside effort: on or off, click to switch.
    pub(super) fn render_thinking_chip(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let t = theme(cx).clone();
        let on = !self.claude.thinking_off;
        let tip = if on {
            "Thinking on: Claude reasons before it answers. Click to turn it off (some models, like Opus 5.5, always think)"
        } else {
            "Thinking off: Claude answers straight away. Click to turn it on"
        };
        Some(
            ghost_button("thinking", cx)
                .text_color(if on { t.text_muted } else { t.text_placeholder })
                .child(if on { "Thinking" } else { "No thinking" })
                .tooltip(Tooltip::for_action(tip, ToggleThinking, "on / off"))
                .on_click(cx.listener(move |this, _, _, cx| this.set_thinking(!on, cx)))
                .into_any_element(),
        )
    }
}
