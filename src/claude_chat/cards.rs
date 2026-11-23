//! Docked 1px amber interactive cards above the composer:
//! - Permission Card (`ToolApproval`) with `[1]` Allow once, `[2]` Allow for session,
//!   optional comment/feedback input, and `[Esc]` Deny.
//! - `AskUserQuestion` structured single/multi-select card with "Other" text input.
//! - `ExitPlanMode` scrollable markdown plan preview with Approve / Revise controls.
//! - Pinned 1-line Plan / Todo strip (`Step 3/6 · ... ▸`, expandable checklist).

use gpui::{AnyElement, Context, Focusable, FontWeight, Window, div, prelude::*};
use kynex_core::claude::diff::EditDiff;
use kynex_core::claude::{PermissionAnswer, PermissionMode};
use serde_json::{Map, Value};

use super::markdown_view::{self, render_inline_diff};
use super::state::*;
use super::{ClaudeChatView, centered_column};
use crate::theme::{mono, theme, ui_font};
use crate::ui::{Icon, Tooltip, ghost_button, icon, outline_button, primary_button, px, scrollbar};

impl ClaudeChatView {
    /// Renders the docked 1px amber interactive card when awaiting permission, questions, or plan approval.
    pub(super) fn render_pending_card(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let card = self.pending_card.clone()?;
        let inner = match card {
            PendingCard::Permission {
                req_id,
                tool_name,
                input,
                suggestions,
                why,
                blocked_path,
                description,
                diff_preview,
                show_comment,
            } => self.render_permission_card(
                req_id,
                tool_name,
                input,
                suggestions,
                why,
                blocked_path,
                description,
                diff_preview,
                show_comment,
                cx,
            ),
            PendingCard::AskUserQuestion { req_id, questions, active_idx, selected, other_active, other_text } => {
                self.render_ask_question_card(req_id, questions, active_idx, selected, other_active, other_text, cx)
            }
            PendingCard::ExitPlanMode { req_id, plan_blocks, plan_path, show_feedback } => {
                self.render_exit_plan_card(req_id, plan_blocks, plan_path, show_feedback, cx)
            }
        };
        Some(div().flex_none().px_4().py_2().child(centered_column(inner)).into_any_element())
    }

    fn render_permission_card(
        &self,
        req_id: String,
        tool_name: String,
        input: Value,
        suggestions: Vec<Value>,
        why: Option<String>,
        blocked_path: Option<String>,
        description: Option<String>,
        diff_preview: Option<EditDiff>,
        show_comment: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx).clone();
        let command = extract_command(&input);
        let file_path = extract_file_path(&input).or(blocked_path);
        let summary = description.unwrap_or_else(|| tool_summary(&self.config.workspace, &tool_name, &input));
        let has_session = !suggestions.is_empty();
        let queue_extra = self.queued_cards.len();

        let allow_once_id = req_id.clone();
        let allow_sess_id = req_id.clone();
        let deny_id = req_id.clone();
        let sess_suggestions = suggestions.clone();

        div()
            .flex()
            .flex_none()
            .flex_col()
            .gap_2()
            .p_3()
            .rounded_lg()
            .border_1()
            .border_color(t.warning)
            .bg(t.elevated)
            .font_family(ui_font())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(icon(Icon::Shield, t.warning).size(px(14.)))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_sm()
                            .text_color(t.text)
                            .child(format!("Allow {tool_name}?")),
                    )
                    .when(queue_extra > 0, |d| {
                        d.child(div().text_xs().text_color(t.text_muted).child(format!("(+{queue_extra} queued)")))
                    })
                    .child(div().flex_1())
                    .child(
                        ghost_button("perm-comment-toggle", cx)
                            .h(px(22.))
                            .text_xs()
                            .text_color(t.text_muted)
                            .child(if show_comment { "Hide feedback" } else { "Feedback…" })
                            .on_click(cx.listener(|this, _, window, cx| {
                                if let Some(PendingCard::Permission { show_comment: sc, .. }) = &mut this.pending_card {
                                    *sc = !*sc;
                                    if *sc {
                                        this.card_input.read(cx).focus_handle(cx).focus(window, cx);
                                    }
                                    cx.notify();
                                }
                            })),
                    ),
            )
            .when_some(why.filter(|w| !w.trim().is_empty()), |d, reason| {
                d.child(div().text_xs().text_color(t.text_muted).child(reason))
            })
            .child(if let Some(diff) = diff_preview.filter(|d| !d.is_empty()) {
                let p = file_path.as_deref().unwrap_or("edit");
                div()
                    .rounded_md()
                    .border_1()
                    .border_color(t.border_variant)
                    .overflow_hidden()
                    .child(render_inline_diff(p, &diff, None, 999_001, &t))
                    .into_any_element()
            } else {
                let body = command.map(|c| format!("$ {c}")).unwrap_or(summary);
                div()
                    .px_2p5()
                    .py_1p5()
                    .rounded_md()
                    .border_1()
                    .border_color(t.border_variant)
                    .bg(t.element)
                    .font_family(mono())
                    .text_xs()
                    .text_color(t.text)
                    .child(truncate_chars(&body, 600))
                    .into_any_element()
            })
            .when(show_comment, |d| {
                d.child(
                    div()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .border_1()
                        .border_color(t.border)
                        .bg(t.editor)
                        .text_xs()
                        .child(self.card_input.clone()),
                )
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        primary_button("perm-allow-once", "[1] Allow once", cx)
                            .tooltip(Tooltip::with_meta("Allow this tool call once", "1"))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.answer_permission(&allow_once_id, PermissionAnswer::AllowOnce, window, cx);
                            })),
                    )
                    .when(has_session, |d| {
                        d.child(
                            outline_button("perm-allow-session", "[2] Allow for session", cx)
                                .tooltip(Tooltip::with_meta("Allow similar calls for this session", "2"))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.answer_permission(
                                        &allow_sess_id,
                                        PermissionAnswer::AllowSession(sess_suggestions.clone()),
                                        window,
                                        cx,
                                    );
                                })),
                        )
                    })
                    .child(div().flex_1())
                    .child(
                        outline_button("perm-deny", "[Esc] Deny", cx)
                            .text_color(t.error)
                            .tooltip(Tooltip::with_meta("Deny (includes feedback if entered)", "Esc"))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                let note = this.take_card_input(cx);
                                this.answer_permission(
                                    &deny_id,
                                    PermissionAnswer::Deny { message: note, interrupt: false },
                                    window,
                                    cx,
                                );
                            })),
                    ),
            )
            .into_any_element()
    }

    fn render_ask_question_card(
        &self,
        req_id: String,
        questions: Vec<kynex_core::claude::AskQuestion>,
        active_idx: usize,
        selected: Vec<Vec<usize>>,
        other_active: Vec<bool>,
        other_text: Vec<String>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx).clone();
        let q_idx = active_idx.min(questions.len().saturating_sub(1));
        let Some(q) = questions.get(q_idx) else {
            return div().into_any_element();
        };
        let multi = q.multi_select;
        let sel_list = selected.get(q_idx).cloned().unwrap_or_default();
        let is_other = other_active.get(q_idx).copied().unwrap_or(false);
        let _cur_other = other_text.get(q_idx).cloned().unwrap_or_default();

        let tabs: Vec<AnyElement> = if questions.len() > 1 {
            questions
                .iter()
                .enumerate()
                .map(|(i, qq)| {
                    let lbl = if qq.header.is_empty() { format!("Q{}", i + 1) } else { qq.header.clone() };
                    let active = i == q_idx;
                    div()
                        .id(("ask-tab", i))
                        .px_2()
                        .h(px(22.))
                        .flex()
                        .items_center()
                        .rounded_sm()
                        .text_xs()
                        .cursor_pointer()
                        .when(active, |d| d.bg(t.active).text_color(t.text).font_weight(FontWeight::MEDIUM))
                        .when(!active, |d| d.text_color(t.text_muted).hover(|s| s.bg(t.hover)))
                        .child(lbl)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.switch_ask_question_tab(i, cx);
                        }))
                        .into_any_element()
                })
                .collect()
        } else {
            Vec::new()
        };

        let pills: Vec<AnyElement> = q
            .options
            .iter()
            .enumerate()
            .map(|(oi, opt)| {
                let picked = sel_list.contains(&oi);
                div()
                    .id(("ask-opt", oi))
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .px_2p5()
                    .py_1p5()
                    .rounded_md()
                    .border_1()
                    .border_color(if picked { t.accent } else { t.border })
                    .bg(if picked { t.selection } else { t.element })
                    .cursor_pointer()
                    .hover(|s| s.border_color(t.border_focused))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .text_xs()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(t.text)
                            .child(if picked { "●" } else { "○" })
                            .child(opt.label.clone()),
                    )
                    .when(!opt.description.is_empty(), |d| {
                        d.child(div().text_size(px(11.)).text_color(t.text_muted).child(opt.description.clone()))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(PendingCard::AskUserQuestion {
                            selected: sel,
                            other_active: oa,
                            active_idx: ai,
                            ..
                        }) = &mut this.pending_card
                        {
                            let idx = *ai;
                            if let Some(list) = sel.get_mut(idx) {
                                if multi {
                                    if let Some(p) = list.iter().position(|&x| x == oi) {
                                        list.remove(p);
                                    } else {
                                        list.push(oi);
                                    }
                                } else {
                                    list.clear();
                                    list.push(oi);
                                    if let Some(flag) = oa.get_mut(idx) {
                                        *flag = false;
                                    }
                                }
                            }
                            cx.notify();
                        }
                    }))
                    .into_any_element()
            })
            .collect();

        let submit_id = req_id.clone();
        let cancel_id = req_id;

        div()
            .flex()
            .flex_none()
            .flex_col()
            .gap_2()
            .p_3()
            .rounded_lg()
            .border_1()
            .border_color(t.warning)
            .bg(t.elevated)
            .font_family(ui_font())
            .when(!tabs.is_empty(), |d| d.child(div().flex().items_center().gap_1().children(tabs)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_sm()
                            .text_color(t.text)
                            .child(q.question.clone()),
                    )
                    .when(multi, |d| d.child(div().text_xs().text_color(t.text_muted).child("Multi-select"))),
            )
            .child(
                div().flex().flex_wrap().gap_1p5().children(pills).child(
                    div()
                        .id("ask-opt-other")
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .px_2p5()
                        .py_1p5()
                        .rounded_md()
                        .border_1()
                        .border_color(if is_other { t.accent } else { t.border })
                        .bg(if is_other { t.selection } else { t.element })
                        .text_xs()
                        .text_color(t.text)
                        .cursor_pointer()
                        .hover(|s| s.border_color(t.border_focused))
                        .child(if is_other { "● Other…" } else { "○ Other…" })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            if let Some(PendingCard::AskUserQuestion {
                                selected: sel,
                                other_active: oa,
                                active_idx: ai,
                                ..
                            }) = &mut this.pending_card
                            {
                                let idx = *ai;
                                if let Some(flag) = oa.get_mut(idx) {
                                    *flag = !*flag;
                                    if *flag
                                        && !multi
                                        && let Some(list) = sel.get_mut(idx)
                                    {
                                        list.clear();
                                    }
                                    if *flag {
                                        this.card_input.read(cx).focus_handle(cx).focus(window, cx);
                                    }
                                }
                                cx.notify();
                            }
                        })),
                ),
            )
            .when(is_other, |d| {
                d.child(
                    div()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .border_1()
                        .border_color(t.border_focused)
                        .bg(t.editor)
                        .text_xs()
                        .child(self.card_input.clone()),
                )
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(primary_button("ask-submit", "Submit answer", cx).on_click(cx.listener(
                        move |this, _, window, cx| {
                            this.submit_ask_questions(&submit_id, window, cx);
                        },
                    )))
                    .child(div().flex_1())
                    .child(outline_button("ask-cancel", "[Esc] Cancel", cx).on_click(cx.listener(
                        move |this, _, window, cx| {
                            this.answer_permission(
                                &cancel_id,
                                PermissionAnswer::Deny { message: Some("Cancelled by user".into()), interrupt: false },
                                window,
                                cx,
                            );
                        },
                    ))),
            )
            .into_any_element()
    }

    fn render_exit_plan_card(
        &mut self,
        req_id: String,
        plan_blocks: Vec<kynex_core::claude::markdown::Block>,
        plan_path: Option<String>,
        show_feedback: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx).clone();
        let supports_auto = self.active_model_supports_auto();
        let ws = self.config.workspace.clone();
        let sel = markdown_view::Sel::new(self.selection_group(cx) ^ (1 << 61), 0, &ws);
        let rendered = markdown_view::render_blocks(&plan_blocks, 888_000, &sel, self.copied_key.as_deref(), cx);
        let scroll = scrollbar::handle("claude-plan-preview-sb");

        let id1 = req_id.clone();
        let id2 = req_id.clone();
        let id3 = req_id.clone();
        let id_revise = req_id;

        div()
            .flex()
            .flex_none()
            .flex_col()
            .gap_2()
            .p_3()
            .rounded_lg()
            .border_1()
            .border_color(t.warning)
            .bg(t.elevated)
            .font_family(ui_font())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(icon(Icon::File, t.warning).size(px(14.)))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_sm()
                            .text_color(t.text)
                            .child("Plan ready for review"),
                    )
                    .when_some(plan_path, |d, p| {
                        let short = short_path(&self.config.workspace, &p);
                        d.child(
                            div()
                                .id("plan-path-link")
                                .font_family(mono())
                                .text_xs()
                                .text_color(t.accent)
                                .cursor_pointer()
                                .hover(|s| s.underline())
                                .child(short)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let full = markdown_view::resolve_workspace_path(&this.config.workspace, &p);
                                    cx.emit(ClaudeChatEvent::OpenFile { path: full, line: None });
                                })),
                        )
                    })
                    .child(div().flex_1())
                    .child(
                        ghost_button("plan-revise-toggle", cx)
                            .h(px(22.))
                            .text_xs()
                            .text_color(t.text_muted)
                            .child(if show_feedback { "Hide feedback" } else { "Revise…" })
                            .on_click(cx.listener(|this, _, window, cx| {
                                if let Some(PendingCard::ExitPlanMode { show_feedback: sf, .. }) =
                                    &mut this.pending_card
                                {
                                    *sf = !*sf;
                                    if *sf {
                                        this.card_input.read(cx).focus_handle(cx).focus(window, cx);
                                    }
                                    cx.notify();
                                }
                            })),
                    ),
            )
            .child(scrollbar::vertical_over(
                "claude-plan-preview-sbar",
                scroll.clone(),
                div()
                    .id("claude-plan-preview-body")
                    .max_h(px(240.))
                    .overflow_y_scroll()
                    .track_scroll(&scroll)
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .border_1()
                    .border_color(t.border_variant)
                    .bg(t.editor)
                    .flex()
                    .flex_col()
                    .gap_1p5()
                    .children(rendered),
                |wrap| wrap.w_full(),
            ))
            .when(show_feedback, |d| {
                d.child(
                    div()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .border_1()
                        .border_color(t.border_focused)
                        .bg(t.editor)
                        .text_xs()
                        .child(self.card_input.clone()),
                )
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .flex_wrap()
                    .gap_2()
                    .child(primary_button("plan-accept-edits", "[1] Approve & Accept edits", cx).on_click(cx.listener(
                        move |this, _, window, cx| {
                            this.answer_permission(
                                &id1,
                                PermissionAnswer::ApprovePlan { next_mode: PermissionMode::AcceptEdits },
                                window,
                                cx,
                            );
                        },
                    )))
                    .when(supports_auto, |d| {
                        d.child(outline_button("plan-accept-auto", "[2] Approve & Auto", cx).on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.answer_permission(
                                    &id2,
                                    PermissionAnswer::ApprovePlan { next_mode: PermissionMode::Auto },
                                    window,
                                    cx,
                                );
                            },
                        )))
                    })
                    .child(outline_button("plan-accept-ask", "[3] Approve & Ask", cx).on_click(cx.listener(
                        move |this, _, window, cx| {
                            this.answer_permission(
                                &id3,
                                PermissionAnswer::ApprovePlan { next_mode: PermissionMode::Default },
                                window,
                                cx,
                            );
                        },
                    )))
                    .child(div().flex_1())
                    .child(
                        outline_button("plan-revise", if show_feedback { "Send revision" } else { "[Esc] Revise" }, cx)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                let fb = this.take_card_input(cx);
                                this.answer_permission(
                                    &id_revise,
                                    PermissionAnswer::Deny {
                                        message: fb.or_else(|| Some("Please revise the plan.".into())),
                                        interrupt: false,
                                    },
                                    window,
                                    cx,
                                );
                            })),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn switch_ask_question_tab(&mut self, next_idx: usize, cx: &mut Context<Self>) {
        let current_text = self.card_input.read(cx).text().to_string();
        let mut next_text = String::new();
        if let Some(PendingCard::AskUserQuestion { active_idx, other_text, .. }) = &mut self.pending_card {
            if let Some(slot) = other_text.get_mut(*active_idx) {
                *slot = current_text;
            }
            *active_idx = next_idx;
            next_text = other_text.get(next_idx).cloned().unwrap_or_default();
        }
        self.card_input.update(cx, |inp, cx| inp.set_text(next_text, cx));
        cx.notify();
    }

    pub(super) fn submit_ask_questions(&mut self, req_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let cur_text = self.card_input.read(cx).text().trim().to_string();
        let mut answers = Map::new();
        if let Some(PendingCard::AskUserQuestion {
            questions, active_idx, selected, other_active, other_text, ..
        }) = &mut self.pending_card
        {
            if let Some(slot) = other_text.get_mut(*active_idx) {
                *slot = cur_text;
            }
            for (i, q) in questions.iter().enumerate() {
                let mut labels: Vec<String> = selected
                    .get(i)
                    .into_iter()
                    .flatten()
                    .filter_map(|&oi| q.options.get(oi).map(|o| o.label.clone()))
                    .collect();
                if other_active.get(i).copied().unwrap_or(false)
                    && let Some(custom) = other_text.get(i).map(|s| s.trim()).filter(|s| !s.is_empty())
                {
                    labels.push(custom.to_string());
                }
                if !labels.is_empty() {
                    answers.insert(q.question.clone(), Value::String(labels.join(", ")));
                }
            }
        }
        self.card_input.update(cx, |inp, cx| inp.set_text("", cx));
        self.answer_permission(req_id, PermissionAnswer::Questions(answers), window, cx);
    }

    pub(super) fn take_card_input(&mut self, cx: &mut Context<Self>) -> Option<String> {
        let text = self.card_input.read(cx).text().trim().to_string();
        if !text.is_empty() {
            self.card_input.update(cx, |inp, cx| inp.set_text("", cx));
            Some(text)
        } else {
            None
        }
    }

    /// Replies to the active permission / question / plan card and pops the next queued card if any.
    pub(super) fn answer_permission(
        &mut self,
        req_id: &str,
        answer: PermissionAnswer,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let PermissionAnswer::ApprovePlan { next_mode } = answer {
            self.permission_mode = next_mode;
        }
        self.note_permission(req_id, &answer);
        if let Some(client) = &self.client
            && let Err(e) = client.reply_permission(req_id, answer)
        {
            self.push_row(LogRow::Notice { text: format!("Failed to send reply: {e}"), is_error: true });
        }
        self.card_input.update(cx, |inp, cx| inp.set_text("", cx));
        if self.pending_card.as_ref().is_some_and(|c| c.req_id() == req_id) {
            self.pending_card = self.queued_cards.pop_front();
        }
        if self.pending_card.is_none() {
            if self.turn_active {
                self.set_status(kynex_core::agent_panes::AgentState::Working, "Working…", cx);
            } else {
                self.set_status(kynex_core::agent_panes::AgentState::Idle, "Ready", cx);
            }
            self.composer_input.read(cx).focus_handle(cx).focus(window, cx);
        }
        cx.notify();
    }

    /// Leaves a one-line record of the answer to request `req_id` in the transcript.
    fn note_permission(&mut self, req_id: &str, answer: &PermissionAnswer) {
        let card = self.pending_card.iter().chain(&self.queued_cards).find(|c| c.req_id() == req_id);
        let title = match card {
            Some(PendingCard::Permission { tool_name, input, .. }) => {
                let what = super::state::tool_summary(&self.config.workspace, tool_name, input);
                if what.is_empty() || what == *tool_name { tool_name.clone() } else { format!("{tool_name} {what}") }
            }
            Some(PendingCard::ExitPlanMode { .. }) => "Plan".to_string(),
            _ => return,
        };
        let (allowed, note) = match answer {
            PermissionAnswer::AllowOnce => (true, "allowed".to_string()),
            PermissionAnswer::AllowSession(_) => (true, "allowed for this session".to_string()),
            PermissionAnswer::ApprovePlan { .. } => (true, "approved".to_string()),
            PermissionAnswer::Deny { message, .. } => match message.as_deref().map(str::trim).filter(|m| !m.is_empty())
            {
                Some(m) => (false, format!("denied: {}", super::state::truncate_chars(m, 120))),
                None => (false, "denied".to_string()),
            },
            PermissionAnswer::Questions(_) => return,
        };
        self.push_row(LogRow::PermissionNote { title, allowed, note });
    }

    /// Handles `1` / `2` / `3` keyboard shortcuts on Permission & ExitPlanMode cards.
    pub(super) fn handle_card_digit(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.card_input.read(cx).is_focused(window) {
            return false;
        }
        let Some(card) = self.pending_card.clone() else { return false };
        match card {
            PendingCard::Permission { req_id, suggestions, .. } => match key {
                "1" => {
                    self.answer_permission(&req_id, PermissionAnswer::AllowOnce, window, cx);
                    true
                }
                "2" if !suggestions.is_empty() => {
                    self.answer_permission(&req_id, PermissionAnswer::AllowSession(suggestions), window, cx);
                    true
                }
                _ => false,
            },
            PendingCard::ExitPlanMode { req_id, .. } => match key {
                "1" => {
                    self.answer_permission(
                        &req_id,
                        PermissionAnswer::ApprovePlan { next_mode: PermissionMode::AcceptEdits },
                        window,
                        cx,
                    );
                    true
                }
                "2" if self.active_model_supports_auto() => {
                    self.answer_permission(
                        &req_id,
                        PermissionAnswer::ApprovePlan { next_mode: PermissionMode::Auto },
                        window,
                        cx,
                    );
                    true
                }
                "3" => {
                    self.answer_permission(
                        &req_id,
                        PermissionAnswer::ApprovePlan { next_mode: PermissionMode::Default },
                        window,
                        cx,
                    );
                    true
                }
                _ => false,
            },
            PendingCard::AskUserQuestion { .. } => false,
        }
    }
}
