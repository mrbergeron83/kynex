//! The Git pane's changed files as a folder tree. Each row starts with its stage box
//! (ticked: staged; a dash: partly staged); a right-click opens its menu.

use std::ops::Range;

use crate::ui::px;
use gpui::{AnyElement, Context, FontWeight, MouseButton, MouseDownEvent, SharedString, div, prelude::*, uniform_list};
use kynex_core::git::{TreeRow, TreeStatus};

use super::actions::Owned;
use super::{GitPane, GitPaneEvent};
use crate::git::changes::stage_box;
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

    /// The box that stages or unstages `what`.
    fn row_box(&mut self, id: SharedString, what: &Owned, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx).clone();
        let state = self.staged_of(what);
        let what = what.clone();
        div()
            .id(id)
            .flex_none()
            .p_0p5()
            .rounded_sm()
            .hover(|s| s.bg(t.hover))
            .child(stage_box(state, &t))
            .tooltip(Tooltip::text(match state {
                kynex_core::git::Staged::All => "Staged: click to unstage",
                kynex_core::git::Staged::Partly => "Partly staged: click to stage the rest",
                kynex_core::git::Staged::No => "Click to stage",
            }))
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.click_box(&what, cx);
            }))
            .into_any_element()
    }

    /// "Discard?" on a row whose discard was asked for: a second click does it.
    fn confirm_chip(&mut self, i: usize, what: &Owned, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.confirm.as_ref() != Some(what) {
            return None;
        }
        let (label, tip) = self.discard_words(what)?;
        let t = theme(cx).clone();
        let what = what.clone();
        Some(
            div()
                .id(("git-pane-confirm", i))
                .flex()
                .flex_none()
                .items_center()
                .h(px(20.))
                .px_1p5()
                .rounded_sm()
                .text_xs()
                .text_color(t.error)
                .cursor_pointer()
                .hover(|s| s.bg(t.hover))
                .child(label)
                .tooltip(Tooltip::text(tip))
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.discard(&what, cx);
                }))
                .into_any_element(),
        )
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
                .pl(px(4. + INDENT * depth as f32))
                .pr_2()
                .rounded_md()
                .cursor_pointer()
                .child(row_wash(false, t.hover, t.active))
        };

        match row {
            TreeRow::Folder { path, name, depth, open, added, removed, .. } => {
                let what = Owned::Folder(path.clone());
                let path = path.clone();
                let check = self.row_box(SharedString::from(format!("git-pane-box-d:{path}")), &what, cx);
                let confirm = self.confirm_chip(i, &what, cx);
                base(*depth)
                    .id(("git-pane-folder", i))
                    .child(check)
                    .child(icon(if *open { Icon::ChevronDown } else { Icon::ChevronRight }, t.text_muted).size_3())
                    .child(div().flex_1().min_w_0().truncate().text_sm().text_color(t.text_muted).child(name.clone()))
                    .children(confirm)
                    .child(counts(*added, *removed, &t))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.open_menu(what.clone(), ev.position, cx);
                        }),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.confirm = None;
                        this.toggle(&path, cx)
                    }))
                    .into_any_element()
            }
            TreeRow::File { index, name, depth } => {
                let file = self.files[*index].clone();
                let what = Owned::File(file.path.clone());
                let (letter, color) = status_mark(file.status, &t);
                let (root, path, untracked, staged) =
                    (self.selected.clone(), file.path.clone(), file.status == TreeStatus::Untracked, file.staged());
                let check = self.row_box(SharedString::from(format!("git-pane-box-f:{path}")), &what, cx);
                let confirm = self.confirm_chip(i, &what, cx);
                base(*depth)
                    .id(("git-pane-file", i))
                    .child(check)
                    // Lines up under the folder's chevron.
                    .child(
                        div()
                            .w(px(12.))
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
                    .children(confirm)
                    .child(counts(file.added, file.removed, &t))
                    .tooltip(Tooltip::text(match &file.orig_path {
                        Some(old) => format!("{old} → {}", file.path),
                        None => file.path.clone(),
                    }))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.open_menu(what.clone(), ev.position, cx);
                        }),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.confirm = None;
                        cx.emit(GitPaneEvent::OpenFile { root: root.clone(), path: path.clone(), untracked, staged })
                    }))
                    .into_any_element()
            }
        }
    }
}
