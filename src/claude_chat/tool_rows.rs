//! Tool call rows in the transcript: grouped reads and searches, compact edit
//! rows with their inline diff, and shell / generic tool calls with their output.

use gpui::{AnyElement, Context, FontWeight, SharedString, div, prelude::*};
use kynex_core::claude::diff::EditDiff;

use super::ClaudeChatView;
use super::markdown_view::{Sel, render_inline_diff, resolve_workspace_path};
use super::state::*;
use crate::theme::{Palette, mono};
use crate::ui::{Icon, Tooltip, ghost_button, icon, px, scrollbar, spinner};

/// Borrowed fields of a [`LogRow::ToolCall`] for [`ClaudeChatView::render_tool_call`].
pub(super) struct ToolCallView<'r> {
    pub(super) ix: usize,
    pub(super) id: u64,
    pub(super) tool_use_id: &'r str,
    pub(super) name: &'r str,
    pub(super) summary: &'r str,
    pub(super) command: Option<&'r str>,
    pub(super) input_preview: &'r str,
    pub(super) output: &'r str,
    pub(super) images: &'r [RowImage],
    pub(super) started_at: Option<std::time::Instant>,
    pub(super) duration_ms: Option<u64>,
    pub(super) done: bool,
    pub(super) is_error: bool,
    pub(super) expanded: bool,
}

impl ClaudeChatView {
    pub(super) fn render_read_search_run(
        &self,
        ix: usize,
        id: u64,
        entries: &[ReadSearchEntry],
        expanded: bool,
        t: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let reads = entries.iter().filter(|e| e.kind == ReadKind::Read).count();
        let searches = entries.iter().filter(|e| e.kind == ReadKind::Search).count();
        let fetches = entries.iter().filter(|e| e.kind == ReadKind::Fetch).count();
        let running = entries.iter().any(|e| !e.done);

        let mut parts = Vec::new();
        if reads > 0 {
            parts.push(format!("Read {reads} file{}", if reads == 1 { "" } else { "s" }));
        }
        if searches > 0 {
            parts.push(format!("Searched {searches}"));
        }
        if fetches > 0 {
            parts.push(format!("Fetched {fetches}"));
        }
        let summary = if parts.is_empty() { format!("Explored {}", entries.len()) } else { parts.join(" · ") };
        let chev = if expanded { Icon::ChevronDown } else { Icon::ChevronRight };

        let details: Vec<AnyElement> = if expanded {
            entries
                .iter()
                .enumerate()
                .map(|(ei, e)| {
                    let dot_col = if e.is_error {
                        t.error
                    } else if e.done {
                        t.text_placeholder
                    } else {
                        t.accent
                    };
                    let open_path = e.path.clone();
                    div()
                        .id(SharedString::from(format!("rs-{id}-{ei}")))
                        .flex()
                        .items_center()
                        .gap_2()
                        .h(px(22.))
                        .px_2()
                        .rounded_sm()
                        .hover(|s| s.bg(t.hover))
                        .when(open_path.is_some(), |d| d.cursor_pointer())
                        .child(div().size(px(5.)).rounded_full().bg(dot_col).flex_none())
                        .child(
                            div().text_xs().text_color(t.text_muted).w(px(56.)).flex_none().child(e.tool_name.clone()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .font_family(mono())
                                .text_xs()
                                .text_color(if open_path.is_some() { t.accent } else { t.text })
                                .child(e.label.clone()),
                        )
                        .when_some(open_path, |d, p| {
                            d.on_click(cx.listener(move |this, _, _, cx| {
                                let full = resolve_workspace_path(&this.config.workspace, &p);
                                cx.emit(ClaudeChatEvent::OpenFile { path: full, line: None });
                            }))
                        })
                        .into_any_element()
                })
                .collect()
        } else {
            Vec::new()
        };

        div()
            .w_full()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .id(SharedString::from(format!("rs-hdr-{id}")))
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .h(px(24.))
                    .cursor_pointer()
                    .text_xs()
                    .text_color(t.text_muted)
                    .hover(|s| s.text_color(t.text))
                    .when(running, |d| d.child(spinner(("rs-spin", id), t.accent, px(12.))))
                    .child(icon(chev, t.text_placeholder).size(px(12.)))
                    .child(summary)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(LogRow::ReadSearchRun { expanded: exp, .. }) = this.rows.get_mut(ix) {
                            *exp = !*exp;
                            this.remeasure_row(ix);
                            cx.notify();
                        }
                    })),
            )
            .when(expanded, |d| {
                d.child(
                    div().ml_3().pl_2().border_l_1().border_color(t.border_variant).flex().flex_col().children(details),
                )
            })
            .into_any_element()
    }

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
        let display_path = short_path(&self.config.workspace, path);
        let verb = if tool_name == "Write" { "Wrote" } else { "Edited" };
        let open_path = path.to_string();
        let chev = if expanded { Icon::ChevronDown } else { Icon::ChevronRight };
        let lamp_col = if is_error {
            t.error
        } else if done {
            t.success
        } else {
            t.accent
        };

        div()
            .w_full()
            .rounded_md()
            .border_1()
            .border_color(if expanded { t.border } else { t.border_variant })
            .bg(t.panel)
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(
                div()
                    .id(SharedString::from(format!("edit-hdr-{id}")))
                    .flex()
                    .items_center()
                    .gap_2()
                    .h(px(28.))
                    .px_2p5()
                    .cursor_pointer()
                    .hover(|s| s.bg(t.hover))
                    .child(if !done {
                        spinner(("edit-spin", id), t.accent, px(12.))
                    } else {
                        div().size(px(6.)).rounded_full().bg(lamp_col).flex_none().into_any_element()
                    })
                    .child(div().text_xs().text_color(t.text_muted).flex_none().child(verb))
                    .child(
                        div()
                            .id(SharedString::from(format!("edit-file-{id}")))
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_family(mono())
                            .text_xs()
                            .text_color(t.accent)
                            .cursor_pointer()
                            .hover(|s| s.underline())
                            .tooltip(Tooltip::text(format!("Open {display_path}")))
                            .child(display_path)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                let full = resolve_workspace_path(&this.config.workspace, &open_path);
                                cx.emit(ClaudeChatEvent::OpenFile { path: full, line: None });
                            })),
                    )
                    .when(diff.added > 0, |d| {
                        d.child(
                            div()
                                .flex_none()
                                .font_family(mono())
                                .text_xs()
                                .text_color(t.success)
                                .child(format!("+{}", diff.added)),
                        )
                    })
                    .when(diff.removed > 0, |d| {
                        d.child(
                            div()
                                .flex_none()
                                .font_family(mono())
                                .text_xs()
                                .text_color(t.error)
                                .child(format!("-{}", diff.removed)),
                        )
                    })
                    .child(icon(chev, t.text_placeholder).size(px(12.)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(LogRow::EditRow { expanded: exp, .. }) = this.rows.get_mut(ix) {
                            *exp = !*exp;
                            this.remeasure_row(ix);
                            cx.notify();
                        }
                    })),
            )
            .when(expanded, |d| d.child(render_inline_diff(path, diff, error_text, id, t)))
            .into_any_element()
    }

    pub(super) fn render_tool_call_sel(
        &mut self,
        sel: &Sel,
        v: ToolCallView<'_>,
        t: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ToolCallView {
            ix,
            id,
            tool_use_id,
            name,
            summary,
            command,
            input_preview,
            output,
            images,
            started_at,
            duration_ms,
            done,
            is_error,
            expanded,
        } = v;
        let lamp_col = if is_error {
            t.error
        } else if done {
            t.success
        } else {
            t.accent
        };
        let dur = duration_ms.or_else(|| started_at.map(|s| s.elapsed().as_millis() as u64)).map(format_duration);
        let chev = if expanded { Icon::ChevronDown } else { Icon::ChevronRight };
        let thumbs = self.render_row_images(id, images, t, cx);
        let shell_cmd = is_shell_tool(name).then(|| command.map(str::to_string)).flatten();
        let peek_id = (is_subagent_tool(name) && self.subagents.iter().any(|s| s.tool_use_id == tool_use_id))
            .then(|| tool_use_id.to_string());

        div()
            .w_full()
            .rounded_md()
            .border_1()
            .border_color(if is_error { t.error } else { t.border_variant })
            .bg(t.panel)
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(
                div()
                    .id(SharedString::from(format!("tool-hdr-{id}")))
                    .flex()
                    .items_center()
                    .gap_2()
                    .h(px(28.))
                    .px_2p5()
                    .cursor_pointer()
                    .hover(|s| s.bg(t.hover))
                    .child(if !done {
                        spinner(("tool-spin", id), t.accent, px(12.))
                    } else {
                        div().size(px(6.)).rounded_full().bg(lamp_col).flex_none().into_any_element()
                    })
                    .child(
                        div()
                            .flex_none()
                            .font_family(mono())
                            .font_weight(FontWeight::MEDIUM)
                            .text_xs()
                            .text_color(t.text)
                            .child(name.to_string()),
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
                    .when_some(dur, |d, dur_str| {
                        d.child(
                            div()
                                .flex_none()
                                .font_family(mono())
                                .text_size(px(11.))
                                .text_color(t.text_placeholder)
                                .child(dur_str),
                        )
                    })
                    .when_some(shell_cmd, |d, cmd| {
                        d.child(
                            ghost_button(SharedString::from(format!("tool-run-{id}")), cx)
                                .h(px(20.))
                                .px_1p5()
                                .text_xs()
                                .text_color(t.accent)
                                .tooltip(Tooltip::text("Run in workspace terminal"))
                                .child("▶")
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    cx.stop_propagation();
                                    cx.emit(ClaudeChatEvent::RunInTerminal(cmd.clone()));
                                })),
                        )
                    })
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
                    .child(icon(chev, t.text_placeholder).size(px(12.)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(LogRow::ToolCall { expanded: exp, .. }) = this.rows.get_mut(ix) {
                            *exp = !*exp;
                            this.remeasure_row(ix);
                            cx.notify();
                        }
                    })),
            )
            .child(if expanded {
                let scroll = scrollbar::handle(("tool-out-sb", id));
                let cmd_opt = command.filter(|c| !c.is_empty());
                let input_preview = input_preview.to_string();
                let show_input = cmd_opt.is_none() && !input_preview.is_empty();
                div()
                    .border_t_1()
                    .border_color(t.border_variant)
                    .bg(t.element)
                    .flex()
                    .flex_col()
                    .when_some(cmd_opt, |d, cmd| {
                        d.child(
                            div()
                                .px_2p5()
                                .py_1p5()
                                .border_b_1()
                                .border_color(t.border_variant)
                                .font_family(mono())
                                .text_size(px(11.5))
                                .text_color(t.text)
                                .child(format!("$ {cmd}")),
                        )
                    })
                    .when(show_input, |d| {
                        d.child(
                            div()
                                .px_2p5()
                                .py_1p5()
                                .border_b_1()
                                .border_color(t.border_variant)
                                .font_family(mono())
                                .text_size(px(11.))
                                .text_color(t.text_muted)
                                .child(input_preview),
                        )
                    })
                    .when(!output.is_empty(), |d| {
                        let capped = truncate_chars(output, 16_000);
                        d.child(scrollbar::vertical_over(
                            ("tool-out-sbar", id),
                            scroll.clone(),
                            div()
                                .id(SharedString::from(format!("tool-out-{id}")))
                                .max_h(px(260.))
                                .overflow_y_scroll()
                                .track_scroll(&scroll)
                                .px_2p5()
                                .py_2()
                                .font_family(mono())
                                .text_size(px(11.5))
                                .line_height(px(17.))
                                .text_color(if is_error { t.error } else { t.text_muted })
                                .child(sel.text(capped, t)),
                            |wrap| wrap.w_full(),
                        ))
                    })
                    .when(!thumbs.is_empty(), |d| d.child(div().p_2().flex().flex_wrap().gap_2().children(thumbs)))
                    .into_any_element()
            } else if !thumbs.is_empty() {
                div().px_2p5().pb_2().flex().flex_wrap().gap_2().children(thumbs).into_any_element()
            } else {
                div().into_any_element()
            })
            .into_any_element()
    }
}
