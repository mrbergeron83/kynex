//! Tool calls in the transcript, drawn as the old chat drew them: one
//! disclosure line per call (name, what it did, a "failed" marker, the diff
//! stat), opening to its input and output, or an edit's diff.

use gpui::{AnyElement, Context, FontWeight, SharedString, div, prelude::*};
use kynex_core::claude::diff::EditDiff;

use super::ClaudeChatView;
use super::markdown_view::{Sel, render_inline_diff};
use super::state::*;
use crate::theme::{Palette, mono};
use crate::ui::wash::wash;
use crate::ui::{Icon, Tooltip, ghost_button, icon, px, scrollbar, spinner};

/// Borrowed fields of a [`LogRow::ToolCall`] for [`ClaudeChatView::render_tool_call_sel`].
pub(super) struct ToolCallView<'r> {
    pub(super) ix: usize,
    pub(super) id: u64,
    pub(super) tool_use_id: &'r str,
    pub(super) name: &'r str,
    pub(super) summary: &'r str,
    pub(super) input_preview: &'r str,
    pub(super) output: &'r str,
    pub(super) images: &'r [RowImage],
    pub(super) done: bool,
    pub(super) is_error: bool,
    pub(super) expanded: bool,
}

/// The most of a tool's output put in its box (the box scrolls).
const MAX_OUTPUT: usize = 50_000;

impl ClaudeChatView {
    /// One tool call. Collapsed it's one line, with a marker if it failed;
    /// open, it shows its input and output.
    pub(super) fn render_tool_call_sel(
        &mut self,
        sel: &Sel,
        v: ToolCallView<'_>,
        t: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ToolCallView { ix, id, tool_use_id, name, summary, output, images, done, is_error, input_preview, .. } = v;
        let expanded = v.expanded || self.expand_tools;
        let running = !done && self.turn_active;
        let peek_id = (is_subagent_tool(name) && self.subagents.iter().any(|s| s.tool_use_id == tool_use_id))
            .then(|| tool_use_id.to_string());

        let head = disclosure_row(SharedString::from(format!("tool-{id}")), expanded, t)
            .when(running, |d| d.child(spinner(("tool-spin", id), t.accent, px(12.))))
            .child(
                div()
                    .flex_none()
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(t.text_muted)
                    .child(tool_label(name)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_family(mono())
                    .text_xs()
                    .text_color(t.text_muted)
                    .child(summary.to_string()),
            )
            .children(failed_mark(SharedString::from(format!("tool-err-{id}")), is_error.then_some(output), t))
            .when_some(peek_id, |d, sid| {
                d.child(
                    ghost_button(SharedString::from(format!("tool-peek-{id}")), cx)
                        .h(px(20.))
                        .px_1p5()
                        .text_xs()
                        .text_color(t.accent)
                        .tooltip(Tooltip::text("Inspect subagent output"))
                        .child("Peek ↗")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.peek_subagent = Some(sid.clone());
                            cx.notify();
                        })),
                )
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Some(LogRow::ToolCall { expanded: exp, .. }) = this.rows.get_mut(ix) {
                    *exp = !*exp;
                    this.remeasure_row(ix);
                    cx.notify();
                }
            }));

        // Pictures it returned (a screenshot, a `Read` of an image) show even folded.
        let thumbs = self.render_row_images(id, images, t, cx);
        let mut body: Vec<AnyElement> = Vec::new();
        if !thumbs.is_empty() {
            body.push(div().pl_6().pt_1().flex().flex_wrap().gap_2().children(thumbs).into_any_element());
        }
        if expanded {
            if !input_preview.is_empty() {
                body.push(code_box(SharedString::from(format!("tool-in-{id}")), input_preview.to_string(), sel, t));
            }
            if !output.trim().is_empty() && (is_error || !is_edit_tool(name)) {
                let text = truncate_chars(output, MAX_OUTPUT);
                body.push(code_box(SharedString::from(format!("tool-out-{id}")), text, sel, t));
            }
        }
        div().flex().flex_col().child(head).children(body).into_any_element()
    }

    /// A file edit: the same line as any call, opening to its diff.
    pub(super) fn render_edit_row(
        &self,
        ix: usize,
        id: u64,
        tool_name: &str,
        path: &str,
        diff: &EditDiff,
        expanded: bool,
        done: bool,
        is_error: bool,
        error_text: Option<&str>,
        t: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let expanded = expanded || self.expand_tools;
        let running = !done && self.turn_active;
        let head = disclosure_row(SharedString::from(format!("edit-{id}")), expanded, t)
            .when(running, |d| d.child(spinner(("edit-spin", id), t.accent, px(12.))))
            .child(
                div()
                    .flex_none()
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(t.text_muted)
                    .child(tool_name.to_string()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_family(mono())
                    .text_xs()
                    .text_color(t.text_muted)
                    .child(short_path(&self.config.workspace, path)),
            )
            .children(failed_mark(
                SharedString::from(format!("edit-err-{id}")),
                is_error.then_some(error_text.unwrap_or("")),
                t,
            ))
            .when(diff.added + diff.removed > 0, |d| d.child(stat(diff.added, diff.removed, t)))
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Some(LogRow::EditRow { expanded: exp, .. }) = this.rows.get_mut(ix) {
                    *exp = !*exp;
                    this.remeasure_row(ix);
                    cx.notify();
                }
            }));
        div()
            .flex()
            .flex_col()
            .child(head)
            .when(expanded, |d| {
                d.child(
                    div()
                        .ml_6()
                        .mt_1()
                        .rounded_md()
                        .overflow_hidden()
                        .border_1()
                        .border_color(t.border_variant)
                        .bg(t.panel)
                        .child(render_inline_diff(path, diff, error_text, id, t)),
                )
            })
            .into_any_element()
    }
}

/// MCP tools read as their server and tool, "playwright · browser_click".
fn tool_label(name: &str) -> String {
    match name.strip_prefix("mcp__").and_then(|rest| rest.split_once("__")) {
        Some((server, tool)) => format!("{server} · {tool}"),
        None => name.to_string(),
    }
}

/// "✗ failed", the error's first line on hover.
fn failed_mark(id: SharedString, error: Option<&str>, t: &Palette) -> Option<AnyElement> {
    let first: String = error?.lines().next().unwrap_or("").chars().take(160).collect();
    Some(
        div()
            .id(id)
            .flex_none()
            .flex()
            .items_center()
            .gap_1()
            .text_xs()
            .text_color(t.error)
            .child(icon(Icon::Close, t.error).size(px(12.)))
            .child("failed")
            .when(!first.is_empty(), |d| d.tooltip(Tooltip::text(first)))
            .into_any_element(),
    )
}

/// A line that opens and shuts what's under it: a chevron, then its content.
pub(super) fn disclosure_row(id: impl Into<gpui::ElementId>, expanded: bool, t: &Palette) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .gap_1p5()
        .h(px(22.))
        .px_1()
        .rounded_md()
        .cursor_pointer()
        .relative()
        .child(wash(t.hover, None))
        .child(icon(if expanded { Icon::ChevronDown } else { Icon::ChevronRight }, t.text_placeholder).size(px(12.)))
}

/// A tool's input or output: selectable mono text in a scrolling frame.
pub(super) fn code_box(id: SharedString, text: String, sel: &Sel, t: &Palette) -> AnyElement {
    let scroll = scrollbar::handle(id.clone());
    let bar: SharedString = format!("{id}-bar").into();
    let area = div()
        .id(id)
        .max_h(px(320.))
        .overflow_y_scroll()
        .track_scroll(&scroll)
        .px_3()
        .py_2()
        .font_family(mono())
        .text_xs()
        .text_color(t.text_muted)
        .child(sel.text(text, t));
    scrollbar::vertical_over(bar, scroll, area, |d| {
        d.ml_6().mt_1().rounded_md().overflow_hidden().border_1().border_color(t.border_variant).bg(t.panel)
    })
    .into_any_element()
}

/// "+12 −3".
pub(super) fn stat(add: usize, del: usize, t: &Palette) -> impl IntoElement + use<> {
    div()
        .flex()
        .flex_none()
        .gap_1()
        .font_family(mono())
        .text_xs()
        .when(add > 0, |d| d.child(div().text_color(t.success).child(format!("+{add}"))))
        .when(del > 0, |d| d.child(div().text_color(t.error).child(format!("−{del}"))))
}
