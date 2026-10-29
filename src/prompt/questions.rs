//! `AskUserQuestion` drawn like Claude Code's prompt: one question at a time,
//! with a tab per question (checked once answered) and a Submit tab that sums
//! up the answers. The rules are `kynex_core::questions::Form`.

use gpui::{Context, FontWeight, SharedString, Window, div, prelude::*, px};

use super::{PromptView, choice_row, hint};
use crate::theme::{Palette, theme};
use crate::ui::{Icon, icon, scrollbar};

impl PromptView {
    pub(super) fn render_questions(&mut self, window: &mut Window, cx: &mut Context<Self>) -> gpui::Div {
        let t = theme(cx).clone();
        let max_h = window.viewport_size().height * 0.5;
        let body = if self.form.on_submit_tab() { self.render_summary(&t, cx) } else { self.render_question(&t, cx) };
        let multi = self.form.current().is_some_and(|q| q.multi);
        let keys = match (self.form.has_tabs(), multi) {
            (true, true) => "←→ between questions · ↑↓ Space to toggle · Enter for the next · Esc to skip",
            (true, false) => "←→ between questions · ↑↓ Enter or 1–9 to choose · Esc to skip",
            (false, true) => "↑↓ Space to toggle · Enter to submit · Esc to skip",
            (false, false) => "↑↓ Enter or 1–9 to choose · Esc to skip",
        };
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(self.header(cx))
            .when(self.form.has_tabs(), |d| d.child(self.render_tabs(&t, cx)))
            .child({
                let scroll = scrollbar::handle("question");
                let area = div().id("question").max_h(max_h).overflow_y_scroll().track_scroll(&scroll).child(body);
                scrollbar::vertical_over("question-bar", scroll, area, |d| d)
            })
            .child(hint(keys, cx))
    }

    /// A tab per question, checked once answered, then Submit.
    fn render_tabs(&self, t: &Palette, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let tab = |id: SharedString, label: String, answered: Option<bool>, current: bool| {
            let hover = t.hover;
            div()
                .id(id)
                .flex()
                .items_center()
                .gap_1()
                .h(px(22.))
                .px_2()
                .rounded_md()
                .text_xs()
                .cursor_pointer()
                .hover(move |s| s.bg(hover))
                .when(current, |d| d.bg(t.element).text_color(t.text))
                .when(!current, |d| d.text_color(t.text_muted))
                .when_some(answered, |d, answered| d.child(check_box(answered, t)))
                .child(label)
        };
        let questions = self.form.questions.iter().enumerate().map(|(q, question)| {
            let label =
                if question.header.is_empty() { format!("Question {}", q + 1) } else { question.header.clone() };
            tab(format!("tab-{q}").into(), label, Some(self.form.answer(q).is_some()), self.form.tab == q).on_click(
                cx.listener(move |this, _, window, cx| {
                    let step = this.form.go_to(q);
                    this.follow(step, window, cx);
                }),
            )
        });
        let n = self.form.questions.len();
        div().flex().flex_wrap().items_center().gap_1().children(questions).child(
            tab("tab-submit".into(), "Submit".into(), None, self.form.on_submit_tab()).on_click(cx.listener(
                move |this, _, window, cx| {
                    let step = this.form.go_to(n);
                    this.follow(step, window, cx);
                },
            )),
        )
    }

    fn render_question(&self, t: &Palette, cx: &mut Context<Self>) -> gpui::Div {
        let q = self.form.tab;
        let Some(question) = self.form.current() else { return div() };
        let rows = question.options.iter().enumerate().map(|(n, option)| {
            let focused = self.form.cursor == n;
            let picked = self.form.picked(q, n);
            choice_row(SharedString::from(format!("q{q}-{n}")), n, focused, t)
                .when(picked, |d| d.bg(t.element))
                .when(question.multi, |d| d.child(div().h(px(20.)).flex().items_center().child(check_box(picked, t))))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_1()
                                .text_sm()
                                .when(picked && !question.multi && !focused, |d| d.text_color(t.success))
                                .child(option.label.clone())
                                .when(picked && !question.multi, |d| {
                                    d.child(icon(Icon::Check, t.success).size(px(12.)))
                                }),
                        )
                        .when(!option.description.is_empty(), |d| {
                            d.child(div().text_xs().text_color(t.text_muted).child(option.description.clone()))
                        }),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    let step = this.form.choose(n);
                    this.follow(step, window, cx);
                }))
        });
        let other = self.form.other_row(q);
        let typing = self.form.cursor == other;
        let typed = self.form.other(q).trim().to_string();
        let other_body = if typing {
            div().flex_1().text_sm().text_color(t.text).child(self.other[q].clone())
        } else if !typed.is_empty() {
            div().flex_1().min_w_0().truncate().text_sm().text_color(t.success).child(typed)
        } else {
            div().text_sm().text_color(t.text_muted).child("Type something.")
        };
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .pb_1()
                    .text_sm()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(t.text)
                    .child(question.question.clone()),
            )
            .children(rows)
            .child(choice_row(SharedString::from(format!("q{q}-other")), other, typing, t).child(other_body).on_click(
                cx.listener(move |this, _, window, cx| {
                    let step = this.form.choose(other);
                    this.follow(step, window, cx);
                }),
            ))
    }

    /// The Submit tab: every question and its answer, then Submit / Cancel.
    fn render_summary(&self, t: &Palette, cx: &mut Context<Self>) -> gpui::Div {
        let answers = self.form.questions.iter().enumerate().map(|(q, question)| {
            let answer = self.form.answer(q);
            div()
                .flex()
                .flex_col()
                .child(div().text_xs().text_color(t.text_muted).child(question.question.clone()))
                .child(match answer {
                    Some(a) => div().text_sm().text_color(t.text).child(format!("→ {a}")),
                    None => div().text_sm().text_color(t.warning).child("→ not answered yet"),
                })
        });
        let complete = self.form.complete();
        let actions = [if complete { "Submit answers" } else { "Answer the rest" }, "Cancel"];
        let rows = actions.into_iter().enumerate().map(|(n, label)| {
            choice_row(("summary", n), n, self.form.cursor == n, t).child(div().text_sm().child(label)).on_click(
                cx.listener(move |this, _, window, cx| {
                    let step = this.form.choose(n);
                    this.follow(step, window, cx);
                }),
            )
        });
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(div().flex().flex_col().gap_1p5().pl_1().children(answers))
            .child(div().flex().flex_col().children(rows))
    }
}

/// A small checkbox: empty, or filled with a check.
fn check_box(checked: bool, t: &Palette) -> impl IntoElement + use<> {
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(12.))
        .rounded_sm()
        .border_1()
        .border_color(if checked { t.accent } else { t.border })
        .when(checked, |d| d.bg(t.accent).child(icon(Icon::Check, t.on_accent).size(px(10.))))
}
