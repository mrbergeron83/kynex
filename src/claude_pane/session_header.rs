//! The strip above the transcript: what this session is (its name), how many
//! turns it has taken, what it has cost, how full its context is, and how its
//! MCP servers are doing. The pane's lamp, title bar and buttons are the app's.

use crate::ui::px;
use gpui::{AnyElement, Context, div, prelude::*};
use kynex_core::claude::mcp;
use kynex_core::claude::session::Item;
use kynex_core::fmt;

use super::{ClaudePane, Key};
use crate::theme::{Palette, mono, theme};
use crate::ui::Tooltip;

impl ClaudePane {
    pub(super) fn render_session_header(&mut self, key: Key, cx: &mut Context<Self>) -> Option<AnyElement> {
        let t = theme(cx).clone();
        let s = self.session_by_key(key)?;
        // Its name; until that comes, the first words of its first message,
        // quieter, since the whole message is right below.
        let name = self.name.clone();
        let stand_in = s.items.iter().find_map(|i| match i {
            Item::User { text, .. } if !text.trim().is_empty() => {
                Some(fmt::first_words(text, kynex_core::claude::titles::FIRST_WORDS))
            }
            _ => None,
        });
        let (turns, handed) = s.counts();
        let (cost, context, busy) = (s.total_cost, s.context, s.busy);
        let knows_cost = s.provider().provider().knows_cost();
        let servers = mcp_status(&s.mcp, &t);

        // How full the context is; past half, a click compacts it.
        let meter = (context.tokens > 0).then(|| {
            let fraction = context.fraction();
            let color = match fraction {
                Some(f) if f >= 0.9 => t.error,
                Some(f) if f >= 0.7 => t.warning,
                _ => t.text_placeholder,
            };
            // The percentage; the tokens are in the tooltip.
            let tokens = match context.window {
                Some(w) => format!("{} / {} tokens", fmt::tokens(context.tokens), fmt::tokens(w)),
                None => format!("{} tokens", fmt::tokens(context.tokens)),
            };
            let label = match fraction {
                Some(f) => format!("{:.0}%", f * 100.0),
                None => tokens.clone(),
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
                            .child(div().h_full().rounded_full().bg(color).w(px(((36. * f) as f32).max(2.)))),
                    )
                })
                .child(label)
                .map(|d| {
                    let what = match fraction {
                        Some(f) => format!("Context {:.0}% full: {tokens}", f * 100.0),
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
                        .on_click(cx.listener(move |this, _, _, cx| this.compact(Some(key), cx)))
                })
        });

        let mut stats = Vec::new();
        if turns > 0 {
            stats.push(format!("{turns} turn{}", if turns == 1 { "" } else { "s" }));
        }
        // Work the session handed to subagents.
        if handed > 0 {
            stats.push(format!("{handed} subagent{}", if handed == 1 { "" } else { "s" }));
        }
        // The CLI prices every turn as Claude; on another provider that's wrong.
        if cost > 0.0 && knows_cost {
            stats.push(format!("${cost:.3}"));
        }
        let title = div().id("session-title").flex_1().min_w_0().truncate().text_sm();
        let title = match (name, stand_in) {
            (Some(name), _) => title.text_color(t.text).child(name),
            (None, Some(words)) => title.text_color(t.text_muted).child(words),
            (None, None) => title.text_color(t.text_placeholder).child("New session"),
        };
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
                .child(title)
                .children(servers)
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

/// The session's MCP servers as a dot: green when all connected, amber while
/// one connects or needs auth, red when one failed; hovering lists them.
fn mcp_status(servers: &[mcp::Server], t: &Palette) -> Option<AnyElement> {
    let health = mcp::health(servers)?;
    let color = match health {
        mcp::Health::Ok => t.success,
        mcp::Health::Waiting => t.warning,
        mcp::Health::Failed => t.error,
    };
    let list = servers
        .iter()
        .filter(|s| s.status != mcp::Status::Disabled)
        .map(|s| format!("{} ({}): {}", mcp::server_label(&s.name), s.name, s.status.label()))
        .collect::<Vec<_>>()
        .join("\n");
    let title = match health {
        mcp::Health::Ok => "MCP servers connected",
        mcp::Health::Waiting => "Some MCP servers are connecting or need auth",
        mcp::Health::Failed => "An MCP server failed; Claude can't use its tools",
    };
    Some(
        div()
            .id("session-mcp")
            .flex()
            .flex_none()
            .items_center()
            .gap_1()
            .px_1p5()
            .h(px(20.))
            .text_xs()
            .text_color(t.text_muted)
            .child(div().size(px(6.)).rounded_full().bg(color))
            .child("MCP")
            .rounded_md()
            .tooltip(Tooltip::with_meta(title, list))
            .into_any_element(),
    )
}
