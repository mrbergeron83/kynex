//! The Claude panes' app-wide settings: Settings → General → Bypass permissions,
//! and Settings → Appearance → Transcript width (`kynex_core::claude::column`).

use gpui::{Context, div, prelude::*};
use kynex_core::claude::column::ColumnWidth;
use kynex_core::project;

use super::KynexApp;
use super::app_settings::{choice, heading};
use crate::theme::Palette;

impl KynexApp {
    /// Settings → General: whether bypass permissions is in the mode cycle.
    pub(super) fn render_bypass_setting(&self, t: &Palette, cx: &mut Context<Self>) -> gpui::Div {
        let allowed = self.saved.allow_bypass;
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(heading(
                "Bypass permissions",
                "A mode where Claude runs every tool without asking, as `claude --dangerously-skip-permissions`. \
                 Only for a sandbox or a throwaway folder. Sessions started from now on can switch to it.",
                t,
            ))
            .children([(false, "Off"), (true, "In the mode list and Shift+Tab")].into_iter().map(|(on, label)| {
                choice(("allow-bypass", usize::from(on)), allowed == on, label.into(), t)
                    .on_click(cx.listener(move |this, _, _, cx| this.set_allow_bypass(on, cx)))
            }))
    }

    /// Turns bypass mode on or off; sessions in it go back to their project's mode.
    fn set_allow_bypass(&mut self, on: bool, cx: &mut Context<Self>) {
        self.saved.allow_bypass = on;
        self.save();
        project::set_bypass_allowed(on);
        if !on {
            for chat in self.claude_chats.values() {
                chat.view.0.update(cx, |v, cx| v.leave_bypass(cx));
            }
        }
        cx.notify();
    }

    /// Settings → Appearance: how wide the conversation and the message box are.
    pub(super) fn render_transcript_width(&self, t: &Palette, cx: &mut Context<Self>) -> gpui::Div {
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(heading("Transcript width", "How wide the conversation and the message box are.", t))
            .children(ColumnWidth::ALL.into_iter().enumerate().map(|(n, w)| {
                choice(("app-column", n), self.saved.column == w, format!("{} · {}", w.label(), w.help()), t)
                    .on_click(cx.listener(move |this, _, _, cx| this.set_column(w, cx)))
            }))
    }

    fn set_column(&mut self, w: ColumnWidth, cx: &mut Context<Self>) {
        self.saved.column = w;
        self.save();
        for chat in self.claude_chats.values() {
            chat.view.0.update(cx, |v, cx| v.set_column(w, cx));
        }
        cx.notify();
    }
}
