//! The History tab: commits newest first, with branch and tag labels.

use crate::ui::px;
use gpui::{AnyElement, Context, FontWeight, Hsla, div, prelude::*, uniform_list};
use kynex_core::fmt::ago_unix;
use kynex_core::git::{self, Commit, Ref, RefKind};

use super::{GitEvent, GitView, Selection};
use crate::theme::{Palette, mono, theme};
use crate::ui::wash::row_wash;
use crate::ui::{Tooltip, scrollbar, spinner};

impl GitView {
    /// The commit list. `hosted` (the Git pane's): a click asks for the commit's details in a
    /// modal ([`GitEvent::ShowDetail`]) rather than beside the list.
    pub(super) fn render_history(&mut self, hosted: bool, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx).clone();
        if self.commits.is_empty() {
            return div()
                .p_3()
                .flex()
                .items_center()
                .gap_2()
                .text_sm()
                .text_color(t.text_placeholder)
                .when(!self.history_read, |d| d.child(spinner("history-loading", t.text_placeholder, px(12.))))
                .child(if self.history_read { "No commits yet" } else { "Reading history…" })
                .into_any_element();
        }
        let commits = self.commits.clone();
        let selected = match &self.selected {
            Some(Selection::Commit(sha)) => Some(sha.clone()),
            _ => None,
        };
        let list = uniform_list(
            "git-history",
            commits.len(),
            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                this.more_history(range.end, cx);
                range
                    .map(|i| {
                        let c = &commits[i];
                        let is_selected = selected.as_deref() == Some(c.sha.as_str());
                        commit_row(i, c, is_selected, hosted, &t, cx)
                    })
                    .collect()
            }),
        )
        .track_scroll(&self.history_scroll)
        .flex_1()
        .w_full()
        .px_1p5()
        .py_1();
        let bar = scrollbar::vertical("git-history-bar", self.history_scroll.clone());
        div().relative().flex().flex_col().flex_1().min_h_0().child(list).child(bar).into_any_element()
    }

    /// Reads a page more of history when the rows in sight near the end of what is read.
    fn more_history(&mut self, seen_to: usize, cx: &mut Context<Self>) {
        let Some(limit) = git::more_history(self.commits.len(), self.history_limit, seen_to) else { return };
        if self.history_more {
            return;
        }
        self.history_more = true;
        let root = self.root.clone();
        cx.spawn(async move |this, cx| {
            let commits = cx.background_spawn(async move { git::log(&root, limit) }).await;
            this.update(cx, |this, cx| {
                this.history_more = false;
                this.history_limit = limit;
                if let Ok(c) = commits {
                    this.commits = c;
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

fn commit_row(
    i: usize,
    c: &Commit,
    selected: bool,
    hosted: bool,
    t: &Palette,
    cx: &mut Context<GitView>,
) -> gpui::AnyElement {
    let hover = t.hover;
    let sha = c.sha.clone();
    div()
        .id(("commit", i))
        .w_full()
        .overflow_hidden()
        .flex()
        .flex_col()
        .justify_center()
        .gap_0p5()
        .h(px(46.))
        .px_2()
        .rounded_md()
        .cursor_pointer()
        .relative()
        .child(row_wash(selected, hover, t.active))
        .child(
            div().flex().items_center().gap_1().min_w_0().children(c.refs.iter().map(|r| ref_badge(r, t))).child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_sm()
                    .text_color(if selected { t.text } else { t.text_muted })
                    .when(c.parents.len() > 1, |d| d.text_color(t.text_placeholder))
                    .child(c.subject.clone()),
            ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap_1p5()
                .text_xs()
                .text_color(t.text_placeholder)
                .child(div().font_family(mono()).child(c.short.clone()))
                .child("·")
                .child(div().min_w_0().truncate().child(c.author.clone()))
                .child("·")
                .child(div().flex_none().child(ago_unix(c.time))),
        )
        .tooltip(Tooltip::with_meta(c.subject.clone(), format!("{} · {}", c.short, c.author)))
        .on_click(cx.listener(move |this, _, _, cx| {
            this.select(Selection::Commit(sha.clone()), cx);
            if hosted {
                cx.emit(GitEvent::ShowDetail);
            }
        }))
        .into_any_element()
}

fn ref_badge(r: &Ref, t: &Palette) -> impl IntoElement + use<> {
    let color: Hsla = match r.kind {
        RefKind::Head => t.accent,
        RefKind::Branch => t.success,
        RefKind::Remote => t.text_muted,
        RefKind::Tag => t.warning,
    };
    div()
        .flex_none()
        .max_w(px(140.))
        .truncate()
        .px_1()
        .rounded_sm()
        .border_1()
        .border_color(color.opacity(0.5))
        .text_xs()
        .text_color(color)
        .when(r.kind == RefKind::Head, |d| d.font_weight(FontWeight::MEDIUM))
        .child(if r.kind == RefKind::Tag { format!("# {}", r.name) } else { r.name.clone() })
}
