//! Overlays for `ClaudeChatView`:
//! - Live Subagent Strip above the composer (`[●] N subagents · <label> [Peek ↗] [Stop]`).
//! - Read-only Subagent Peek Modal showing live forwarded thinking, tool calls, and text.
//! - Full-resolution Image Lightbox Modal (`Esc` or backdrop click to close).

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{
    AnyElement, App, AppContext, Context, FontWeight, MouseButton, ObjectFit, RenderImage, SharedString, div, img,
    prelude::*,
};

use super::markdown_view;
use super::media;
use super::state::{LightboxImage, SubagentRow};
use super::{ClaudeChatView, centered_column};
use crate::theme::{mono, theme, ui_font};
use crate::ui::{Icon, Lamp, Tooltip, ghost_button, icon, icon_button, lamp, px, scrollbar};

impl ClaudeChatView {
    /// Pinned 1-line Plan / Todo strip above cards & composer.
    pub(super) fn render_plan_strip(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.plan_steps.is_empty() {
            return None;
        }
        let total = self.plan_steps.len();
        let done = self.plan_steps.iter().filter(|s| s.status == "completed").count();
        // A finished checklist has nothing left to track: drop it once the turn ends.
        if done == total && !self.turn_active {
            return None;
        }
        let t = theme(cx).clone();
        let current = self
            .plan_steps
            .iter()
            .find(|s| s.status == "in_progress")
            .or_else(|| self.plan_steps.iter().find(|s| s.status != "completed"))
            .or_else(|| self.plan_steps.last());
        let current_text = current
            .map(|s| s.active_form.as_deref().filter(|a| !a.is_empty()).unwrap_or(&s.content))
            .unwrap_or("All steps complete");
        let step_num = (done + usize::from(done < total)).min(total);
        let chev = if self.plan_expanded { Icon::ChevronDown } else { Icon::ChevronRight };

        let checklist: Vec<AnyElement> = if self.plan_expanded {
            self.plan_steps
                .iter()
                .map(|step| {
                    let (glyph, color) = match step.status.as_str() {
                        "completed" => ("✓", t.success),
                        "in_progress" => ("●", t.accent),
                        _ => ("○", t.text_placeholder),
                    };
                    div()
                        .flex()
                        .items_start()
                        .gap_2()
                        .text_xs()
                        .child(div().w(px(12.)).flex_none().text_color(color).child(glyph))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_color(if step.status == "completed" { t.text_muted } else { t.text })
                                .child(step.content.clone()),
                        )
                        .into_any_element()
                })
                .collect()
        } else {
            Vec::new()
        };

        Some(
            div()
                .flex()
                .flex_none()
                .flex_col()
                .border_t_1()
                .border_color(t.border_variant)
                .bg(t.panel)
                .child(
                    div()
                        .id("claude-plan-strip-toggle")
                        .px_4()
                        .h(px(28.))
                        .flex()
                        .items_center()
                        .cursor_pointer()
                        .hover(|s| s.bg(t.hover))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.plan_expanded = !this.plan_expanded;
                            cx.notify();
                        }))
                        .child(centered_column(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .flex_none()
                                        .font_family(mono())
                                        .text_xs()
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(if done == total { t.success } else { t.accent })
                                        .child(format!("Step {step_num}/{total}")),
                                )
                                .child(div().text_xs().text_color(t.text_placeholder).child("·"))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .text_xs()
                                        .text_color(t.text)
                                        .child(current_text.to_string()),
                                )
                                .child(icon(chev, t.text_muted).size(px(12.))),
                        )),
                )
                .when(self.plan_expanded, |d| {
                    d.child(
                        div()
                            .px_4()
                            .pb_2()
                            .pt_1()
                            .border_t_1()
                            .border_color(t.border_variant)
                            .child(centered_column(div().flex().flex_col().gap_1().children(checklist))),
                    )
                })
                .into_any_element(),
        )
    }

    /// Quiet 1-line strip listing active or recent subagents above the composer.
    pub(super) fn render_subagent_strip(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        // Only live subagents get the strip; finished ones stay reachable from their
        // Agent/Task row's Peek button in the transcript.
        let active_count = self.subagents.iter().filter(|s| s.running).count();
        if active_count == 0 {
            return None;
        }
        let t = theme(cx).clone();
        let primary = self.subagents.iter().rev().find(|s| s.running)?;
        let primary_id = primary.tool_use_id.clone();
        let stop_task_id = primary.task_id.clone();

        let count_label = format!("{active_count} subagent{}", if active_count == 1 { "" } else { "s" });
        let detail = match &primary.last_tool {
            Some(tool) => format!("{} ({tool})", primary.label),
            None => primary.label.clone(),
        };

        let lamp_state = Lamp::Working;

        Some(
            div()
                .flex()
                .flex_none()
                .items_center()
                .px_4()
                .h(px(28.))
                .border_t_1()
                .border_color(t.border_variant)
                .bg(t.panel)
                .child(centered_column(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(lamp("subagent-strip-lamp", lamp_state, cx))
                        .child(
                            div()
                                .flex_none()
                                .font_weight(FontWeight::MEDIUM)
                                .text_xs()
                                .text_color(t.text)
                                .child(count_label),
                        )
                        .child(div().text_xs().text_color(t.text_placeholder).child("·"))
                        .child(div().flex_1().min_w_0().truncate().text_xs().text_color(t.text_muted).child(detail))
                        .child(
                            ghost_button("subagent-peek-btn", cx)
                                .h(px(20.))
                                .px_1p5()
                                .text_xs()
                                .text_color(t.accent)
                                .tooltip(Tooltip::text("Inspect live subagent output"))
                                .child("Peek ↗")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.peek_subagent = Some(primary_id.clone());
                                    cx.notify();
                                })),
                        )
                        .when_some(stop_task_id, |d, tid| {
                            d.child(
                                ghost_button("subagent-stop-btn", cx)
                                    .h(px(20.))
                                    .px_1p5()
                                    .text_xs()
                                    .text_color(t.error)
                                    .tooltip(Tooltip::text("Stop subagent task"))
                                    .child(div().size(px(7.)).rounded_xs().bg(t.error).flex_none())
                                    .child("Stop")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.stop_subagent_task(&tid, cx);
                                    })),
                            )
                        }),
                ))
                .into_any_element(),
        )
    }

    /// Read-only modal overlay inspecting one subagent's live thinking, tools, and prose.
    pub(super) fn render_subagent_peek_modal(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let active_id = self.peek_subagent.as_deref()?;
        let track = self.subagents.iter().find(|s| s.tool_use_id == active_id)?;
        let t = theme(cx).clone();

        let tabs: Vec<AnyElement> = if self.subagents.len() > 1 {
            self.subagents
                .iter()
                .enumerate()
                .map(|(i, s)| {
                    let is_sel = s.tool_use_id == active_id;
                    let sid = s.tool_use_id.clone();
                    let st = if s.running { Lamp::Working } else { Lamp::Done };
                    div()
                        .id(("subagent-tab", i))
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .px_2()
                        .h(px(24.))
                        .rounded_md()
                        .text_xs()
                        .cursor_pointer()
                        .when(is_sel, |d| d.bg(t.active).text_color(t.text).font_weight(FontWeight::MEDIUM))
                        .when(!is_sel, |d| d.text_color(t.text_muted).hover(|s| s.bg(t.hover)))
                        .child(lamp(("sub-tab-lamp", i), st, cx))
                        .child(div().max_w(px(160.)).truncate().child(s.label.clone()))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.peek_subagent = Some(sid.clone());
                            cx.notify();
                        }))
                        .into_any_element()
                })
                .collect()
        } else {
            Vec::new()
        };

        // Long subagent runs can forward thousands of rows; the modal isn't virtualized,
        // so show only the latest ones.
        const PEEK_MAX_ROWS: usize = 200;
        let hidden = track.rows.len().saturating_sub(PEEK_MAX_ROWS);
        let body_items: Vec<AnyElement> = if track.rows.is_empty() {
            vec![
                div()
                    .py_6()
                    .text_center()
                    .text_xs()
                    .text_color(t.text_placeholder)
                    .child(if track.running {
                        "Waiting for subagent output…".to_string()
                    } else {
                        track.summary.clone().unwrap_or_else(|| "No forwarded subagent messages.".into())
                    })
                    .into_any_element(),
            ]
        } else {
            let earlier = (hidden > 0).then(|| {
                div()
                    .text_center()
                    .text_xs()
                    .text_color(t.text_placeholder)
                    .child(format!("{hidden} earlier rows not shown"))
                    .into_any_element()
            });
            earlier
                .into_iter()
                .chain(track.rows.iter().enumerate().skip(hidden).map(|(i, row)| {
                    match row {
                        SubagentRow::Thought(text) => div()
                            .px_2p5()
                            .py_1p5()
                            .rounded_md()
                            .bg(t.panel)
                            .border_1()
                            .border_color(t.border_variant)
                            .font_family(mono())
                            .text_size(px(11.5))
                            .text_color(t.text_muted)
                            .child(text.clone())
                            .into_any_element(),
                        SubagentRow::Prose { blocks, .. } => {
                            let rendered =
                                markdown_view::render_blocks(blocks, 10_000 + i as u64, self.copied_key.as_deref(), cx);
                            div().flex().flex_col().gap_1p5().children(rendered).into_any_element()
                        }
                        SubagentRow::Tool { name, summary, output, is_error, .. } => {
                            let dot_color = if *is_error {
                                t.error
                            } else if output.is_some() {
                                t.success
                            } else {
                                t.accent
                            };
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .px_2p5()
                                .py_1p5()
                                .rounded_md()
                                .border_1()
                                .border_color(t.border_variant)
                                .bg(t.element)
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .child(div().size(px(6.)).rounded_full().bg(dot_color).flex_none())
                                        .child(
                                            div()
                                                .font_family(mono())
                                                .font_weight(FontWeight::MEDIUM)
                                                .text_xs()
                                                .text_color(t.text)
                                                .child(name.clone()),
                                        )
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .truncate()
                                                .font_family(mono())
                                                .text_xs()
                                                .text_color(t.text_muted)
                                                .child(summary.clone()),
                                        ),
                                )
                                .when_some(output.as_ref().filter(|o| !o.trim().is_empty()), |d, out| {
                                    let preview = super::state::truncate_chars(out.trim(), 800);
                                    d.child(
                                        div()
                                            .pt_1()
                                            .border_t_1()
                                            .border_color(t.border_variant)
                                            .font_family(mono())
                                            .text_size(px(11.))
                                            .text_color(if *is_error { t.error } else { t.text_muted })
                                            .child(preview),
                                    )
                                })
                                .into_any_element()
                        }
                    }
                }))
                .collect()
        };

        let scroll = scrollbar::handle("claude-subagent-peek-scroll");
        let stop_tid = track.running.then(|| track.task_id.clone()).flatten();
        let lamp_state = if track.running { Lamp::Working } else { Lamp::Done };

        Some(
            div()
                .id("subagent-peek-backdrop")
                .absolute()
                .inset_0()
                .bg(t.scrim())
                .flex()
                .items_center()
                .justify_center()
                .p_6()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.peek_subagent = None;
                        cx.notify();
                    }),
                )
                .child(
                    div()
                        .w_full()
                        .max_w(px(720.))
                        .max_h(px(560.))
                        .flex()
                        .flex_col()
                        .rounded_lg()
                        .border_1()
                        .border_color(t.border)
                        .bg(t.elevated)
                        .shadow_lg()
                        .overflow_hidden()
                        .font_family(ui_font())
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .px_3()
                                .h(px(36.))
                                .border_b_1()
                                .border_color(t.border_variant)
                                .bg(t.panel)
                                .child(lamp("subagent-modal-lamp", lamp_state, cx))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_sm()
                                        .text_color(t.text)
                                        .child(track.label.clone()),
                                )
                                .child(div().text_xs().text_color(t.text_muted).child(track.status.clone()))
                                .when_some(stop_tid, |d, tid| {
                                    d.child(
                                        ghost_button("subagent-modal-stop", cx)
                                            .h(px(22.))
                                            .text_xs()
                                            .text_color(t.error)
                                            .child(div().size(px(7.)).rounded_xs().bg(t.error).flex_none())
                                            .child("Stop")
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.stop_subagent_task(&tid, cx);
                                            })),
                                    )
                                })
                                .child(
                                    icon_button("subagent-modal-close", Icon::Close, cx)
                                        .tooltip(Tooltip::with_meta("Close", "Esc"))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.peek_subagent = None;
                                            cx.notify();
                                        })),
                                ),
                        )
                        .when(!tabs.is_empty(), |d| {
                            d.child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .px_2()
                                    .py_1()
                                    .border_b_1()
                                    .border_color(t.border_variant)
                                    .children(tabs),
                            )
                        })
                        .child(scrollbar::vertical_over(
                            "subagent-peek-sb",
                            scroll.clone(),
                            div()
                                .id("subagent-peek-body")
                                .flex_1()
                                .min_h_0()
                                .overflow_y_scroll()
                                .track_scroll(&scroll)
                                .p_3()
                                .flex()
                                .flex_col()
                                .gap_2()
                                .children(body_items),
                            |d| d.flex_1().min_h_0(),
                        )),
                )
                .into_any_element(),
        )
    }

    /// Full-resolution image lightbox modal (`Esc` or backdrop click to close).
    pub(super) fn render_lightbox_modal(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let lb = self.lightbox.as_ref()?;
        let t = theme(cx).clone();
        let dims: SharedString = format!("{} × {}", lb.size.0, lb.size.1).into();
        let label = lb.label.clone();
        let image = lb.image.clone();

        Some(
            div()
                .id("claude-lightbox-backdrop")
                .absolute()
                .inset_0()
                .bg(t.scrim())
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .p_6()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.close_lightbox(cx);
                    }),
                )
                .child(
                    div()
                        .size_full()
                        .max_w(px(960.))
                        .max_h(px(680.))
                        .flex()
                        .flex_col()
                        .rounded_lg()
                        .border_1()
                        .border_color(t.border)
                        .bg(t.elevated)
                        .shadow_lg()
                        .overflow_hidden()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .h(px(32.))
                                .px_3()
                                .border_b_1()
                                .border_color(t.border_variant)
                                .bg(t.panel)
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .text_xs()
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(t.text)
                                        .child(label),
                                )
                                .child(div().font_family(mono()).text_xs().text_color(t.text_muted).child(dims))
                                .child(
                                    icon_button("lightbox-close", Icon::Close, cx)
                                        .tooltip(Tooltip::with_meta("Close", "Esc"))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.close_lightbox(cx);
                                        })),
                                ),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_h_0()
                                .p_3()
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(img(image).size_full().object_fit(ObjectFit::ScaleDown)),
                        ),
                )
                .into_any_element(),
        )
    }

    /// Renders the `Ctrl+F` transcript find bar when open.
    pub(super) fn render_find_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.find.open {
            return None;
        }
        let t = theme(cx).clone();
        let count_text = if self.find.query.is_empty() {
            String::new()
        } else if self.find.matches.is_empty() {
            "No matches".into()
        } else {
            format!("{}/{}", self.find.current + 1, self.find.matches.len())
        };
        let has_matches = !self.find.matches.is_empty();

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
                .bg(t.panel)
                .child(icon(Icon::Search, t.text_muted).size(px(13.)))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .px_1p5()
                        .py_0p5()
                        .rounded_sm()
                        .border_1()
                        .border_color(t.border)
                        .bg(t.editor)
                        .text_xs()
                        .child(self.find_input.clone()),
                )
                .when(!count_text.is_empty(), |d| {
                    d.child(
                        div()
                            .flex_none()
                            .font_family(mono())
                            .text_xs()
                            .text_color(if has_matches { t.text_muted } else { t.error })
                            .child(count_text),
                    )
                })
                .child(
                    icon_button("find-prev", Icon::ArrowUp, cx)
                        .tooltip(Tooltip::text("Previous match"))
                        .on_click(cx.listener(|this, _, _, cx| this.step_find(-1, cx))),
                )
                .child(
                    icon_button("find-next", Icon::ArrowDown, cx)
                        .tooltip(Tooltip::text("Next match"))
                        .on_click(cx.listener(|this, _, _, cx| this.step_find(1, cx))),
                )
                .child(
                    icon_button("find-close", Icon::Close, cx)
                        .tooltip(Tooltip::with_meta("Close find", "Esc"))
                        .on_click(cx.listener(|this, _, window, cx| this.close_find(window, cx))),
                )
                .into_any_element(),
        )
    }

    pub(super) fn stop_subagent_task(&mut self, task_id: &str, cx: &mut Context<Self>) {
        if let Some(client) = &self.client {
            let _ = client.stop_task(task_id);
        }
        if let Some(track) = self.subagents.iter_mut().find(|s| s.task_id.as_deref() == Some(task_id)) {
            track.running = false;
            track.status = "Stopping…".into();
        }
        cx.notify();
    }

    pub(super) fn open_lightbox(
        &mut self,
        image: Arc<RenderImage>,
        size: (u32, u32),
        label: String,
        owned: bool,
        cx: &mut Context<Self>,
    ) {
        self.close_lightbox(cx);
        self.lightbox = Some(LightboxImage { image, size, label, owned });
        cx.notify();
    }

    pub(super) fn open_image_file_in_lightbox(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let decoded = cx.background_spawn(async move { media::decode_attachment_from_path(&path) }).await;
            this.update(cx, |this, cx| match decoded {
                Ok(att) => {
                    let full = att.full.unwrap_or_else(|| att.thumb.clone());
                    if !Arc::ptr_eq(&full, &att.thumb) {
                        cx.drop_image(att.thumb, None);
                    }
                    this.open_lightbox(full, att.size, att.label, true, cx);
                }
                Err(e) => this.push_notice(e, true, cx),
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn close_lightbox(&mut self, cx: &mut App) {
        if let Some(lb) = self.lightbox.take()
            && lb.owned
        {
            cx.drop_image(lb.image, None);
        }
    }
}
