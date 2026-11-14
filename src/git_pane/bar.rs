//! The Git pane's title bar (the worktree shown, refresh, close) and its tabs: Changes,
//! History and Branches.

use crate::ui::px;
use gpui::{Context, FontWeight, div, prelude::*};
use kynex_core::tiling::GitTab;
use kynex_core::workspace::same_folder;

use super::{GitPane, GitPaneEvent};
use crate::app::ToggleGit;
use crate::theme::theme;
use crate::ui::wash::row_wash;
use crate::ui::{Icon, Tooltip, icon_button};

impl GitPane {
    /// The worktree the pane acts on (its branch is the picker below), refresh and close.
    pub(super) fn render_title(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = theme(cx).clone();
        let name = self.name();
        let linked = self.worktrees.read(cx).list.iter().any(|w| same_folder(&w.path, &self.selected) && !w.main);
        // The bar is the handle that drags the pane.
        crate::ui::pane_drag::pane_handle(div().id("git-pane-title"), self.pane, "Git")
            .flex()
            .flex_none()
            .items_center()
            .gap_1p5()
            .h(px(36.))
            .pl_3()
            .pr_1()
            .border_b_1()
            .border_color(t.border_variant)
            .child(div().min_w_0().truncate().text_sm().font_weight(FontWeight::MEDIUM).child(name))
            .when(linked, |d| d.child(div().flex_none().text_xs().text_color(t.info).child("worktree")))
            .child(div().flex_1())
            .child(
                icon_button("git-pane-refresh", Icon::Refresh, cx)
                    .tooltip(Tooltip::text("Re-read the repository"))
                    .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
            )
            .child(
                icon_button("git-pane-close", Icon::Close, cx)
                    .tooltip(Tooltip::for_action("Close the Git pane", ToggleGit, ""))
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(GitPaneEvent::Close))),
            )
    }

    /// Changes (with how many files), History and Branches.
    pub(super) fn render_tabs(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = theme(cx).clone();
        let n = self.files.len();
        let tab = |id: &'static str, label: String, which: GitTab, cx: &mut Context<Self>| {
            let on = self.tab == which;
            div()
                .id(id)
                .relative()
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .justify_center()
                .h(px(24.))
                .rounded_md()
                .text_sm()
                .truncate()
                .cursor_pointer()
                .text_color(if on { t.text } else { t.text_muted })
                .when(on, |d| d.font_weight(FontWeight::MEDIUM))
                .child(row_wash(on, t.hover, t.active))
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.set_tab(which, cx);
                    cx.emit(GitPaneEvent::Tab(which));
                }))
        };
        let changes = if n > 0 { format!("Changes ({n})") } else { "Changes".into() };
        div()
            .flex()
            .flex_none()
            .gap_1()
            .p_1()
            .border_b_1()
            .border_color(t.border_variant)
            .child(tab("git-pane-tab-changes", changes, GitTab::Changes, cx))
            .child(tab("git-pane-tab-history", "History".into(), GitTab::History, cx))
            .child(tab("git-pane-tab-branches", "Branches".into(), GitTab::Branches, cx))
    }
}
