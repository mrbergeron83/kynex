//! The strip above the transcript: what this session is (its first message),
//! where it runs (branch, and whether that's a separate worktree), how many
//! turns it has taken, what it has cost and how full its context is.

use gpui::{AnyElement, Context, FontWeight, div, prelude::*, px};
use kynex_core::fmt;
use kynex_core::session::{Item, Kind};

use super::KynexApp;
use crate::theme::{mono, theme};
use crate::ui::{Icon, Tooltip, icon};

impl KynexApp {
    pub(super) fn render_session_header(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let t = theme(cx).clone();
        let s = self.active_session()?;
        let name = s.session_id.as_deref().and_then(|id| self.session_name(id));
        let title = name.or_else(|| {
            s.items.iter().find_map(|i| match i {
                Item::User { text, .. } if !text.is_empty() => {
                    Some(text.split_whitespace().collect::<Vec<_>>().join(" "))
                }
                _ => None,
            })
        });
        let turns = s.items.iter().filter(|i| matches!(i, Item::Result { .. })).count();
        let (cost, kind, cwd, context, busy) = (s.total_cost, s.kind, s.cwd.clone(), s.context, s.busy);

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

        // Where Claude is actually working, when that's another worktree: one
        // click opens its changes.
        let working = self.working_worktree(cx).map(|w| {
            let path = w.path.clone();
            let accent = t.accent;
            div()
                .id("session-working-in")
                .flex()
                .flex_none()
                .items_center()
                .gap_1()
                .px_1p5()
                .h(px(20.))
                .rounded_md()
                .border_1()
                .border_color(accent.opacity(0.5))
                .bg(accent.opacity(0.1))
                .text_xs()
                .text_color(accent)
                .cursor_pointer()
                .hover(move |s| s.bg(accent.opacity(0.2)))
                .child("working in")
                .child(icon(Icon::GitBranch, accent).size_3())
                .child(w.label().to_string())
                .when_some(w.changes.filter(|&n| n > 0), |d, n| d.child(format!("· {n} changed")))
                .tooltip(Tooltip::with_meta(
                    format!("Claude is editing the worktree at {}", w.path.display()),
                    "Click to see its changes",
                ))
                .on_click(cx.listener(move |this, _, _, cx| this.open_worktree_changes(path.clone(), cx)))
        });

        // How full the context is; past half, a click compacts it.
        let meter = (context.tokens > 0).then(|| {
            let fraction = context.fraction();
            let color = match fraction {
                Some(f) if f >= 0.9 => t.error,
                Some(f) if f >= 0.7 => t.warning,
                _ => t.text_placeholder,
            };
            let label = match context.window {
                Some(w) => format!("{} / {}", fmt::tokens(context.tokens), fmt::tokens(w)),
                None => format!("{} tokens", fmt::tokens(context.tokens)),
            };
            let can_compact = !busy && fraction.is_some_and(|f| f >= 0.5);
            let hover = t.hover;
            div()
                .id("context-meter")
                .flex()
                .flex_none()
                .items_center()
                .gap_1p5()
                .px_1p5()
                .h(px(20.))
                .rounded_md()
                .text_xs()
                .font_family(mono())
                .text_color(color)
                .when_some(fraction, |d, f| {
                    d.child(
                        div()
                            .w(px(36.))
                            .h(px(4.))
                            .rounded_full()
                            .bg(t.border_variant)
                            .child(div().h_full().rounded_full().bg(color).w(px((36. * f) as f32).max(px(2.)))),
                    )
                })
                .child(label)
                .map(|d| {
                    let what = match fraction {
                        Some(f) => format!("Context {:.0}% full", f * 100.0),
                        None => "Context in use".to_string(),
                    };
                    if can_compact {
                        d.tooltip(Tooltip::with_meta(
                            what,
                            "Click to compact: Claude summarizes the conversation so far",
                        ))
                    } else {
                        d.tooltip(Tooltip::text(what))
                    }
                })
                .when(can_compact, |d| {
                    d.cursor_pointer()
                        .hover(move |s| s.bg(hover))
                        .on_click(cx.listener(|this, _, _, cx| this.compact(cx)))
                })
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
                .children(working)
                .children(meter)
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
