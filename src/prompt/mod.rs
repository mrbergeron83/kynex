//! The prompt Claude shows when it needs you: a tool to allow, a plan to approve
//! or questions to answer. Like Claude Code's own prompt it takes the composer's
//! place until it's answered: a cursor over numbered choices (↑↓ and Enter, or
//! 1–9), a plain "No" that says no and stops the turn (as Esc does), and "No,
//! and tell Claude what to do differently" as the last one with its field
//! opening in place.
//!
//! | file           | what                                                    |
//! |----------------|---------------------------------------------------------|
//! | `questions.rs` | `AskUserQuestion`: a tab per question, then Submit      |
//! | `preview.rs`   | titles, "don't ask again" wording, previews and diffs   |

mod preview;
mod questions;

use crate::ui::px;
use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, FontWeight, KeyBinding, KeyDownEvent, Window, actions,
    div, prelude::*,
};
use serde_json::Value;

use kynex_core::mcp;
use kynex_core::project::{self, Mode};
use kynex_core::questions::{Form, Step};
use kynex_core::session::{Answer, Kind};

use crate::markdown;
use crate::theme::{Palette, mono, theme};
use crate::ui::input::{InputEvent, TextInput};
use crate::ui::{Icon, icon, scrollbar};
pub use preview::{diff_stat, preview, title};
use preview::{rules_label, session_label};

actions!(prompt, [SelectPrev, SelectNext, Confirm, Cancel, Tab, NextTab, PrevTab]);

const CONTEXT: &str = "Prompt";

pub fn bind_keys(cx: &mut App) {
    let c = Some(CONTEXT);
    cx.bind_keys([
        KeyBinding::new("up", SelectPrev, c),
        KeyBinding::new("down", SelectNext, c),
        KeyBinding::new("enter", Confirm, c),
        KeyBinding::new("escape", Cancel, c),
        KeyBinding::new("tab", Tab, c),
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
    /// Why the CLI asked (empty if it didn't say); shown under the title.
    pub why: String,
    pub kind: Kind,
}

/// The answer, and a comment that goes to Claude with a yes (Tab on it).
pub struct PromptAnswer(pub Answer, pub String);

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
    /// The choice whose comment field Tab opened, and that field.
    commenting: Option<usize>,
    comment: Entity<TextInput>,
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
                cx.emit(PromptAnswer(Answer::Deny(text), String::new()));
            }
            InputEvent::Changed => {
                this.selected = this.choices.len();
                cx.notify();
            }
            _ => {}
        })
        .detach();

        let comment = cx.new(|cx| TextInput::new(cx).placeholder("Add a comment for Claude"));
        cx.subscribe_in(&comment, window, |this, _, ev, window, cx| {
            if let (InputEvent::Submit, Some(n)) = (ev, this.commenting) {
                this.pick(n, window, cx);
            }
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
                // An MCP server's tools at once, so 30 clicks aren't 30 prompts.
                if let Some((server, _)) = mcp::split(&req.tool) {
                    let label = mcp::server_label(server);
                    choices.push((format!("Yes, and allow all {label} actions this session"), Answer::Server));
                }
                let rules = project::suggested_rules(&req.suggestions);
                if req.kind == Kind::Code && !rules.is_empty() {
                    choices.push((
                        format!("Yes, and always allow {} in this project", rules_label(&rules)),
                        Answer::Project,
                    ));
                }
                // No without a reason, as Esc: the turn stops.
                choices.push(("No (esc)".to_string(), Answer::Deny(String::new())));
                choices
            }
        };

        let focus = cx.focus_handle();
        focus.focus(window, cx);
        Self { req, queued: 0, focus, selected: 0, feedback, choices, commenting: None, comment, form, other }
    }

    fn is_questions(&self) -> bool {
        self.req.tool == "AskUserQuestion"
    }

    fn typing(&self, window: &Window, cx: &App) -> bool {
        self.feedback.read(cx).is_focused(window)
            || self.comment.read(cx).is_focused(window)
            || self.other.iter().any(|o| o.read(cx).is_focused(window))
    }

    /// Puts the caret in the field the cursor is on, or takes it out again.
    fn sync_focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        let field = if self.is_questions() {
            self.form.on_other_row().then(|| self.other.get(self.form.tab)).flatten()
        } else if self.selected == self.choices.len() {
            Some(&self.feedback)
        } else {
            (self.commenting == Some(self.selected)).then_some(&self.comment)
        };
        let handle = field.map_or_else(|| self.focus.clone(), |f| f.read(cx).focus_handle(cx));
        if !handle.is_focused(window) {
            handle.focus(window, cx);
        }
    }

    /// Acts on what a key or click did to the question form.
    fn follow(&mut self, step: Step, window: &mut Window, cx: &mut Context<Self>) {
        match step {
            Step::Submit => cx.emit(PromptAnswer(Answer::Questions(self.form.answers()), String::new())),
            Step::Cancel => cx.emit(PromptAnswer(Answer::Deny(String::new()), String::new())),
            Step::Moved | Step::Type => self.sync_focus(window, cx),
        }
        cx.notify();
    }

    /// Picks choice `n`, with its comment if Tab opened one. The "No" row opens
    /// its field first; picked again (or Enter in the field) it says no, with
    /// whatever was typed.
    fn pick(&mut self, n: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((_, answer)) = self.choices.get(n) {
            let comment =
                if self.commenting == Some(n) { self.comment.read(cx).text().to_string() } else { String::new() };
            cx.emit(PromptAnswer(answer.clone(), comment));
        } else if self.selected != n {
            self.selected = n;
            self.sync_focus(window, cx);
        } else {
            cx.emit(PromptAnswer(Answer::Deny(self.feedback.read(cx).text().to_string()), String::new()));
        }
        cx.notify();
    }

    fn select_prev(&mut self, _: &SelectPrev, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_questions() {
            let step = self.form.move_cursor(false);
            return self.follow(step, window, cx);
        }
        self.selected = self.selected.saturating_sub(1);
        self.commenting = None;
        self.sync_focus(window, cx);
        cx.notify();
    }

    fn select_next(&mut self, _: &SelectNext, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_questions() {
            let step = self.form.move_cursor(true);
            return self.follow(step, window, cx);
        }
        self.selected = (self.selected + 1).min(self.choices.len());
        self.commenting = None;
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

    /// Esc closes an open comment field; otherwise it says no and stops the turn.
    fn cancel(&mut self, _: &Cancel, window: &mut Window, cx: &mut Context<Self>) {
        if self.commenting.take().is_some() {
            self.sync_focus(window, cx);
            return cx.notify();
        }
        cx.emit(PromptAnswer(Answer::Deny(String::new()), String::new()));
    }

    /// Tab moves between questions; as in Claude Code, on a yes it opens a
    /// comment for Claude to go with it (the "No" row's field is open already).
    /// On a plain no it moves to that field: a no with words is that row.
    fn tab(&mut self, _: &Tab, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_questions() || self.selected >= self.choices.len() {
            return self.next_tab(&NextTab, window, cx);
        }
        if matches!(self.choices[self.selected].1, Answer::Deny(_)) {
            self.selected = self.choices.len();
            self.sync_focus(window, cx);
            return cx.notify();
        }
        self.commenting = Some(self.selected);
        self.sync_focus(window, cx);
        cx.notify();
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
            .when(!self.req.why.is_empty(), |d| {
                d.child(div().text_xs().text_color(t.text_placeholder).child(format!("Why: {}", self.req.why)))
            })
            .child({
                let scroll = scrollbar::handle("preview");
                let area =
                    div().id("preview").max_h(max_h).overflow_y_scroll().track_scroll(&scroll).p_2().child(preview(
                        &self.req.tool,
                        &self.req.input,
                        &t,
                    ));
                scrollbar::vertical_over("preview-bar", scroll, area, |d| {
                    d.rounded_md().overflow_hidden().border_1().border_color(t.border_variant).bg(t.editor)
                })
            })
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
            .child({
                let scroll = scrollbar::handle("plan");
                let area = div()
                    .id("plan")
                    .max_h(max_h)
                    .overflow_y_scroll()
                    .track_scroll(&scroll)
                    .px_3()
                    .py_2()
                    .text_sm()
                    .child(markdown::render("plan-md", &plan, 0, 0, None, None, cx));
                scrollbar::vertical_over("plan-bar", scroll, area, |d| {
                    d.rounded_md().overflow_hidden().border_1().border_color(t.border_variant).bg(t.editor)
                })
            })
            .child(self.choice_list(cx))
    }

    /// The numbered choices, then "No" with its field opening in place.
    fn choice_list(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = theme(cx).clone();
        let no = self.choices.len();
        let rows = self.choices.iter().enumerate().map(|(n, (label, _))| {
            let commenting = self.commenting == Some(n);
            choice_row(("choice", n), n, self.selected == n, &t)
                .child(div().flex_none().text_sm().child(if commenting { format!("{label},") } else { label.clone() }))
                .when(commenting, |d| d.child(div().flex_1().text_sm().text_color(t.text).child(self.comment.clone())))
                // A click in the open comment field places the caret; Enter answers.
                .when(!commenting, |d| d.on_click(cx.listener(move |this, _, window, cx| this.pick(n, window, cx))))
        });
        let no_label = match self.req.tool.as_str() {
            "ExitPlanMode" => "No, keep planning (esc)",
            _ => "No, and tell Claude what to do differently",
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
            .child(hint(
                if self.commenting.is_some() {
                    "Enter to answer with the comment · Esc to close it"
                } else {
                    "↑↓ Enter or 1–9 to choose · Tab to add a comment · Esc to say no and stop"
                },
                cx,
            ))
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
            .on_action(cx.listener(Self::tab))
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
