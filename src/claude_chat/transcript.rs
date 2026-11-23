//! Virtualized transcript (`gpui::list` + `scrollbar::vertical`), sticky bottom
//! auto-scroll with floating "↓ Latest" button, `Ctrl+F` find bar, and every
//! transcript row renderer (user prompt, auto-folded thought, grouped read/search
//! entries, compact 1-line edit rows with collapsible syntax-colored inline diffs,
//! shell & generic tool rows, prose markdown, compaction dividers, notices).

use gpui::{AnyElement, Context, FontWeight, SharedString, Window, div, list, prelude::*};

use super::ClaudeChatView;
use super::markdown_view::{Sel, resolve_workspace_path};
use super::state::*;
use super::tool_rows::{ToolCallView, disclosure_row};
use crate::theme::{Palette, mono, theme, ui_font};
use crate::ui::{Icon, Tooltip, icon, px, scrollbar};
use kynex_core::claude::composer::slash_command;

impl ClaudeChatView {
    /// Renders the virtualized transcript area + scrollbar + floating "↓ Latest" pill.
    pub(super) fn render_transcript(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = theme(cx).clone();

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

        // Measure the usable width (inside the rows' padding) for the wide tool lane;
        // a change redraws once with the new width.
        let width = self.column_width.clone();
        let probe_view = cx.entity().downgrade();
        let probe = gpui::canvas(
            move |bounds, _, cx| {
                let w = f32::from(bounds.size.width) - 2. * super::runs::ROW_PADDING;
                if (width.get() - w).abs() > 0.5 {
                    width.set(w);
                    probe_view.update(cx, |_, cx| cx.notify()).ok();
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();

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
            .child(probe)
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
        let (compact, wide) = self.rows.get(ix).map_or((false, false), row_spacing);
        let inner = self.render_row_body(ix, cx);
        let el = self
            .column_row(inner, compact, wide)
            .when(is_find_hit, |d| d.bg(if is_current_hit { t.selection } else { t.hover }));
        self.flash_rows(ix..ix + 1, el, &t)
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
        let ws = self.config.workspace.clone();
        let sel = Sel::new(self.selection_group(cx), ix, &ws);
        match row {
            LogRow::UserPrompt { id, text, images, file_chips, paste_labels, .. } => {
                self.render_user_prompt(*id, text, images, file_chips, paste_labels, &sel, &t, cx)
            }
            LogRow::Thought { id, text, started_at, duration_ms, expanded, streaming } => {
                self.render_thought(ix, *id, text, *started_at, *duration_ms, *expanded, *streaming, &sel, &t, cx)
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
                input_preview,
                output,
                images,
                done,
                is_error,
                expanded,
                ..
            } => self.render_tool_call_sel(
                &sel,
                ToolCallView {
                    ix,
                    id: *id,
                    tool_use_id,
                    name,
                    summary,
                    input_preview,
                    output,
                    images,
                    done: *done,
                    is_error: *is_error,
                    expanded: *expanded,
                },
                &t,
                cx,
            ),
            LogRow::Prose { id, raw, .. } => {
                // Commands Claude suggests (```bash / ```powershell) run in the project's terminal.
                let this = cx.entity().downgrade();
                let run: crate::markdown::RunCommand = std::rc::Rc::new(move |_shell, command, _window, cx| {
                    this.update(cx, |_, cx| cx.emit(ClaudeChatEvent::RunInTerminal(command))).ok();
                });
                let wide = self.wide_width();
                div()
                    .text_sm()
                    .child(crate::markdown::render_wide(
                        SharedString::from(format!("md-{id}")),
                        raw,
                        sel.group,
                        ix,
                        Some(run),
                        Some(&ws),
                        wide,
                        cx,
                    ))
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
            LogRow::Shell { id, command, output, sent } => {
                self.render_shell_run(*id, command, output.as_ref(), *sent, &sel, &t)
            }
            LogRow::PermissionNote { title, allowed, note } => {
                let (glyph, color) = if *allowed { (Icon::Check, t.success) } else { (Icon::Close, t.error) };
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_1()
                    .text_xs()
                    .child(icon(glyph, color).size(px(12.)))
                    .child(div().min_w_0().truncate().text_color(t.text_muted).child(title.clone()))
                    .child(div().flex_none().text_color(color).child(note.clone()))
                    .into_any_element()
            }
            LogRow::TurnEnd { duration_ms, tools, cost_usd, is_error, files } => {
                self.render_turn_end(ix, *duration_ms, *tools, *cost_usd, *is_error, files, &t, cx)
            }
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
                .child(sel.text(text.clone(), &t))
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
        sel: &Sel,
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
                    .rounded_md()
                    .bg(t.active)
                    .font_family(mono())
                    .text_size(px(11.))
                    .text_color(t.accent)
                    .cursor_pointer()
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
                    .rounded_md()
                    .bg(t.active)
                    .font_family(mono())
                    .text_size(px(11.))
                    .text_color(t.text_muted)
                    .child(lbl)
                    .into_any_element()
            }))
            .collect();

        div()
            .flex()
            .flex_col()
            .gap_2()
            .px_3()
            .py_2()
            .rounded_lg()
            .bg(t.element)
            .when(!thumbs.is_empty(), |d| d.child(div().flex().flex_wrap().gap_2().children(thumbs)))
            .when(!text.is_empty(), |d| match slash_command(text) {
                // A slash command: its name as a chip, then whatever it was given.
                Some((name, args)) => d.child(
                    div()
                        .flex()
                        .items_start()
                        .gap_2()
                        .text_sm()
                        .child(
                            div()
                                .flex_none()
                                .px_1p5()
                                .rounded_md()
                                .bg(t.active)
                                .font_family(mono())
                                .text_color(t.accent)
                                .child(format!("/{name}")),
                        )
                        .when(!args.is_empty(), |d| {
                            d.child(div().flex_1().min_w_0().child(sel.text(args.to_string(), t)))
                        }),
                ),
                None => d.child(div().text_sm().child(sel.text(text.to_string(), t))),
            })
            .when(!chips.is_empty(), |d| d.child(div().flex().flex_wrap().gap_1().children(chips)))
            .into_any_element()
    }

    /// "Thinking… 4s" while it streams; after, "Thought for 6s", which opens to
    /// the text when the CLI shared it.
    fn render_thought(
        &self,
        ix: usize,
        id: u64,
        text: &str,
        started_at: Option<std::time::Instant>,
        duration_ms: Option<u64>,
        expanded: bool,
        streaming: bool,
        sel: &Sel,
        t: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let label = if streaming {
            format!("Thinking… {}s", started_at.map_or(0, |s| s.elapsed().as_secs()))
        } else if let Some(ms) = duration_ms {
            format!("Thought for {}", format_duration(ms))
        } else {
            "Thought".into()
        };
        let label = div().text_xs().text_color(t.text_placeholder).child(label);
        // Nothing to open: the same 22 px line, without the chevron.
        let plain = |label: gpui::Div| div().flex().items_center().h(px(22.)).px_1().child(label).into_any_element();
        if streaming {
            return plain(label.italic());
        }
        if text.trim().is_empty() {
            return plain(label);
        }
        let head = disclosure_row(SharedString::from(format!("thought-{id}")), expanded, t).child(label).on_click(
            cx.listener(move |this, _, _, cx| {
                if let Some(LogRow::Thought { expanded: exp, .. }) = this.rows.get_mut(ix) {
                    *exp = !*exp;
                    this.remeasure_row(ix);
                    cx.notify();
                }
            }),
        );
        div()
            .flex()
            .flex_col()
            .child(head)
            .when(expanded, |d| {
                d.child(
                    div().pl_6().pr_1().pt_1().text_sm().text_color(t.text_muted).child(sel.text(text.to_string(), t)),
                )
            })
            .into_any_element()
    }

    /// The quiet rule that ends a turn: duration, tools, cost, a chip per edited file.
    fn render_turn_end(
        &self,
        ix: usize,
        duration_ms: u64,
        tools: usize,
        cost_usd: Option<f64>,
        is_error: bool,
        files: &[String],
        t: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        const MAX_CHIPS: usize = 6;
        let mut recap = vec![format_duration(duration_ms)];
        if tools > 0 {
            recap.push(format!("{tools} tool{}", if tools == 1 { "" } else { "s" }));
        }
        if let Some(c) = cost_usd {
            recap.push(format_cost(c));
        }
        let color = if is_error { t.error } else { t.text_placeholder };
        let chips: Vec<AnyElement> = files
            .iter()
            .take(MAX_CHIPS)
            .enumerate()
            .map(|(n, f)| {
                let open = f.clone();
                let name =
                    std::path::Path::new(f).file_name().map_or_else(|| f.clone(), |n| n.to_string_lossy().into());
                div()
                    .id(SharedString::from(format!("turn-file-{ix}-{n}")))
                    .flex_none()
                    .flex()
                    .items_center()
                    .h(px(18.))
                    .px_1p5()
                    .rounded_sm()
                    .bg(t.element)
                    .cursor_pointer()
                    .hover(|s| s.bg(t.hover).text_color(t.text))
                    .font_family(mono())
                    .text_size(px(11.))
                    .text_color(t.text_muted)
                    .child(name)
                    .tooltip(Tooltip::text(short_path(&self.config.workspace, f)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let full = resolve_workspace_path(&this.config.workspace, &open);
                        cx.emit(ClaudeChatEvent::OpenFile { path: full, line: None });
                    }))
                    .into_any_element()
            })
            .collect();
        let more = files.len().saturating_sub(MAX_CHIPS);
        let rule = || div().flex_1().min_w(px(16.)).h(px(1.)).bg(t.border_variant);
        div()
            .w_full()
            .flex()
            .items_center()
            .gap_2()
            .pt_1()
            .text_xs()
            .child(rule())
            .when(is_error, |d| {
                d.child(div().flex_none().font_weight(FontWeight::MEDIUM).text_color(color).child("Stopped"))
            })
            .child(div().flex_none().text_color(color).child(recap.join(" · ")))
            .when(!chips.is_empty(), |d| {
                d.child(
                    div().flex().flex_wrap().gap_1().children(chips).when(more > 0, |d| {
                        d.child(div().text_color(t.text_placeholder).child(format!("+{more} more")))
                    }),
                )
            })
            .child(rule())
            .into_any_element()
    }
}

/// How row `row` sits in the list: `(compact, wide)`. Tool traffic sits tight
/// together and may use the wider column; messages get room to breathe.
pub(super) fn row_spacing(row: &LogRow) -> (bool, bool) {
    match row {
        LogRow::EditRow { .. } | LogRow::ToolCall { .. } => (true, true),
        LogRow::Thought { .. } | LogRow::PermissionNote { .. } | LogRow::TurnEnd { .. } => (true, false),
        _ => (false, false),
    }
}
