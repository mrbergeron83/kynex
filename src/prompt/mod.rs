//! The prompt Claude shows when it needs you: a tool to allow, a plan to approve
//! or questions to answer. Like Claude Code's own prompt it takes the composer's
//! place until it's answered: a cursor over numbered choices (↑↓ and Enter, or
//! 1–9), "No, and tell Claude what to do differently" as the last one with its
//! field opening in place, and Esc to say no and stop the turn.
//!
//! | file           | what                                                    |
//! |----------------|---------------------------------------------------------|
//! | `questions.rs` | `AskUserQuestion`: a tab per question, then Submit      |
//! | `preview.rs`   | titles, "don't ask again" wording, previews and diffs   |

mod preview;
mod questions;

use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, FontWeight, KeyBinding, KeyDownEvent, Window, actions,
    div, prelude::*, px,
};
use serde_json::Value;

use kynex_core::project::{self, Mode};
use kynex_core::questions::{Form, Step};
use kynex_core::session::{Answer, Kind};

use crate::markdown;
use crate::theme::{Palette, mono, theme};
use crate::ui::input::{InputEvent, TextInput};
use crate::ui::{Icon, icon};
pub use preview::{diff_stat, preview, title};
use preview::{rules_label, session_label};

actions!(prompt, [SelectPrev, SelectNext, Confirm, Cancel, NextTab, PrevTab]);

const CONTEXT: &str = "Prompt";

pub fn bind_keys(cx: &mut App) {
    let c = Some(CONTEXT);
    cx.bind_keys([
        KeyBinding::new("up", SelectPrev, c),
        KeyBinding::new("down", SelectNext, c),
        KeyBinding::new("enter", Confirm, c),
        KeyBinding::new("escape", Cancel, c),
        KeyBinding::new("tab", NextTab, c),
        KeyBinding::new("shift-tab", PrevTab, c),
        KeyBinding::new("right", NextTab, c),
        KeyBinding::new("left", PrevTab, c),
    ]);
}

/// One pending `can_use_tool` request.
#[derive(Clone)]
pub struct Request {
    pub id: String,
    pub tool: String,
    pub input: Value,
    pub suggestions: Value,
    pub description: String,
    pub kind: Kind,
}

pub struct PromptAnswer(pub Answer);

impl EventEmitter<PromptAnswer> for PromptView {}

pub struct PromptView {
    pub req: Request,
    /// More prompts queued behind this one (parallel tool calls).
    pub queued: usize,
    focus: FocusHandle,
    /// The cursor over `choices`, then the "No" row at `choices.len()`.
    selected: usize,
    feedback: Entity<TextInput>,
    choices: Vec<(String, Answer)>,
    /// `AskUserQuestion`: where you are and what's picked, and each
    /// question's "Type something" field.
    form: Form,
    other: Vec<Entity<TextInput>>,
}

impl PromptView {
    pub fn new(req: Request, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let no_hint = match req.tool.as_str() {
            "ExitPlanMode" => "Tell Claude what to change in the plan",
            _ => "Tell Claude what to do differently",
        };
        let feedback = cx.new(|cx| TextInput::new(cx).placeholder(no_hint));
        cx.subscribe_in(&feedback, window, |this, input, ev, _, cx| match ev {
            InputEvent::Submit => {
                let text = input.read(cx).text().to_string();
                cx.emit(PromptAnswer(Answer::Deny(text)));
            }
            InputEvent::Changed => {
                this.selected = this.choices.len();
                cx.notify();
            }
            _ => {}
        })
        .detach();

        let form = Form::parse(&req.input);
        let other = (0..form.questions.len())
            .map(|q| {
                let input = cx.new(|cx| TextInput::new(cx).placeholder("Type your answer"));
                cx.subscribe_in(&input, window, move |this, input, ev, window, cx| match ev {
                    InputEvent::Changed => {
                        let text = input.read(cx).text().to_string();
                        this.form.set_other(q, text);
                        cx.notify();
                    }
                    InputEvent::Submit => {
                        let step = this.form.confirm();
                        this.follow(step, window, cx);
                    }
                    _ => {}
                })
                .detach();
                input
            })
            .collect();

        let choices = match req.tool.as_str() {
            "ExitPlanMode" => vec![
                ("Yes, and use auto mode".to_string(), Answer::Plan(Mode::Auto)),
                ("Yes, and auto-accept edits".to_string(), Answer::Plan(Mode::AcceptEdits)),
                ("Yes, and manually approve edits".to_string(), Answer::Plan(Mode::Manual)),
            ],
            "AskUserQuestion" => Vec::new(),
            _ => {
                let mut choices = vec![("Yes".to_string(), Answer::Once)];
                if let Some(label) = session_label(&req.suggestions) {
                    choices.push((label, Answer::Session));
                }
                let rules = project::suggested_rules(&req.suggestions);
                if req.kind == Kind::Code && !rules.is_empty() {
                    choices.push((
                        format!("Yes, and always allow {} in this project", rules_label(&rules)),
                        Answer::Project,
                    ));
                }
                choices
            }
        };

        let focus = cx.focus_handle();
        focus.focus(window, cx);
        Self { req, queued: 0, focus, selected: 0, feedback, choices, form, other }
    }

    fn is_questions(&self) -> bool {
        self.req.tool == "AskUserQuestion"
    }

    fn typing(&self, window: &Window, cx: &App) -> bool {
        self.feedback.read(cx).is_focused(window) || self.other.iter().any(|o| o.read(cx).is_focused(window))
    }

    /// Puts the caret in the field the cursor is on, or takes it out again.
    fn sync_focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        let field = if self.is_questions() {
            self.form.on_other_row().then(|| self.other.get(self.form.tab)).flatten()
        } else {
            (self.selected == self.choices.len()).then_some(&self.feedback)
        };
        let handle = field.map_or_else(|| self.focus.clone(), |f| f.read(cx).focus_handle(cx));
        if !handle.is_focused(window) {
            handle.focus(window, cx);
        }
    }

    /// Acts on what a key or click did to the question form.
    fn follow(&mut self, step: Step, window: &mut Window, cx: &mut Context<Self>) {
        match step {
            Step::Submit => cx.emit(PromptAnswer(Answer::Questions(self.form.answers()))),
            Step::Cancel => cx.emit(PromptAnswer(Answer::Deny(String::new()))),
            Step::Moved | Step::Type => self.sync_focus(window, cx),
        }
        cx.notify();
    }

    /// Picks choice `n`. The "No" row opens its field first; picked again (or
    /// Enter in the field) it says no, with whatever was typed.
    fn pick(&mut self, n: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((_, answer)) = self.choices.get(n) {
            cx.emit(PromptAnswer(answer.clone()));
        } else if self.selected != n {
            self.selected = n;
            self.sync_focus(window, cx);
        } else {
            cx.emit(PromptAnswer(Answer::Deny(self.feedback.read(cx).text().to_string())));
        }
        cx.notify();
    }

    fn select_prev(&mut self, _: &SelectPrev, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_questions() {
            let step = self.form.move_cursor(false);
            return self.follow(step, window, cx);
        }
        self.selected = self.selected.saturating_sub(1);
        self.sync_focus(window, cx);
        cx.notify();
    }

    fn select_next(&mut self, _: &SelectNext, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_questions() {
            let step = self.form.move_cursor(true);
            return self.follow(step, window, cx);
        }
        self.selected = (self.selected + 1).min(self.choices.len());
        self.sync_focus(window, cx);
        cx.notify();
    }

    fn confirm(&mut self, _: &Confirm, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_questions() {
            let step = self.form.confirm();
            self.follow(step, window, cx);
        } else {
            self.pick(self.selected, window, cx);
        }
    }

    fn cancel(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(PromptAnswer(Answer::Deny(String::new())));
    }

    fn next_tab(&mut self, _: &NextTab, window: &mut Window, cx: &mut Context<Self>) {
        if !self.form.has_tabs() {
            return cx.propagate();
        }
        let step = self.form.next_tab();
        self.follow(step, window, cx);
    }

    fn prev_tab(&mut self, _: &PrevTab, window: &mut Window, cx: &mut Context<Self>) {
        if !self.form.has_tabs() {
            // As in Claude Code, Shift+Tab on a permission prompt allows the
            // action for the rest of the session; without that choice it
            // cycles the permission mode, as in the composer.
            let session = self.choices.iter().position(|(_, a)| *a == Answer::Session);
            return match session {
                Some(n) if !self.is_questions() => self.pick(n, window, cx),
                _ => cx.propagate(),
            };
        }
        let step = self.form.prev_tab();
        self.follow(step, window, cx);
    }

    fn key_down(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &ev.keystroke;
        if self.typing(window, cx) || k.modifiers.modified() {
            return;
        }
        if self.is_questions() {
            let step = if k.key == "space" && self.form.current().is_some_and(|q| q.multi) {
                self.form.choose(self.form.cursor)
            } else if let Some(n) = digit(&k.key) {
                self.form.choose(n)
            } else {
                return;
            };
            cx.stop_propagation();
            return self.follow(step, window, cx);
        }
        if let Some(n) = digit(&k.key).filter(|&n| n <= self.choices.len()) {
            self.pick(n, window, cx);
            cx.stop_propagation();
        }
    }

    fn header(&self, cx: &App) -> impl IntoElement + use<> {
        let t = theme(cx);
        div()
            .flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.warning)
                    .child(title(&self.req.tool, &self.req.input)),
            )
            .when(self.queued > 0, |d| {
                d.child(div().text_xs().text_color(t.text_muted).child(format!("+{} more", self.queued)))
            })
    }

    fn render_permission(&mut self, window: &mut Window, cx: &mut Context<Self>) -> gpui::Div {
        let t = theme(cx).clone();
        let max_h = window.viewport_size().height * 0.35;
        let show_desc = !self.req.description.is_empty()
            && !matches!(self.req.tool.as_str(), "Write" | "Edit" | "MultiEdit" | "Read");
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(self.header(cx))
            .when(show_desc, |d| d.child(div().text_sm().text_color(t.text_muted).child(self.req.description.clone())))
            .child(
                div()
                    .id("preview")
                    .max_h(max_h)
                    .overflow_y_scroll()
                    .rounded_md()
                    .border_1()
                    .border_color(t.border_variant)
                    .bg(t.editor)
                    .p_2()
                    .child(preview(&self.req.tool, &self.req.input, &t)),
            )
            .child(self.choice_list(cx))
    }

    fn render_plan(&mut self, window: &mut Window, cx: &mut Context<Self>) -> gpui::Div {
        let t = theme(cx).clone();
        let max_h = window.viewport_size().height * 0.45;
        let plan = self.req.input["plan"].as_str().unwrap_or("").to_string();
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(self.header(cx))
            .child(
                div()
                    .id("plan")
                    .max_h(max_h)
                    .overflow_y_scroll()
                    .rounded_md()
                    .border_1()
                    .border_color(t.border_variant)
                    .bg(t.editor)
                    .px_3()
                    .py_2()
                    .text_sm()
                    .child(markdown::render("plan-md", &plan, 0, 0, None, cx)),
            )
            .child(self.choice_list(cx))
    }

    /// The numbered choices, then "No" with its field opening in place.
    fn choice_list(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = theme(cx).clone();
        let no = self.choices.len();
        let rows = self.choices.iter().enumerate().map(|(n, (label, _))| {
            choice_row(("choice", n), n, self.selected == n, &t)
                .child(div().text_sm().child(label.clone()))
                .on_click(cx.listener(move |this, _, window, cx| this.pick(n, window, cx)))
        });
        let no_label = match self.req.tool.as_str() {
            "ExitPlanMode" => "No, keep planning (esc)",
            _ => "No, and tell Claude what to do differently (esc)",
        };
        let typed = self.feedback.read(cx).text().to_string();
        let no_body = if self.selected == no {
            div().flex_1().text_sm().text_color(t.text).child(self.feedback.clone())
        } else if !typed.trim().is_empty() {
            div().flex_1().min_w_0().truncate().text_sm().text_color(t.text_muted).child(typed)
        } else {
            div().text_sm().child(no_label)
        };
        div()
            .flex()
            .flex_col()
            .children(rows)
            .child(
                choice_row("choice-no", no, self.selected == no, &t)
                    .child(no_body)
                    .on_click(cx.listener(move |this, _, window, cx| this.pick(no, window, cx))),
            )
            .child(hint("↑↓ Enter or 1–9 to choose · Esc to say no and stop", cx))
    }
}

impl Focusable for PromptView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for PromptView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match self.req.tool.as_str() {
            "AskUserQuestion" => self.render_questions(window, cx),
            "ExitPlanMode" => self.render_plan(window, cx),
            _ => self.render_permission(window, cx),
        };
        div()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::select_prev))
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(Self::next_tab))
            .on_action(cx.listener(Self::prev_tab))
            .on_key_down(cx.listener(Self::key_down))
            .child(body)
    }
}

/// 1–9 as a row index.
fn digit(key: &str) -> Option<usize> {
    key.parse::<usize>().ok().filter(|n| (1..=9).contains(n)).map(|n| n - 1)
}

/// One numbered, clickable choice. The one under the cursor gets a pointer and
/// the accent colour, like Claude Code's `❯`; callers add the label.
fn choice_row(id: impl Into<gpui::ElementId>, n: usize, focused: bool, t: &Palette) -> gpui::Stateful<gpui::Div> {
    let hover = t.hover;
    let color = if focused { t.accent } else { t.text };
    div()
        .id(id)
        .flex()
        .items_start()
        .gap_1p5()
        .px_1()
        .py_0p5()
        .rounded_sm()
        .text_color(color)
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .child(
            div()
                .flex_none()
                .w(px(12.))
                .h(px(20.))
                .flex()
                .items_center()
                .when(focused, |d| d.child(icon(Icon::ChevronRight, t.accent).size_3())),
        )
        .child(
            div()
                .flex_none()
                .min_w(px(16.))
                .font_family(mono())
                .text_sm()
                .text_color(if focused { t.accent } else { t.text_placeholder })
                .child(format!("{}.", n + 1)),
        )
}

fn hint(text: &'static str, cx: &App) -> impl IntoElement + use<> {
    div().pt_1().text_xs().text_color(theme(cx).text_placeholder).child(text)
}
