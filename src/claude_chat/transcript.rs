//! Virtualized transcript (`gpui::list` + `scrollbar::vertical`), sticky bottom
//! auto-scroll with floating "↓ Latest" button, `Ctrl+F` find bar, and every
//! transcript row renderer (user prompt, auto-folded thought, grouped read/search
//! entries, compact 1-line edit rows with collapsible syntax-colored inline diffs,
//! shell & generic tool rows, prose markdown, compaction dividers, notices).

use gpui::{AnyElement, Context, FontStyle, FontWeight, SharedString, StyledText, Window, div, list, prelude::*};
use kynex_core::claude::diff::EditDiff;

use super::markdown_view::{self, render_inline_diff, resolve_workspace_path};
use super::state::*;
use super::{ClaudeChatView, centered_column};
use crate::theme::{Palette, mono, theme, ui_font};
use crate::ui::{Icon, Tooltip, ghost_button, icon, px, scrollbar, spinner};

/// Borrowed fields of a [`LogRow::ToolCall`] for [`ClaudeChatView::render_tool_call`].
struct ToolCallView<'r> {
    ix: usize,
    id: u64,
    tool_use_id: &'r str,
    name: &'r str,
    summary: &'r str,
    command: Option<&'r str>,
    input_preview: &'r str,
    output: &'r str,
    images: &'r [RowImage],
    started_at: Option<std::time::Instant>,
    duration_ms: Option<u64>,
    done: bool,
    is_error: bool,
    expanded: bool,
}

impl ClaudeChatView {
    /// Renders the virtualized transcript area + scrollbar + floating "↓ Latest" pill.
    pub(super) fn render_transcript(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = theme(cx).clone();

        // Track sticky-bottom scroll state passively without any polling timer.
        if let Some(at_end) = self.list_state.is_scrolled_to_end() {
            self.stick_to_bottom = at_end;
        }
        let show_latest = !self.rows.is_empty() && !self.stick_to_bottom;

        if self.rows.is_empty() {
            return div()
                .relative()
                .flex_1()
                .min_h_0()
                .bg(t.editor)
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_2()
                .px_6()
                .child(icon(Icon::Bot, t.accent).size(px(28.)))
                .child(
                    div()
                        .font_family(ui_font())
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_size(px(15.))
                        .text_color(t.text)
                        .child(if self.history_loading { "Loading session…" } else { "Claude Code" }),
                )
                .child(div().text_xs().text_color(t.text_muted).child(
                    self.config.workspace.file_name().and_then(|n| n.to_str()).unwrap_or("Workspace").to_string(),
                ))
                .when(!self.history_loading, |d| {
                    d.child(
                        div()
                            .mt_2()
                            .text_xs()
                            .text_color(t.text_placeholder)
                            .child("Type a message, paste screenshots (Ctrl+V), / commands, or @ files"),
                    )
                });
        }

        let view = cx.entity().clone();
        let list_el = list(self.list_state.clone(), move |ix, window, cx| {
            view.update(cx, |this, cx| this.render_item(ix, window, cx))
        })
        .size_full();

        div()
            .relative()
            .flex_1()
            .min_h_0()
            .bg(t.editor)
            .child(list_el)
            .child(scrollbar::vertical("claude-transcript-sb", self.list_state.clone()))
            .when(show_latest, |d| {
                d.child(
                    div().absolute().bottom_3().right_4().child(
                        div()
                            .id("claude-jump-latest")
                            .flex()
                            .items_center()
                            .gap_1()
                            .h(px(26.))
                            .px_2p5()
                            .rounded_full()
                            .border_1()
                            .border_color(t.border)
                            .bg(t.elevated)
                            .shadow_md()
                            .text_xs()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(t.text)
                            .cursor_pointer()
                            .hover(|s| s.border_color(t.border_focused).bg(t.hover))
                            .child(icon(Icon::ArrowDown, t.accent).size(px(12.)))
                            .child("Latest")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.scroll_to_bottom();
                                cx.notify();
                            })),
                    ),
                )
            })
    }

    /// Row `ix` drawn on its own in the list, in the reading column.
    pub(super) fn render_row(&mut self, ix: usize, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx).clone();
        let is_find_hit = self.find.open && self.find.matches.contains(&ix);
        let is_current_hit = self.find.open && self.find.matches.get(self.find.current) == Some(&ix);
        let inner = self.render_row_body(ix, cx);
        div()
            .w_full()
            .px_4()
            .py_1()
            .when(is_find_hit, |d| d.bg(if is_current_hit { t.selection } else { t.hover }))
            .child(centered_column(inner))
            .into_any_element()
    }

    /// Row `ix`'s own content, without the column around it (runs draw their
    /// calls with this).
    pub(super) fn render_row_body(&mut self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        if ix >= self.rows.len() {
            return div().into_any_element();
        }
        // The list re-renders every visible row on each scroll frame: borrow the row
        // (swapped out for a cheap placeholder) instead of deep-cloning its text, blocks,
        // diffs and tool output every frame.
        let placeholder = LogRow::CompactionDivider { detail: String::new() };
        let row = std::mem::replace(&mut self.rows[ix], placeholder);
        let el = self.row_body(ix, &row, cx);
        self.rows[ix] = row;
        el
    }

    fn row_body(&mut self, ix: usize, row: &LogRow, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx).clone();
        match row {
            LogRow::UserPrompt { id, text, images, file_chips, paste_labels, .. } => {
                self.render_user_prompt(*id, text, images, file_chips, paste_labels, &t, cx)
            }
            LogRow::Thought { id, text, started_at, duration_ms, expanded, streaming } => {
                self.render_thought(ix, *id, text, *started_at, *duration_ms, *expanded, *streaming, &t, cx)
            }
            LogRow::ReadSearchRun { id, entries, expanded } => {
                self.render_read_search_run(ix, *id, entries, *expanded, &t, cx)
            }
            LogRow::EditRow { id, tool_name, path, diff, expanded, done, is_error, error_text, .. } => self
                .render_edit_row(
                    ix,
                    *id,
                    tool_name,
                    path,
                    diff,
                    *expanded,
                    *done,
                    *is_error,
                    error_text.as_deref(),
                    &t,
                    cx,
                ),
            LogRow::ToolCall {
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
            } => self.render_tool_call(
                ToolCallView {
                    ix,
                    id: *id,
                    tool_use_id,
                    name,
                    summary,
                    command: command.as_deref(),
                    input_preview,
                    output,
                    images,
                    started_at: *started_at,
                    duration_ms: *duration_ms,
                    done: *done,
                    is_error: *is_error,
                    expanded: *expanded,
                },
                &t,
                cx,
            ),
            LogRow::Prose { id, blocks, streaming, .. } => {
                let rendered = markdown_view::render_blocks(blocks, *id, self.copied_key.as_deref(), cx);
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .children(rendered)
                    .when(*streaming, |d| d.child(div().w(px(7.)).h(px(13.)).rounded_xs().bg(t.accent)))
                    .into_any_element()
            }
            LogRow::CompactionDivider { detail } => div()
                .w_full()
                .py_1()
                .flex()
                .items_center()
                .gap_2()
                .child(div().flex_1().h(px(1.)).bg(t.border_variant))
                .child(div().text_xs().text_color(t.text_placeholder).child(detail.clone()))
                .child(div().flex_1().h(px(1.)).bg(t.border_variant))
                .into_any_element(),
            LogRow::Notice { text, is_error } => div()
                .w_full()
                .px_3()
                .py_1p5()
                .rounded_md()
                .border_1()
                .border_color(if *is_error { t.error } else { t.border_variant })
                .bg(t.panel)
                .text_xs()
                .text_color(if *is_error { t.error } else { t.text_muted })
                .child(text.clone())
                .into_any_element(),
        }
    }

    fn render_user_prompt(
        &mut self,
        id: u64,
        text: &str,
        images: &[RowImage],
        file_chips: &[String],
        paste_labels: &[String],
        t: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let thumbs = self.render_row_images(id, images, t, cx);

        let chips: Vec<AnyElement> = file_chips
            .iter()
            .cloned()
            .enumerate()
            .map(|(ci, path)| {
                let open_path = path.clone();
                div()
                    .id(SharedString::from(format!("u{id}-f{ci}")))
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_1p5()
                    .h(px(20.))
                    .rounded_sm()
                    .border_1()
                    .border_color(t.border_variant)
                    .bg(t.element)
                    .font_family(mono())
                    .text_size(px(11.))
                    .text_color(t.accent)
                    .cursor_pointer()
                    .hover(|s| s.border_color(t.border_focused))
                    .child(icon(Icon::File, t.accent).size(px(10.)))
                    .child(path)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let full = resolve_workspace_path(&this.config.workspace, &open_path);
                        cx.emit(ClaudeChatEvent::OpenFile { path: full, line: None });
                    }))
                    .into_any_element()
            })
            .chain(paste_labels.iter().cloned().map(|lbl| {
                div()
                    .px_1p5()
                    .h(px(20.))
                    .flex()
                    .items_center()
                    .rounded_sm()
                    .border_1()
                    .border_color(t.border_variant)
                    .bg(t.element)
                    .font_family(mono())
                    .text_size(px(11.))
                    .text_color(t.text_muted)
                    .child(lbl)
                    .into_any_element()
            }))
            .collect();

        div()
            .w_full()
            .my_1()
            .pl_3()
            .pr_2()
            .py_2()
            .border_l_2()
            .border_color(t.accent)
            .bg(t.panel)
            .rounded_r_md()
            .flex()
            .flex_col()
            .gap_1p5()
            .when(!text.is_empty(), |d| {
                d.child(
                    div()
                        .font_family(ui_font())
                        .text_size(px(13.5))
                        .line_height(px(20.))
                        .text_color(t.text)
                        .child(text.to_string()),
                )
            })
            .when(!chips.is_empty(), |d| d.child(div().flex().flex_wrap().gap_1().children(chips)))
            .when(!thumbs.is_empty(), |d| d.child(div().flex().flex_wrap().gap_2().children(thumbs)))
            .into_any_element()
    }

    fn render_thought(
        &self,
        ix: usize,
        id: u64,
        text: &str,
        started_at: Option<std::time::Instant>,
        duration_ms: Option<u64>,
        expanded: bool,
        streaming: bool,
        t: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let label = if streaming {
            let secs = started_at.map_or(0, |s| s.elapsed().as_secs());
            format!("Thinking… {secs}s")
        } else if let Some(ms) = duration_ms {
            format!("Thought for {}", format_duration(ms))
        } else {
            "Thought".into()
        };
        let chev = if expanded { Icon::ChevronDown } else { Icon::ChevronRight };

        div()
            .w_full()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .id(SharedString::from(format!("thought-{id}")))
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .h(px(24.))
                    .cursor_pointer()
                    .text_xs()
                    .text_color(t.text_muted)
                    .hover(|s| s.text_color(t.text))
                    .when(streaming, |d| d.child(spinner(("thought-spin", id), t.accent, px(12.))))
                    .child(icon(chev, t.text_placeholder).size(px(12.)))
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(LogRow::Thought { expanded: exp, .. }) = this.rows.get_mut(ix) {
                            *exp = !*exp;
                            this.remeasure_row(ix);
                            cx.notify();
                        }
                    })),
            )
            .when(expanded, |d| {
                d.child(
                    div()
                        .ml_3()
                        .pl_2p5()
                        .py_1()
                        .border_l_1()
                        .border_color(t.border_variant)
                        .font_family(ui_font())
                        .text_xs()
                        .line_height(px(18.))
                        .text_color(t.text_muted)
                        .child(StyledText::new(text.to_string()).with_highlights(vec![(
                            0..text.len(),
                            gpui::HighlightStyle { font_style: Some(FontStyle::Italic), ..Default::default() },
                        )])),
                )
            })
            .into_any_element()
    }

    fn render_read_search_run(
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

    fn render_edit_row(
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

    fn render_tool_call(&mut self, v: ToolCallView<'_>, t: &Palette, cx: &mut Context<Self>) -> AnyElement {
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
                                .child(capped),
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
