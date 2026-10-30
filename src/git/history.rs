//! The History tab: commits newest first, with branch and tag labels.

use crate::ui::px;
use gpui::{Context, FontWeight, Hsla, div, prelude::*, uniform_list};
use kynex_core::fmt::ago_unix;
use kynex_core::git::{Commit, Ref, RefKind};

use super::{GitView, Selection};
use crate::theme::{Palette, mono, theme};
use crate::ui::wash::row_wash;
use crate::ui::{Tooltip, scrollbar};

impl GitView {
    pub(super) fn render_history(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = theme(cx).clone();
        if self.commits.is_empty() {
            return div().p_3().text_sm().text_color(t.text_placeholder).child("No commits yet").into_any_element();
        }
        let commits = self.commits.clone();
        let selected = match &self.selected {
            Some(Selection::Commit(sha)) => Some(sha.clone()),
            _ => None,
        };
        let list = uniform_list(
            "git-history",
            commits.len(),
            cx.processor(move |_, range: std::ops::Range<usize>, _, cx| {
                range
                    .map(|i| {
                        let c = &commits[i];
                        let is_selected = selected.as_deref() == Some(c.sha.as_str());
                        commit_row(i, c, is_selected, &t, cx)
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
}

fn commit_row(i: usize, c: &Commit, selected: bool, t: &Palette, cx: &mut Context<GitView>) -> gpui::AnyElement {
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
        .on_click(cx.listener(move |this, _, _, cx| this.select(Selection::Commit(sha.clone()), cx)))
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
