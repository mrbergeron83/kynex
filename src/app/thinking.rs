//! The composer's thinking switch (a project setting, live on its sessions) and
//! bypass permissions: its setting, and its place in the Shift+Tab cycle.

use std::path::PathBuf;

use gpui::{AnyElement, Context, Window, div, prelude::*};
use kynex_core::project::{self, Mode, Permissions};
use kynex_core::toasts::Toast;

use super::KynexApp;
use super::app_settings::choice;
use super::keys::ToggleThinking;
use super::settings::heading;
use crate::theme::{Palette, theme};
use crate::ui::{Tooltip, ghost_button};

impl KynexApp {
    /// Alt+T: thinking on or off in the composer's project.
    pub(super) fn toggle_thinking(&mut self, _: &ToggleThinking, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(path) = self.active_project().map(|p| p.path.clone()) {
            let off = self.claude_settings(&self.settings_dir(&path)).thinking_off;
            self.set_thinking(path, off, cx);
        }
    }

    /// Turns thinking on or off for a project's sessions, the running ones live.
    fn set_thinking(&mut self, path: PathBuf, on: bool, cx: &mut Context<Self>) {
        let path = self.settings_dir(&path);
        let mut claude = self.claude_settings(&path);
        if claude.thinking_off == !on {
            return;
        }
        claude.thinking_off = !on;
        if let Err(e) = claude.save(&path) {
            self.toast(Toast::error(e));
        }
        self.claude_settings.insert(path.clone(), claude.clone());
        self.apply_project_settings(&path, &Permissions::load(&path), &claude);
        cx.notify();
    }

    /// The composer's thinking chip, beside effort: on or off, click to switch.
    pub(super) fn render_thinking_chip(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let path = self.active_project()?.path.clone();
        let t = theme(cx).clone();
        let on = !self.claude_settings(&self.settings_dir(&path)).thinking_off;
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
                .on_click(cx.listener(move |this, _, _, cx| this.set_thinking(path.clone(), !on, cx)))
                .into_any_element(),
        )
    }

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
            for s in self.all_sessions_mut().filter(|s| s.mode == Mode::Bypass) {
                let default = project::ClaudeSettings::load(s.settings_folder()).mode;
                s.set_mode(if default == Mode::Bypass { Mode::Manual } else { default });
            }
        }
        cx.notify();
    }
}
