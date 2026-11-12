//! The bar above the transcript: the project path, the timeline and Git panes' buttons, the
//! project's Claude settings, and buttons to open the project in other apps. The session's own
//! details are in the session header below it.

use std::path::PathBuf;

use crate::ui::px;
use gpui::{Context, div, prelude::*};
use kynex_core::open::{Opener, open_project};

use kynex_core::toasts::Toast;

use super::KynexApp;
use super::keys::{ToggleGit, ToggleSessions, ToggleTerminal, ToggleTimeline};

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
        let timeline = icon_button("toggle-timeline", Icon::Timeline, cx)
            .when(self.timeline_shown(), |b| b.bg(t.active))
            .tooltip(Tooltip::for_action("Timeline", ToggleTimeline, "jump to any message or step"))
            .on_click(cx.listener(|this, _, window, cx| this.toggle_timeline(&ToggleTimeline, window, cx)));
        let Some(path) = project else {
            // Chat is a tab in the title bar; here only the timeline.
            return bar.child(div().flex_1()).child(timeline);
        };
        // Where you are: the selected workspace's folder, for the path and open-in buttons.
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
            icon_button("toggle-git-pane", Icon::GitBranch, cx)
                .when(self.git_pane_shown(), |b| b.bg(t.active))
                .tooltip(Tooltip::for_action("Git", ToggleGit, "changes, commit, sync, history and branches"))
                .on_click(cx.listener(|this, _, window, cx| this.toggle_git(&ToggleGit, window, cx))),
        )
        .child(timeline)
        .child({
            // The project's Claude settings: permissions, mode, effort.
            let path = path.clone();
            icon_button("claude-settings", Icon::Shield, cx)
                .tooltip(Tooltip::with_meta("Claude settings", "permissions, mode and effort for this project"))
                .on_click(cx.listener(move |this, _, _, cx| this.open_settings(path.clone(), cx)))
        })
        .child({
            // The project's subagents: which it uses and its own rule for them.
            let path = path.clone();
            icon_button("project-agents", Icon::Bot, cx)
                .tooltip(Tooltip::with_meta("Agents", "the subagents this project hands work to"))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.open_project_settings_on(path.clone(), super::settings::AGENTS_SECTION, cx)
                }))
        })
        .child(
            icon_button("open-scratch", Icon::Files, cx)
                .tooltip(Tooltip::with_meta("Scratch files", "Claude's temporary files, for every session here"))
                .on_click(cx.listener(|this, _, window, cx| this.open_project_scratch(window, cx))),
        )
        .child(open("open-code", Icon::Code, "Open in VS Code", Opener::Code, here.clone(), cx))
        .child(open("open-files", Icon::Folder, "Open in file manager", Opener::Explorer, here.clone(), cx))
        .child(
            icon_button("open-term", Icon::Terminal, cx)
                .when(self.has_terminal(), |b| b.bg(t.active))
                .tooltip(Tooltip::for_action("Terminal", ToggleTerminal, "a pane in Kynex, under the conversation"))
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
