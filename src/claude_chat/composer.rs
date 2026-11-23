//! Composer UI & prompt submission:
//! - Multiline `TextInput` with `.intercept_long_paste()` (`PasteFold` atomic chips).
//! - First-class image pasting (`Ctrl+V` `PasteImages`), drag-drop (`ExternalPaths`),
//!   and `+` Attach button (`cx.prompt_for_paths`), with thumbnail chips + lightbox.
//! - Prompt suggestion as ghost text inside the empty composer (`Tab` to accept).
//! - Dynamic toolbar: Model picker, Effort/Thinking picker, Permission Mode (`Shift+Tab`),
//!   Fast Mode toggle, honest Context % + `$` Cost pill, `Steer` (`Alt+Enter`),
//!   `Stop` (`Esc`), `Send` / `Queue` (`Enter`), plus queued-message strip.

use std::path::PathBuf;

use gpui::{
    AnyElement, Context, ExternalPaths, Focusable, ObjectFit, PathPromptOptions, SharedString, Window, div, img,
    prelude::*,
};
use kynex_core::agent_panes::AgentState;
use kynex_core::claude::PromptBlock;

use super::composer_popups::active_model_supported_efforts;
use super::media;
use super::state::*;
use super::{ClaudeChatView, centered_column};
use crate::theme::{mono, theme, ui_font};
use crate::ui::input::Mark;
use crate::ui::{Icon, Tooltip, ghost_button, icon, icon_button, outline_button, primary_button, px, scrollbar};

/// The composer's gray hint while empty and no suggestion is pending.
pub(super) const COMPOSER_PLACEHOLDER: &str =
    "Message Claude… (Enter to send, Shift+Enter for newline, / commands, @ files)";

impl ClaudeChatView {
    /// Renders queued messages waiting for the current turn to complete.
    pub(super) fn render_queued_strip(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.queued.is_empty() {
            return None;
        }
        let t = theme(cx).clone();
        let rows: Vec<AnyElement> = self
            .queued
            .iter()
            .enumerate()
            .map(|(i, q)| {
                let qid = q.id;
                let preview = truncate_chars(q.display_text.lines().next().unwrap_or("(image)"), 72);
                div()
                    .id(("queued-row", i))
                    .flex()
                    .items_center()
                    .gap_2()
                    .h(px(24.))
                    .px_2()
                    .rounded_sm()
                    .bg(t.element)
                    .child(div().flex_none().text_size(px(10.5)).text_color(t.text_placeholder).child("Queued"))
                    .child(div().flex_1().min_w_0().truncate().text_xs().text_color(t.text).child(preview))
                    .child(
                        ghost_button(("queued-steer", i), cx)
                            .h(px(18.))
                            .px_1p5()
                            .text_size(px(11.))
                            .text_color(t.accent)
                            .tooltip(Tooltip::text("Send now as mid-turn steer"))
                            .child("Steer now")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.steer_queued_now(qid, cx);
                            })),
                    )
                    .child(
                        icon_button(("queued-rm", i), Icon::Close, cx)
                            .size(px(18.))
                            .tooltip(Tooltip::text("Remove from queue"))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.queued.retain(|m| m.id != qid);
                                cx.notify();
                            })),
                    )
                    .into_any_element()
            })
            .collect();

        Some(
            div()
                .flex_none()
                .px_4()
                .py_1p5()
                .border_t_1()
                .border_color(t.border_variant)
                .bg(t.panel)
                .child(centered_column(div().flex().flex_col().gap_1().children(rows)))
                .into_any_element(),
        )
    }

    /// Shows the prompt suggestion as gray ghost text inside the empty composer
    /// (`Tab` accepts it) in place of the usual placeholder.
    pub(super) fn sync_composer_ghost(&self, cx: &mut Context<Self>) {
        let ghost: SharedString = match &self.prompt_suggestion {
            Some(sug) if !self.turn_active && self.attachments.is_empty() => format!("{sug}   ⇥ Tab").into(),
            _ => COMPOSER_PLACEHOLDER.into(),
        };
        if self.composer_input.read(cx).placeholder_text() != &ghost {
            self.composer_input.update(cx, |inp, _| inp.set_placeholder(ghost));
        }
    }

    /// Renders the docked composer card (attachments, multiline TextInput, dynamic toolbar).
    pub(super) fn render_composer(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = theme(cx).clone();
        let accent = t.accent;
        let has_content = !self.composer_input.read(cx).text().trim().is_empty() || !self.attachments.is_empty();
        let input_scroll = self.composer_input.read(cx).scroll_handle().clone();

        // Attachment thumbnail chips inside composer
        let attach_chips: Vec<AnyElement> = self
            .attachments
            .iter()
            .enumerate()
            .map(|(i, att)| {
                let att_id = att.id;
                let thumb = att.thumb.clone();
                let full = att.full.clone().unwrap_or_else(|| att.thumb.clone());
                let size = att.size;
                let label = att.label.clone();
                div()
                    .id(("comp-att", i))
                    .relative()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .p_1()
                    .pr_1p5()
                    .rounded_md()
                    .border_1()
                    .border_color(t.border)
                    .bg(t.element)
                    .child(
                        div()
                            .id(("comp-att-thumb", i))
                            .w(px(36.))
                            .h(px(28.))
                            .rounded_sm()
                            .overflow_hidden()
                            .cursor_pointer()
                            .child(img(thumb).size_full().object_fit(ObjectFit::Cover))
                            .tooltip(Tooltip::text("Click to zoom"))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.open_lightbox(full.clone(), size, label.clone(), false, cx);
                            })),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .max_w(px(110.))
                                    .truncate()
                                    .text_size(px(11.))
                                    .text_color(t.text)
                                    .child(att.label.clone()),
                            )
                            .child(
                                div()
                                    .font_family(mono())
                                    .text_size(px(10.))
                                    .text_color(t.text_placeholder)
                                    .child(format!("{}×{}", size.0, size.1)),
                            ),
                    )
                    .child(
                        icon_button(("comp-att-rm", i), Icon::Close, cx)
                            .size(px(18.))
                            .tooltip(Tooltip::text("Remove image"))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.remove_attachment(att_id, cx);
                            })),
                    )
                    .into_any_element()
            })
            .collect();

        // Toolbar controls
        let model_label = self.active_model_display_name();
        let efforts = active_model_supported_efforts(self);
        let supports_fast = self.active_model_supports_fast();
        let mode_label = self.permission_mode.short_label();
        let mode_color = match self.permission_mode {
            kynex_core::claude::PermissionMode::Plan => t.info,
            kynex_core::claude::PermissionMode::Auto | kynex_core::claude::PermissionMode::AcceptEdits => t.success,
            kynex_core::claude::PermissionMode::BypassPermissions => t.warning,
            _ => t.text_muted,
        };

        let ctx_pill = self.render_context_cost_pill(&t, cx);

        div().flex_none().px_4().pb_3().pt_1().bg(t.editor).child(centered_column(
            div()
                .flex()
                .flex_col()
                .rounded_lg()
                .border_1()
                .border_color(t.border)
                .bg(t.panel)
                .drag_over::<ExternalPaths>(move |s, _, _, _| s.border_color(accent))
                .on_drop(cx.listener(Self::on_drop_external_paths))
                .when(!attach_chips.is_empty(), |d| {
                    d.child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_1p5()
                            .px_2p5()
                            .pt_2()
                            .pb_1()
                            .border_b_1()
                            .border_color(t.border_variant)
                            .children(attach_chips),
                    )
                })
                .child(scrollbar::vertical_over(
                    "claude-composer-sb",
                    input_scroll.clone(),
                    div()
                        .id("claude-composer-input-wrap")
                        .w_full()
                        .min_h(px(40.))
                        .max_h(px(160.))
                        .overflow_y_scroll()
                        .track_scroll(&input_scroll)
                        .px_3()
                        .py_2()
                        .font_family(ui_font())
                        .text_size(px(13.5))
                        .text_color(t.text)
                        .child(self.composer_input.clone()),
                    |w| w.w_full(),
                ))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .px_2()
                        .py_1p5()
                        .border_t_1()
                        .border_color(t.border_variant)
                        // Left toolbar cluster
                        .child(
                            icon_button("comp-attach", Icon::Plus, cx)
                                .tooltip(Tooltip::text("Attach image or file"))
                                .on_click(cx.listener(|this, _, _, cx| this.prompt_attach_files(cx))),
                        )
                        .child(
                            ghost_button("comp-model", cx)
                                .h(px(22.))
                                .px_1p5()
                                .text_xs()
                                .text_color(t.text_muted)
                                .tooltip(Tooltip::text("Switch model"))
                                .child(div().max_w(px(150.)).truncate().child(model_label))
                                .child(icon(Icon::ChevronDown, t.text_placeholder).size(px(11.)))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.toggle_popover(OpenPopover::Model, cx);
                                })),
                        )
                        .when(!efforts.is_empty(), |d| {
                            let eff_lbl = self.effort.label();
                            d.child(
                                ghost_button("comp-effort", cx)
                                    .h(px(22.))
                                    .px_1p5()
                                    .text_xs()
                                    .text_color(t.text_muted)
                                    .tooltip(Tooltip::text("Reasoning effort"))
                                    .child(eff_lbl)
                                    .child(icon(Icon::ChevronDown, t.text_placeholder).size(px(11.)))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.toggle_popover(OpenPopover::Effort, cx);
                                    })),
                            )
                        })
                        .child(
                            ghost_button("comp-mode", cx)
                                .h(px(22.))
                                .px_1p5()
                                .text_xs()
                                .text_color(mode_color)
                                .tooltip(Tooltip::with_meta("Permission mode", "Shift+Tab"))
                                .child(icon(Icon::Shield, mode_color).size(px(11.)))
                                .child(mode_label)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.toggle_popover(OpenPopover::Mode, cx);
                                })),
                        )
                        .when(supports_fast, |d| {
                            let on = self.fast_mode;
                            d.child(
                                ghost_button("comp-fast", cx)
                                    .h(px(22.))
                                    .px_1p5()
                                    .text_xs()
                                    .text_color(if on { t.warning } else { t.text_placeholder })
                                    .tooltip(Tooltip::text(if on { "Fast mode on" } else { "Enable fast mode" }))
                                    .child("Fast")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.toggle_fast_mode(cx);
                                    })),
                            )
                        })
                        .child(div().flex_1())
                        // Right toolbar cluster
                        .children(ctx_pill)
                        .when(self.turn_active, |d| {
                            d.when(has_content, |d| {
                                d.child(
                                    outline_button("comp-steer", "Steer", cx)
                                        .h(px(22.))
                                        .px_2()
                                        .text_xs()
                                        .tooltip(Tooltip::with_meta("Steer Claude mid-turn", "Alt+Enter"))
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.submit_composer(true, window, cx);
                                        })),
                                )
                            })
                            .child(
                                outline_button("comp-stop", "Stop", cx)
                                    .h(px(22.))
                                    .px_2()
                                    .text_xs()
                                    .text_color(t.error)
                                    .tooltip(Tooltip::with_meta("Interrupt current turn", "Esc"))
                                    .on_click(cx.listener(|this, _, _, cx| this.cancel_turn(cx))),
                            )
                            .when(has_content, |d| {
                                d.child(
                                    primary_button("comp-queue", "Queue", cx)
                                        .h(px(22.))
                                        .px_2()
                                        .text_xs()
                                        .tooltip(Tooltip::with_meta("Queue after current turn", "Enter"))
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.submit_composer(false, window, cx);
                                        })),
                                )
                            })
                        })
                        .when(!self.turn_active, |d| {
                            d.child(
                                primary_button("comp-send", "Send", cx)
                                    .h(px(22.))
                                    .px_2p5()
                                    .text_xs()
                                    .when(!has_content, |b| b.opacity(0.45))
                                    .tooltip(Tooltip::with_meta("Send message", "Enter"))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.submit_composer(false, window, cx);
                                    })),
                            )
                        }),
                ),
        ))
    }

    fn render_context_cost_pill(&self, t: &crate::theme::Palette, _cx: &Context<Self>) -> Option<AnyElement> {
        let show_ctx = self.context_used > 0;
        let show_cost = !self.is_openrouter() && self.cumulative_cost_usd > 0.0;
        if !show_ctx && !show_cost {
            return None;
        }
        let mut parts = Vec::new();
        let mut pct_color = t.text_muted;
        if show_ctx {
            if let Some(win) = self.context_window.filter(|&w| w > 0) {
                let pct = ((self.context_used as f64 / win as f64) * 100.0).clamp(0.0, 100.0);
                if pct >= 85.0 {
                    pct_color = t.error;
                } else if pct >= 65.0 {
                    pct_color = t.warning;
                }
                parts.push(format!("{pct:.0}%"));
            } else {
                parts.push(format!("{} tok", format_tokens(self.context_used)));
            }
        }
        if show_cost {
            parts.push(format_cost(self.cumulative_cost_usd));
        }
        let label: SharedString = parts.join(" · ").into();
        let u = self.last_usage;
        let win_str = self.context_window.map(format_tokens).unwrap_or_else(|| "?".into());
        let tooltip_meta = format!(
            "Context: {} / {win_str} · in {} · cache r {} / w {} · out {}",
            format_tokens(self.context_used),
            format_tokens(u.input_tokens),
            format_tokens(u.cache_read_input_tokens),
            format_tokens(u.cache_creation_input_tokens),
            format_tokens(u.output_tokens),
        );

        Some(
            div()
                .id("claude-ctx-cost-pill")
                .px_1p5()
                .h(px(22.))
                .flex()
                .items_center()
                .rounded_sm()
                .font_family(mono())
                .text_size(px(11.))
                .text_color(pct_color)
                .tooltip(Tooltip::with_meta("Session usage", tooltip_meta))
                .child(label)
                .into_any_element(),
        )
    }

    // -----------------------------------------------------------------------
    // Composer actions: paste, drop, attach, marks, submit, steer, queue
    // -----------------------------------------------------------------------

    pub(super) fn sync_composer_marks(&mut self, cx: &mut Context<Self>) {
        let t = theme(cx).clone();
        let text = self.composer_input.read(cx).text().to_string();
        self.paste_fold.prune_unused(&text);

        let mut marks = Vec::new();
        // Atomic marks for `[Pasted text #N +M lines]` chips.
        for block in self.paste_fold.blocks() {
            let mut offset = 0usize;
            while let Some(rel) = text[offset..].find(&block.label) {
                let start = offset + rel;
                let end = start + block.label.len();
                marks.push(Mark { range: start..end, color: t.accent, background: Some(t.selection), atomic: true });
                offset = end;
            }
        }
        // Highlight `@path` mentions (non-atomic so user can edit freely).
        let mut pos = 0usize;
        for part in text.split_inclusive(char::is_whitespace) {
            let trimmed = part.trim_end_matches(char::is_whitespace);
            if let Some(rest) = trimmed.strip_prefix('@')
                && !rest.is_empty()
                && (rest.contains('/') || rest.contains('.'))
            {
                marks.push(Mark {
                    range: pos..pos + trimmed.len(),
                    color: t.accent,
                    background: Some(t.element),
                    atomic: false,
                });
            }
            pos += part.len();
        }
        marks.sort_by_key(|m| m.range.start);
        self.composer_input.update(cx, |inp, cx| inp.set_marks(marks, cx));
    }

    pub(super) fn on_long_paste(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        let block = self.paste_fold.fold_always(text);
        let range = self.composer_input.read(cx).selected_range();
        self.composer_input.update(cx, |inp, cx| {
            inp.replace_range(range, &block.label, window, cx);
        });
        self.sync_composer_marks(cx);
        cx.notify();
    }

    pub(super) fn on_composer_mark_clicked(
        &mut self,
        range: std::ops::Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = self.composer_input.read(cx).text().to_string();
        let Some(label) = text.get(range.clone()).map(str::to_string) else { return };
        if let Some(expanded) = self.paste_fold.expand_one(&text, &label) {
            self.composer_input.update(cx, |inp, cx| inp.set_text(expanded, cx));
            self.sync_composer_marks(cx);
            self.composer_input.read(cx).focus_handle(cx).focus(window, cx);
            cx.notify();
        }
    }

    pub(super) fn on_paste_images(&mut self, images: Vec<gpui::Image>, cx: &mut Context<Self>) {
        let base_idx = self.attachments.len() + 1;
        for (offset, img) in images.into_iter().enumerate() {
            let idx = base_idx + offset;
            cx.spawn(async move |this, cx| {
                let res = cx.background_spawn(async move { media::decode_clipboard_image(&img, idx) }).await;
                this.update(cx, |this, cx| match res {
                    Ok(att) => this.push_decoded_attachment(att, cx),
                    Err(e) => this.push_notice(e, true, cx),
                })
                .ok();
            })
            .detach();
        }
    }

    pub(super) fn on_drop_external_paths(
        &mut self,
        paths: &ExternalPaths,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.composer_input.read(cx).focus_handle(cx).focus(window, cx);
        self.ingest_external_paths(paths.paths().to_vec(), window, cx);
    }

    pub(super) fn prompt_attach_files(&mut self, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Attach images or files".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = picked.await {
                this.update(cx, |this, cx| {
                    this.ingest_paths_no_window(paths, cx);
                })
                .ok();
            }
        })
        .detach();
    }

    fn ingest_external_paths(&mut self, paths: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        let mut non_images = Vec::new();
        for path in paths {
            if media::is_supported_image_path(&path) {
                self.spawn_decode_image_path(path, cx);
            } else {
                let display = short_path(&self.config.workspace, &path.to_string_lossy());
                non_images.push(format!("@{display}"));
            }
        }
        if !non_images.is_empty() {
            let insertion = format!("{} ", non_images.join(" "));
            let range = self.composer_input.read(cx).selected_range();
            self.composer_input.update(cx, |inp, cx| inp.replace_range(range, &insertion, window, cx));
            self.sync_composer_marks(cx);
        }
        cx.notify();
    }

    fn ingest_paths_no_window(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let mut non_images = Vec::new();
        for path in paths {
            if media::is_supported_image_path(&path) {
                self.spawn_decode_image_path(path, cx);
            } else {
                let display = short_path(&self.config.workspace, &path.to_string_lossy());
                non_images.push(format!("@{display}"));
            }
        }
        if !non_images.is_empty() {
            let cur = self.composer_input.read(cx).text().to_string();
            let sep = if cur.is_empty() || cur.ends_with(char::is_whitespace) { "" } else { " " };
            let next = format!("{cur}{sep}{} ", non_images.join(" "));
            self.composer_input.update(cx, |inp, cx| inp.set_text(next, cx));
            self.sync_composer_marks(cx);
        }
        cx.notify();
    }

    fn spawn_decode_image_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let res = cx.background_spawn(async move { media::decode_attachment_from_path(&path) }).await;
            this.update(cx, |this, cx| match res {
                Ok(att) => this.push_decoded_attachment(att, cx),
                Err(e) => this.push_notice(e, true, cx),
            })
            .ok();
        })
        .detach();
    }

    fn push_decoded_attachment(&mut self, att: media::DecodedAttachment, cx: &mut Context<Self>) {
        let id = self.next_id();
        self.attachments.push(ComposerAttachment {
            id,
            media_type: att.media_type,
            base64: att.base64,
            size: att.size,
            thumb: att.thumb,
            full: att.full,
            label: att.label,
        });
        cx.notify();
    }

    pub(super) fn remove_attachment(&mut self, id: u64, cx: &mut Context<Self>) {
        if let Some(pos) = self.attachments.iter().position(|a| a.id == id) {
            let removed = self.attachments.remove(pos);
            cx.drop_image(removed.thumb, None);
            if let Some(full) = removed.full {
                cx.drop_image(full, None);
            }
            cx.notify();
        }
    }

    /// Submits the current composer contents (normal send, queue when busy, or mid-turn steer).
    pub(super) fn submit_composer(&mut self, steer: bool, _window: &mut Window, cx: &mut Context<Self>) {
        let raw_text = self.composer_input.read(cx).text().to_string();
        let trimmed = raw_text.trim();
        if trimmed.is_empty() && self.attachments.is_empty() {
            return;
        }

        // Local Kynex slash commands handled directly without sending to the CLI.
        if self.attachments.is_empty() {
            if trimmed == "/fork" {
                self.composer_input.update(cx, |inp, cx| inp.set_text("", cx));
                self.paste_fold.clear();
                self.completion = None;
                self.request_fork(cx);
                return;
            }
            if trimmed == "/clear" {
                self.composer_input.update(cx, |inp, cx| inp.set_text("", cx));
                self.paste_fold.clear();
                self.completion = None;
                self.clear_transcript(cx);
                return;
            }
        }

        let paste_labels: Vec<String> =
            self.paste_fold.blocks().iter().filter(|b| raw_text.contains(&b.label)).map(|b| b.label.clone()).collect();
        let expanded = self.paste_fold.expand(trimmed);
        let file_chips = extract_at_chips(&expanded);

        // Move attachments into ImageCache so their textures survive inside the transcript row.
        let mut blocks: Vec<PromptBlock> = Vec::new();
        let mut row_images: Vec<RowImage> = Vec::new();
        let attachments = std::mem::take(&mut self.attachments);
        for att in attachments {
            blocks.push(PromptBlock::Image { media_type: att.media_type, base64: att.base64 });
            self.image_cache.insert_inline(
                att.id,
                media::CachedImage { thumb: att.thumb, full: att.full, size: att.size, label: att.label.clone() },
            );
            row_images.push(RowImage { key: ImageKey::Inline(att.id), logged_ref: None, label: att.label });
        }
        if !expanded.is_empty() {
            blocks.push(PromptBlock::Text(expanded.clone()));
        }

        // Clear composer state immediately.
        self.composer_input.update(cx, |inp, cx| inp.set_text("", cx));
        self.paste_fold.clear();
        self.sync_composer_marks(cx);
        self.completion = None;
        self.open_popover = None;
        self.prompt_suggestion = None;

        // If a turn is running and user didn't request Steer, enqueue cleanly.
        if self.turn_active && !steer {
            let qid = self.next_id();
            self.queued.push_back(QueuedMessage {
                id: qid,
                display_text: trimmed.to_string(),
                blocks,
                file_chips,
                paste_labels,
                row_images,
            });
            cx.notify();
            return;
        }

        self.send_blocks_now(trimmed.to_string(), blocks, file_chips, paste_labels, row_images, steer, cx);
    }

    pub(super) fn dispatch_queued(&mut self, msg: QueuedMessage, cx: &mut Context<Self>) {
        self.send_blocks_now(msg.display_text, msg.blocks, msg.file_chips, msg.paste_labels, msg.row_images, false, cx);
    }

    fn steer_queued_now(&mut self, qid: u64, cx: &mut Context<Self>) {
        if let Some(pos) = self.queued.iter().position(|m| m.id == qid)
            && let Some(msg) = self.queued.remove(pos)
        {
            self.send_blocks_now(
                msg.display_text,
                msg.blocks,
                msg.file_chips,
                msg.paste_labels,
                msg.row_images,
                true,
                cx,
            );
        }
    }

    pub(super) fn send_blocks_now(
        &mut self,
        display_text: String,
        blocks: Vec<PromptBlock>,
        file_chips: Vec<String>,
        paste_labels: Vec<String>,
        row_images: Vec<RowImage>,
        steer: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(client) = &self.client else {
            self.push_notice("Claude is not running.", true, cx);
            return;
        };
        let res = if steer { client.steer(blocks) } else { client.send_prompt(blocks) };
        if let Err(e) = res {
            self.push_notice(format!("Couldn't send message: {e}"), true, cx);
            return;
        }
        let id = self.next_id();
        self.push_row(LogRow::UserPrompt {
            id,
            uuid: None,
            text: display_text,
            images: row_images,
            file_chips,
            paste_labels,
        });
        self.update_title_from_first_prompt(cx);
        self.turn_active = true;
        self.scroll_to_bottom();
        self.set_status(AgentState::Working, if steer { "Steering…" } else { "Thinking…" }, cx);
        cx.notify();
    }
}
