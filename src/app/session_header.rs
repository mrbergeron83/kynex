//! The strip above the transcript: what this session is (its first message),
//! where it runs (branch, and whether that's a separate worktree), how many
//! turns it has taken and what it has cost.

use gpui::{AnyElement, Context, FontWeight, div, prelude::*, px};
use kynex_core::session::{Item, Kind};

use super::KynexApp;
use crate::theme::{mono, theme};
use crate::ui::{Icon, Tooltip, icon};

impl KynexApp {
    pub(super) fn render_session_header(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let t = theme(cx).clone();
        let s = self.active_session()?;
        let title = s.items.iter().find_map(|i| match i {
            Item::User { text, .. } if !text.is_empty() => Some(text.split_whitespace().collect::<Vec<_>>().join(" ")),
            _ => None,
        });
        let turns = s.items.iter().filter(|i| matches!(i, Item::Result { .. })).count();
        let (cost, kind, cwd) = (s.total_cost, s.kind, s.cwd.clone());

        // Where a project session runs: its branch, flagged when it's a linked worktree.
        let place = if kind == Kind::Code { self.worktrees(&cwd, cx).read(cx).at(&cwd).cloned() } else { None };
        let place = place.map(|w| {
            let where_ = if w.main { "Main worktree" } else { "Worktree" };
            div()
                .id("session-branch")
                .flex()
                .flex_none()
                .items_center()
                .gap_1()
                .px_1p5()
                .h(px(20.))
                .rounded_md()
                .border_1()
                .border_color(t.border_variant)
                .text_xs()
                .text_color(t.text_muted)
                .child(icon(Icon::GitBranch, t.text_muted).size_3())
                .child(w.label().to_string())
                .when(!w.main, |d| d.child(div().text_color(t.info).child("worktree")))
                .when_some(w.changes.filter(|&n| n > 0), |d, n| {
                    d.child(div().text_color(t.warning).child(format!("{n} changed")))
                })
                .tooltip(Tooltip::text(format!("{where_} at {}", w.path.display())))
        });

        let mut stats = Vec::new();
        if turns > 0 {
            stats.push(format!("{turns} turn{}", if turns == 1 { "" } else { "s" }));
        }
        if cost > 0.0 {
            stats.push(format!("${cost:.3}"));
        }
        Some(
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap_2()
                .h(px(32.))
                .px_3()
                .border_b_1()
                .border_color(t.border_variant)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(if title.is_some() { t.text } else { t.text_placeholder })
                        .child(title.unwrap_or_else(|| "New session".into())),
                )
                .children(place)
                .when(!stats.is_empty(), |d| {
                    d.child(
                        div()
                            .flex_none()
                            .text_xs()
                            .font_family(mono())
                            .text_color(t.text_placeholder)
                            .child(stats.join(" · ")),
                    )
                })
                .into_any_element(),
        )
    }
}
