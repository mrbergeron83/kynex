//! The workspace toolbar: workspace path, Sessions, Agents, Git, Terminal pane toggles,
//! and buttons to open the workspace in external apps.

use std::path::PathBuf;

use crate::ui::px;
use gpui::{Context, div, prelude::*};
use kynex_core::open::{Opener, open_project};
use kynex_core::toasts::Toast;

use super::KynexApp;
use super::keys::{ToggleAgents, ToggleGit, ToggleSessions, ToggleTerminal};
use crate::theme::theme;
use crate::ui::{Icon, Tooltip, icon_button};

impl KynexApp {
    pub(super) fn render_toolbar(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = theme(cx).clone();
        let project = self.active_project().map(|p| p.path.clone());
        let collapsed = self.sessions_hidden();
        let bar = div()
            .flex()
            .flex_none()
            .items_center()
            .gap_1()
            .h(px(36.))
            .px_2()
            .border_b_1()
            .border_color(t.border_variant)
            .when(collapsed, |d| {
                d.child(
                    icon_button("show-sessions", Icon::PanelLeft, cx)
                        .tooltip(Tooltip::for_action("Show sessions", ToggleSessions, ""))
                        .on_click(cx.listener(|this, _, window, cx| this.toggle_sessions(&ToggleSessions, window, cx))),
                )
            });
        let Some(path) = project else {
            return bar;
        };
        let here = self.workspace_cwd().unwrap_or_else(|| path.clone());
        let open = |id: &'static str,
                    which: Icon,
                    tip: &'static str,
                    opener: Opener,
                    path: PathBuf,
                    cx: &mut Context<Self>| {
            icon_button(id, which, cx).tooltip(Tooltip::text(tip)).on_click(cx.listener(move |this, _, _, cx| {
                this.open_in(opener, &path);
                cx.notify();
            }))
        };
        bar.child(
            div()
                .flex_1()
                .min_w_0()
                .px_2()
                .truncate()
                .text_xs()
                .text_color(t.text_placeholder)
                .child(here.display().to_string()),
        )
        .child(
            icon_button("toggle-agents-pane", Icon::Bot, cx)
                .when(self.agents_pane_open(), |b| b.bg(t.active))
                .tooltip(Tooltip::for_action("Agents", ToggleAgents, "running agents across all workspaces"))
                .on_click(cx.listener(|this, _, window, cx| this.toggle_agents(&ToggleAgents, window, cx))),
        )
        .child(
            icon_button("toggle-git-pane", Icon::GitBranch, cx)
                .when(self.git_pane_shown(), |b| b.bg(t.active))
                .tooltip(Tooltip::for_action("Git", ToggleGit, "changes, commit, sync, history and branches"))
                .on_click(cx.listener(|this, _, window, cx| this.toggle_git(&ToggleGit, window, cx))),
        )
        .child(open("open-code", Icon::Code, "Open in VS Code", Opener::Code, here.clone(), cx))
        .child(open("open-files", Icon::Folder, "Open in file manager", Opener::Explorer, here.clone(), cx))
        .child(
            icon_button("open-term", Icon::Terminal, cx)
                .when(self.has_terminal(), |b| b.bg(t.active))
                .tooltip(Tooltip::for_action("Terminal", ToggleTerminal, "a shell pane in Kynex"))
                .on_click(cx.listener(|this, _, window, cx| this.terminal_button(window, cx))),
        )
    }

    /// Opens a project folder in another app; a failure shows as a notice.
    pub(super) fn open_in(&mut self, opener: Opener, path: &std::path::Path) {
        if let Err(e) = open_project(opener, path) {
            self.toast(Toast::error(e));
        }
    }
}
