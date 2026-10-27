//! The bar above the transcript: Code / Git switch, the project path, and
//! buttons to open the project in other apps. The session's own details are in
//! the session header below it.

use std::path::PathBuf;

use gpui::{Context, FontWeight, div, prelude::*, px};
use kynex_core::open::{Opener, open_project};

use super::KynexApp;
use super::keys::{ToggleSessions, ToggleTimeline};
use super::state::{Active, View};
use crate::theme::theme;
use crate::ui::{Icon, Tooltip, icon, icon_button};

impl KynexApp {
    pub(super) fn render_toolbar(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = theme(cx).clone();
        let project = self.active_project().map(|p| (p.path.clone(), p.view));
        let collapsed = self.saved.sessions_collapsed;
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
                        .tooltip(Tooltip::with_meta("Show sessions", "Ctrl+B"))
                        .on_click(cx.listener(|this, _, window, cx| this.toggle_sessions(&ToggleSessions, window, cx))),
                )
            });
        let timeline = icon_button("toggle-timeline", Icon::Timeline, cx)
            .when(self.saved.timeline_open, |b| b.bg(t.active))
            .tooltip(Tooltip::with_meta("Timeline", "Ctrl+Shift+T · jump to any message or step"))
            .on_click(cx.listener(|this, _, window, cx| this.toggle_timeline(&ToggleTimeline, window, cx)));
        let Some((path, view)) = project else {
            return bar
                .child(div().px_1().text_sm().font_weight(FontWeight::MEDIUM).child("Chat"))
                .child(div().flex_1())
                .child(timeline);
        };
        let segment = |id: &'static str, label: &'static str, which: Icon, v: View, cx: &mut Context<Self>| {
            let selected = view == v;
            let hover = t.hover;
            div()
                .id(id)
                .flex()
                .items_center()
                .gap_1p5()
                .h(px(24.))
                .px_2()
                .rounded_md()
                .text_sm()
                .cursor_pointer()
                .text_color(if selected { t.text } else { t.text_muted })
                .when(selected, |d| d.bg(t.active))
                .when(!selected, |d| d.hover(move |s| s.bg(hover)))
                .child(icon(which, if selected { t.text } else { t.text_muted }).size_3p5())
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Active::Project(i) = this.saved.active
                        && let Some(p) = this.saved.projects.get_mut(i)
                    {
                        p.view = v;
                    }
                    this.save();
                    cx.notify();
                }))
        };
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
        bar.child(segment("view-code", "Code", Icon::Message, View::Code, cx))
            .child(segment("view-git", "Git", Icon::GitBranch, View::Git, cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .px_2()
                    .truncate()
                    .text_xs()
                    .text_color(t.text_placeholder)
                    .child(path.display().to_string()),
            )
            .when(view == View::Code, |d| d.child(timeline))
            .child(open("open-code", Icon::Code, "Open in VS Code", Opener::Code, path.clone(), cx))
            .child(open("open-files", Icon::Folder, "Open in file manager", Opener::Explorer, path.clone(), cx))
            .child(
                icon_button("open-term", Icon::Terminal, cx)
                    .when(self.saved.terminal_open && view == View::Code, |b| b.bg(t.active))
                    .tooltip(Tooltip::with_meta("Terminal", "Ctrl+` · in Kynex, under the conversation"))
                    .on_click(cx.listener(|this, _, window, cx| this.terminal_button(window, cx))),
            )
    }

    /// Opens a project folder in another app; a failure shows as a notice.
    pub(super) fn open_in(&mut self, opener: Opener, path: &std::path::Path) {
        if let Err(e) = open_project(opener, path) {
            self.notice = Some(e);
        }
    }
}
