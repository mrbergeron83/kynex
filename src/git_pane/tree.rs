//! The Git pane's changed files as a folder tree.

use std::ops::Range;

use crate::ui::px;
use gpui::{AnyElement, Context, FontWeight, div, prelude::*, uniform_list};
use kynex_core::git::{TreeRow, TreeStatus};

use super::{GitPane, GitPaneEvent};
use crate::git::diff_view::counts;
use crate::theme::{Palette, mono, theme};
use crate::ui::wash::row_wash;
use crate::ui::{Icon, Tooltip, icon, scrollbar, spinner};

const ROW_HEIGHT: f32 = 24.;
const INDENT: f32 = 12.;

/// The status letter and its colour.
fn status_mark(status: TreeStatus, t: &Palette) -> (&'static str, gpui::Hsla) {
    match status {
        TreeStatus::Added => ("A", t.success),
        TreeStatus::Modified => ("M", t.warning),
        TreeStatus::Deleted => ("D", t.error),
        TreeStatus::Renamed => ("R", t.info),
        TreeStatus::Untracked => ("U", t.success),
        TreeStatus::Conflicted => ("!", t.error),
    }
}

impl GitPane {
    pub(super) fn render_tree(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx).clone();
        let message = |text: String, color| {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .p_4()
                .text_sm()
                .text_color(color)
                .child(text)
                .into_any_element()
        };
        if let Some(e) = &self.error {
            return message(e.clone(), t.error);
        }
        if !self.loaded {
            return div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .child(spinner("git-pane-loading", t.text_placeholder, px(16.)))
                .into_any_element();
        }
        if self.rows.is_empty() {
            return message("No changes".into(), t.text_placeholder);
        }
        let list = uniform_list(
            "git-pane-tree",
            self.rows.len(),
            cx.processor(|this, range: Range<usize>, _, cx| {
                let rows = this.rows.clone();
                range.map(|i| this.render_row(i, &rows[i], cx)).collect()
            }),
        )
        .track_scroll(&self.scroll)
        .flex_1();
        div()
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .p_1()
            .child(list)
            .child(scrollbar::vertical("git-pane-bar", self.scroll.clone()))
            .into_any_element()
    }

    fn render_row(&mut self, i: usize, row: &TreeRow, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx).clone();
        let base = |depth: usize| {
            div()
                .relative()
                .flex()
                .items_center()
                .gap_1p5()
                .h(px(ROW_HEIGHT))
                .pl(px(6. + INDENT * depth as f32))
                .pr_2()
                .rounded_md()
                .cursor_pointer()
                .child(row_wash(false, t.hover, t.active))
        };
        match row {
            TreeRow::Folder { path, name, depth, open, added, removed, .. } => {
                let path = path.clone();
                base(*depth)
                    .id(("git-pane-folder", i))
                    .child(icon(if *open { Icon::ChevronDown } else { Icon::ChevronRight }, t.text_muted).size_3())
                    .child(div().flex_1().min_w_0().truncate().text_sm().text_color(t.text_muted).child(name.clone()))
                    .child(counts(*added, *removed, &t))
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle(&path, cx)))
                    .into_any_element()
            }
            TreeRow::File { index, name, depth } => {
                let file = self.files[*index].clone();
                let (letter, color) = status_mark(file.status, &t);
                let (root, path, untracked) =
                    (self.selected.clone(), file.path.clone(), file.status == TreeStatus::Untracked);
                base(*depth)
                    .id(("git-pane-file", i))
                    // Lines up under the folder's chevron-and-name.
                    .child(
                        div()
                            .w(px(14.))
                            .flex_none()
                            .flex()
                            .justify_center()
                            .font_family(mono())
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(color)
                            .child(letter),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .when(file.status == TreeStatus::Deleted, |d| d.line_through())
                            .child(name.clone()),
                    )
                    .child(counts(file.added, file.removed, &t))
                    .tooltip(Tooltip::text(match &file.orig_path {
                        Some(old) => format!("{old} → {}", file.path),
                        None => file.path.clone(),
                    }))
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(GitPaneEvent::OpenFile { root: root.clone(), path: path.clone(), untracked })
                    }))
                    .into_any_element()
            }
        }
    }
}
