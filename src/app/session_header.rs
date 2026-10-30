//! The strip above the transcript: what this session is (its name),
//! where it runs (branch, and whether that's a separate worktree), how many
//! turns it has taken, what it has cost and how full its context is.

use crate::ui::px;
use gpui::{AnyElement, ClickEvent, Context, FontWeight, div, prelude::*};
use kynex_core::git::Worktree;
use kynex_core::session::{Item, Kind};
use kynex_core::{fmt, mcp};

use super::KynexApp;
use crate::theme::{Palette, mono, theme};
use crate::ui::{Icon, Tooltip, icon};

impl KynexApp {
    pub(super) fn render_session_header(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let t = theme(cx).clone();
        let s = self.active_session()?;
        // Its name; until that comes, the first words of its first message,
        // quieter, since the whole message is right below.
        let name = s.session_id.as_deref().and_then(|id| self.session_name(id));
        let stand_in = s.items.iter().find_map(|i| match i {
            Item::User { text, .. } if !text.trim().is_empty() => Some(fmt::first_words(text, 8)),
            _ => None,
        });
        let turns = s.items.iter().filter(|i| matches!(i, Item::Result { .. })).count();
        let (cost, kind, cwd, context, busy) = (s.total_cost, s.kind, s.cwd.clone(), s.context, s.busy);
        let id = s.session_id.clone();
        // The session's files, once it's saved and has a folder to keep them in.
        let scratch = (kind == Kind::Code && id.is_some()).then(|| {
            let hover = t.hover;
            div()
                .id("session-scratch")
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
                .cursor_pointer()
                .hover(move |s| s.bg(hover))
                .child(icon(Icon::Files, t.text_muted).size_3())
                .child("Files")
                .tooltip(Tooltip::with_meta(
                    "This session's files",
                    "its scratch folder, what it edited, its images and task output",
                ))
                .on_click(cx.listener(|this, _, window, cx| this.open_session_scratch(window, cx)))
        });
        let servers = mcp_status(&s.mcp, &t, cx);
        // Double-clicking the name renames the session in place.
        let field = id.as_deref().and_then(|id| self.session_name_field(id, true, cx));

        let place = if kind == Kind::Code { self.worktrees(&cwd, cx).read(cx).at(&cwd).cloned() } else { None };
        // Where a project session runs: its branch, flagged when it's a linked
        // worktree, and what's changed there. A click opens its Git view.
        let place = place.map(|w| {
            let where_ = if w.main { "Main worktree" } else { "Worktree" };
            let path = w.path.clone();
            let hover = t.hover;
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
                .cursor_pointer()
                .hover(move |s| s.bg(hover))
                .child(icon(Icon::GitBranch, t.text_muted).size_3())
                .child(w.label().to_string())
                .when(!w.main, |d| d.child(div().text_color(t.info).child("worktree")))
                .children(git_stats(&w, &t))
                .tooltip(Tooltip::with_meta(format!("{where_} at {}", w.path.display()), "Click to see its changes"))
                .on_click(cx.listener(move |this, _, _, cx| this.open_git_modal(path.clone(), cx)))
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
                .children(git_stats(&w, &t))
                .tooltip(Tooltip::with_meta(
                    format!("Claude is editing the worktree at {}", w.path.display()),
                    "Click to see its changes",
                ))
                .on_click(cx.listener(move |this, _, _, cx| this.open_git_modal(path.clone(), cx)))
        });

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
                .child(match field {
                    Some(field) => field.flex_1().into_any_element(),
                    None => {
                        let title = div().id("session-title").flex_1().min_w_0().truncate().text_sm();
                        let title = match (name, stand_in) {
                            (Some(name), _) => title.font_weight(FontWeight::MEDIUM).text_color(t.text).child(name),
                            (None, Some(words)) => title.text_color(t.text_muted).child(words),
                            (None, None) => title.text_color(t.text_placeholder).child("New session"),
                        };
                        // A session not saved yet has nothing to name.
                        match id {
                            Some(id) => title
                                .tooltip(Tooltip::text("Double-click to rename"))
                                .on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
                                    if ev.click_count() == 2 {
                                        this.start_session_rename(cwd.clone(), id.clone(), true, window, cx);
                                    }
                                }))
                                .into_any_element(),
                            None => title.into_any_element(),
                        }
                    }
                })
                .children(place)
                .children(working)
                .children(servers)
                .children(scratch)
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
/// one connects or needs auth, red when one failed; hovering lists them, and a
/// click opens the Extensions modal on them.
fn mcp_status(servers: &[mcp::Server], t: &Palette, cx: &mut Context<KynexApp>) -> Option<AnyElement> {
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
    let hover = t.hover;
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
            .cursor_pointer()
            .hover(move |s| s.bg(hover))
            .tooltip(Tooltip::with_meta(title, format!("{list}\n\nClick for details")))
            .on_click(cx.listener(|this, _, _, cx| this.open_extensions(0, cx)))
            .into_any_element(),
    )
}

/// A worktree's uncommitted changes and distance from its upstream, for a
/// branch chip: `3 files +42 −7 ↑2 ↓1`; nothing when it's clean and in sync.
fn git_stats(w: &Worktree, t: &Palette) -> Vec<AnyElement> {
    let mut out = Vec::new();
    let stat = |text: String, color| div().font_family(mono()).text_color(color).child(text).into_any_element();
    if let Some(n) = w.changes.filter(|&n| n > 0) {
        out.push(stat(format!("{n} file{}", if n == 1 { "" } else { "s" }), t.warning));
        if let Some((added, removed)) = w.lines {
            out.push(stat(format!("+{added}"), t.success));
            out.push(stat(format!("−{removed}"), t.error));
        }
    }
    if let Some((ahead, behind)) = w.upstream {
        if ahead > 0 {
            out.push(stat(format!("↑{ahead}"), t.text_muted));
        }
        if behind > 0 {
            out.push(stat(format!("↓{behind}"), t.text_muted));
        }
    }
    out
}
