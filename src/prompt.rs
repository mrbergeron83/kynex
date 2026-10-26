//! The prompt Claude shows when it needs you: a tool to allow, a plan to approve
//! or questions to answer. Like the VS Code extension and Claude Desktop it takes
//! the composer's place until it's answered, with numbered choices: 1–9 picks
//! one, ↑↓ and Enter work too, Esc says no and stops the turn.

use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, FontWeight, KeyBinding, KeyDownEvent, SharedString,
    Window, actions, div, prelude::*, px,
};
use serde_json::Value;

use kynex_core::project::{self, Mode};
use kynex_core::session::{Answer, Kind};

use crate::markdown;
use crate::theme::{Palette, mono, theme};
use crate::ui::input::{InputEvent, TextInput};
use crate::ui::{Tooltip, outline_button, primary_button};

actions!(prompt, [SelectPrev, SelectNext, Confirm, Cancel]);

const CONTEXT: &str = "Prompt";

pub fn bind_keys(cx: &mut App) {
    let c = Some(CONTEXT);
    cx.bind_keys([
        KeyBinding::new("up", SelectPrev, c),
        KeyBinding::new("down", SelectNext, c),
        KeyBinding::new("enter", Confirm, c),
        KeyBinding::new("escape", Cancel, c),
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
    selected: usize,
    feedback: Entity<TextInput>,
    /// `AskUserQuestion`: the chosen options and the "Other" field, per question.
    picks: Vec<Vec<usize>>,
    other: Vec<Entity<TextInput>>,
    choices: Vec<(String, Answer)>,
}

impl PromptView {
    pub fn new(req: Request, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let no_hint = match req.tool.as_str() {
            "ExitPlanMode" => "No, keep planning: tell Claude what to change",
            _ => "No, and tell Claude what to do differently",
        };
        let feedback = cx.new(|cx| TextInput::new(cx).placeholder(no_hint));
        cx.subscribe_in(&feedback, window, |this, input, ev, window, cx| match ev {
            InputEvent::Submit => {
                let text = input.read(cx).text().to_string();
                if text.trim().is_empty() {
                    this.focus.focus(window, cx);
                } else {
                    cx.emit(PromptAnswer(Answer::Deny(text)));
                }
            }
            InputEvent::Changed => {
                this.selected = this.choices.len();
                cx.notify();
            }
            _ => {}
        })
        .detach();

        let qs = req.input["questions"].as_array().cloned().unwrap_or_default();
        let other: Vec<_> = qs
            .iter()
            .enumerate()
            .map(|(q, _)| {
                let input = cx.new(|cx| TextInput::new(cx).placeholder("Other: type your own answer"));
                cx.subscribe_in(&input, window, move |this, _, ev, window, cx| match ev {
                    InputEvent::Changed => {
                        if this.multi(q) {
                            cx.notify();
                        } else {
                            this.picks[q].clear();
                            cx.notify();
                        }
                    }
                    InputEvent::Submit => {
                        if this.single() {
                            this.submit_questions(cx);
                        } else {
                            this.focus.focus(window, cx);
                        }
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
                    choices.push((format!("Yes, and always allow {} in this project", rules_label(&rules)), Answer::Project));
                }
                choices
            }
        };

        let focus = cx.focus_handle();
        focus.focus(window, cx);
        Self { picks: vec![Vec::new(); qs.len()], req, queued: 0, focus, selected: 0, feedback, other, choices }
    }

    fn is_questions(&self) -> bool {
        self.req.tool == "AskUserQuestion"
    }

    fn questions(&self) -> Vec<Value> {
        self.req.input["questions"].as_array().cloned().unwrap_or_default()
    }

    fn multi(&self, q: usize) -> bool {
        self.req.input["questions"][q]["multiSelect"] == true
    }

    /// A single single-choice question answers with one key, like the CLI.
    fn single(&self) -> bool {
        self.questions().len() == 1 && !self.multi(0)
    }

    fn typing(&self, window: &Window, cx: &App) -> bool {
        self.feedback.read(cx).is_focused(window) || self.other.iter().any(|o| o.read(cx).is_focused(window))
    }

    fn pick(&mut self, n: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.selected = n;
        if let Some((_, answer)) = self.choices.get(n) {
            cx.emit(PromptAnswer(answer.clone()));
            return;
        }
        // "No": with nothing typed yet move to the field; with text, send it.
        let text = self.feedback.read(cx).text().to_string();
        if text.trim().is_empty() {
            self.feedback.read(cx).focus_handle(cx).focus(window, cx);
            cx.notify();
        } else {
            cx.emit(PromptAnswer(Answer::Deny(text)));
        }
    }

    fn select_prev(&mut self, _: &SelectPrev, _: &mut Window, cx: &mut Context<Self>) {
        self.selected = self.selected.saturating_sub(1);
        cx.notify();
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        self.selected = (self.selected + 1).min(self.choices.len());
        cx.notify();
    }

    fn confirm(&mut self, _: &Confirm, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_questions() {
            self.submit_questions(cx);
        } else {
            self.pick(self.selected, window, cx);
        }
    }

    fn cancel(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(PromptAnswer(Answer::Deny(String::new())));
    }

    fn key_down(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &ev.keystroke;
        if self.typing(window, cx) || k.modifiers.modified() {
            return;
        }
        let Some(n) = k.key.parse::<usize>().ok().filter(|n| (1..=9).contains(n)).map(|n| n - 1) else { return };
        if self.is_questions() {
            let count = self.questions().first().and_then(|q| q["options"].as_array().map(Vec::len)).unwrap_or(0);
            if self.single() && n < count {
                self.picks[0] = vec![n];
                self.submit_questions(cx);
                cx.stop_propagation();
            }
        } else if n <= self.choices.len() {
            self.pick(n, window, cx);
            cx.stop_propagation();
        }
    }

    fn toggle(&mut self, q: usize, n: usize, cx: &mut Context<Self>) {
        let picks = &mut self.picks[q];
        if !self.req.input["questions"][q]["multiSelect"].as_bool().unwrap_or(false) {
            *picks = vec![n];
        } else if let Some(at) = picks.iter().position(|&p| p == n) {
            picks.remove(at);
        } else {
            picks.push(n);
        }
        self.other[q].update(cx, |o, cx| o.set_text("", cx));
        if self.single() {
            self.submit_questions(cx);
        }
        cx.notify();
    }

    fn answers(&self, cx: &App) -> Vec<Option<String>> {
        self.questions()
            .iter()
            .enumerate()
            .map(|(q, question)| {
                let other = self.other[q].read(cx).text().trim().to_string();
                if !other.is_empty() {
                    return Some(other);
                }
                let labels: Vec<&str> =
                    self.picks[q].iter().filter_map(|&n| question["options"][n]["label"].as_str()).collect();
                (!labels.is_empty()).then(|| labels.join(", "))
            })
            .collect()
    }

    fn submit_questions(&mut self, cx: &mut Context<Self>) {
        let answers = self.answers(cx);
        if !answers.iter().all(Option::is_some) {
            return;
        }
        let map = self
            .questions()
            .iter()
            .zip(answers)
            .filter_map(|(question, a)| Some((question["question"].as_str()?.to_string(), Value::String(a?))))
            .collect();
        cx.emit(PromptAnswer(Answer::Questions(map)));
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
            .when(self.queued > 0, |d| d.child(div().text_xs().text_color(t.text_muted).child(format!("+{} more", self.queued))))
    }

    fn render_permission(&mut self, window: &mut Window, cx: &mut Context<Self>) -> gpui::Div {
        let t = theme(cx).clone();
        let max_h = window.viewport_size().height * 0.35;
        let show_desc = !self.req.description.is_empty() && !matches!(self.req.tool.as_str(), "Write" | "Edit" | "MultiEdit" | "Read");
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
                    .child(markdown::render("plan-md", &plan, 0, 0, cx)),
            )
            .child(self.choice_list(cx))
    }

    /// Numbered choices plus a last "No" row with a feedback field.
    fn choice_list(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = theme(cx).clone();
        let no = self.choices.len();
        let rows = self.choices.iter().enumerate().map(|(n, (label, _))| {
            choice_row(("choice", n), n, self.selected == n, &t)
                .child(div().text_sm().child(label.clone()))
                .on_click(cx.listener(move |this, _, window, cx| this.pick(n, window, cx)))
        });
        div()
            .flex()
            .flex_col()
            .gap_0p5()
            .children(rows)
            .child(
                choice_row("choice-no", no, self.selected == no, &t)
                    .child(div().flex_1().text_sm().child(self.feedback.clone()))
                    .on_click(cx.listener(move |this, _, window, cx| this.pick(no, window, cx))),
            )
            .child(hint("1–9 or ↑↓ Enter to choose · Esc to say no and stop", cx))
    }

    fn render_questions(&mut self, window: &mut Window, cx: &mut Context<Self>) -> gpui::Div {
        let t = theme(cx).clone();
        let qs = self.questions();
        let single = self.single();
        let complete = self.answers(cx).iter().all(Option::is_some);
        let max_h = window.viewport_size().height * 0.5;
        let blocks = qs.iter().enumerate().map(|(q, question)| {
            let options = question["options"].as_array().cloned().unwrap_or_default();
            let other_filled = !self.other[q].read(cx).text().trim().is_empty();
            div()
                .flex()
                .flex_col()
                .gap_0p5()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .pb_1()
                        .children(question["header"].as_str().map(|h| {
                            div()
                                .px_1p5()
                                .rounded_sm()
                                .bg(t.element)
                                .text_xs()
                                .text_color(t.info)
                                .child(h.to_string())
                        }))
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::MEDIUM)
                                .child(question["question"].as_str().unwrap_or("").to_string()),
                        ),
                )
                .children(options.iter().enumerate().map(|(n, option)| {
                    let picked = self.picks[q].contains(&n);
                    choice_row(SharedString::from(format!("q{q}-{n}")), n, picked, &t)
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .child(div().text_sm().child(option["label"].as_str().unwrap_or("").to_string()))
                                .children(option["description"].as_str().filter(|d| !d.is_empty()).map(|d| {
                                    div().text_xs().text_color(t.text_muted).child(d.to_string())
                                })),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| this.toggle(q, n, cx)))
                }))
                .child(
                    choice_row(SharedString::from(format!("q{q}-other")), options.len(), other_filled, &t)
                        .child(div().flex_1().text_sm().child(self.other[q].clone())),
                )
        });
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(self.header(cx))
            .child(div().id("questions").max_h(max_h).overflow_y_scroll().flex().flex_col().gap_3().children(blocks))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .when(!single, |d| {
                        d.child(
                            primary_button("submit", "Submit answers", cx)
                                .when(!complete, |b| b.opacity(0.5))
                                .on_click(cx.listener(|this, _, _, cx| this.submit_questions(cx))),
                        )
                    })
                    .child(
                        outline_button("skip", "Skip", cx)
                            .tooltip(Tooltip::text("Don't answer; Claude stops (Esc)"))
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(PromptAnswer(Answer::Deny(String::new()))))),
                    )
                    .child(hint(if single { "1–9 to choose · Esc to skip" } else { "Esc to skip" }, cx)),
            )
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
            .on_key_down(cx.listener(Self::key_down))
            .child(body)
    }
}

/// One numbered, clickable choice, outlined while selected.
fn choice_row(id: impl Into<gpui::ElementId>, n: usize, selected: bool, t: &Palette) -> gpui::Stateful<gpui::Div> {
    let hover = t.hover;
    div()
        .id(id)
        .flex()
        .items_start()
        .gap_2()
        .px_2()
        .py_1()
        .rounded_md()
        .border_1()
        .border_color(if selected { t.warning } else { gpui::transparent_black() })
        .when(selected, |d| d.bg(t.hover))
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .child(div().flex_none().w(px(12.)).font_family(mono()).text_sm().text_color(t.text_placeholder).child(format!("{}", n + 1)))
}

fn hint(text: &'static str, cx: &App) -> impl IntoElement + use<> {
    div().pt_1().text_xs().text_color(theme(cx).text_placeholder).child(text)
}

/// The headline for a request, e.g. "Allow Claude to run this command?".
pub fn title(tool: &str, input: &Value) -> String {
    let file = || input["file_path"].as_str().or(input["notebook_path"].as_str()).map(file_name).unwrap_or("this file");
    match tool {
        "Bash" | "PowerShell" => "Allow Claude to run this command?".into(),
        "Edit" | "MultiEdit" | "NotebookEdit" => format!("Make this edit to {}?", file()),
        "Write" => format!("Allow Claude to write {}?", file()),
        "Read" => format!("Allow Claude to read {}?", file()),
        "WebFetch" => format!("Allow Claude to fetch {}?", input["url"].as_str().unwrap_or("this page")),
        "WebSearch" => format!("Allow Claude to search the web for \u{201c}{}\u{201d}?", input["query"].as_str().unwrap_or("")),
        "ExitPlanMode" => "Ready to code? Here is Claude's plan:".into(),
        "AskUserQuestion" => "Claude has a question".into(),
        _ => match tool.strip_prefix("mcp__").and_then(|t| t.split_once("__")) {
            Some((server, name)) => format!("Allow Claude to use {name} from {server}?"),
            None => format!("Allow Claude to use {tool}?"),
        },
    }
}

fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().filter(|n| !n.is_empty()).unwrap_or(path)
}

/// The "don't ask again this session" choice, worded after what it unlocks.
fn session_label(suggestions: &Value) -> Option<String> {
    let list = suggestions.as_array().filter(|a| !a.is_empty())?;
    let mut parts = Vec::new();
    for s in list {
        match s["type"].as_str() {
            Some("setMode") if s["mode"] == "acceptEdits" => return Some("Yes, allow all edits this session".into()),
            Some("addRules") if s["behavior"] == "allow" => {
                let rules = project::suggested_rules(&Value::Array(vec![s.clone()]));
                if !rules.is_empty() {
                    parts.push(rules_label(&rules));
                }
            }
            Some("addDirectories") => {
                let dirs: Vec<&str> = s["directories"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
                if !dirs.is_empty() {
                    parts.push(format!("access to {}", dirs.join(", ")));
                }
            }
            _ => {}
        }
    }
    Some(if parts.is_empty() {
        "Yes, and don't ask again this session".into()
    } else {
        format!("Yes, and don't ask again for {} this session", parts.join(" and "))
    })
}

/// `Bash(npm test:*)` reads as "`npm test:*` commands", `WebFetch(domain:x)` as "fetches from x".
fn rules_label(rules: &[String]) -> String {
    let words: Vec<String> = rules
        .iter()
        .map(|r| match r.split_once('(').and_then(|(t, c)| Some((t, c.strip_suffix(')')?))) {
            Some(("Bash" | "PowerShell", c)) => format!("`{c}` commands"),
            Some(("WebFetch", c)) => format!("fetches from {}", c.trim_start_matches("domain:")),
            Some((t, c)) => format!("{t} in {c}"),
            None => r.clone(),
        })
        .collect();
    words.join(", ")
}

/// What the tool will do: the command, the edit as a diff, the file to write…
fn preview(tool: &str, input: &Value, t: &Palette) -> impl IntoElement + use<> {
    let code = |s: &str| div().font_family(mono()).text_size(px(13.)).child(s.to_string());
    let path_line = |input: &Value| {
        input["file_path"].as_str().or(input["notebook_path"].as_str()).map(|p| {
            div().font_family(mono()).text_xs().text_color(t.text_muted).pb_1().child(p.to_string())
        })
    };
    let el = div().flex().flex_col();
    match tool {
        "Bash" | "PowerShell" => el.child(code(input["command"].as_str().unwrap_or(""))),
        "Edit" => el.children(path_line(input)).child(diff_lines(
            input["old_string"].as_str().unwrap_or(""),
            input["new_string"].as_str().unwrap_or(""),
            t,
        )),
        "MultiEdit" => el.children(path_line(input)).children(input["edits"].as_array().into_iter().flatten().enumerate().map(
            |(n, e)| {
                div()
                    .when(n > 0, |d| d.mt_2().pt_2().border_t_1().border_color(t.border_variant))
                    .child(diff_lines(e["old_string"].as_str().unwrap_or(""), e["new_string"].as_str().unwrap_or(""), t))
            },
        )),
        "Write" => el.children(path_line(input)).child(diff_lines("", input["content"].as_str().unwrap_or(""), t)),
        "NotebookEdit" => el.children(path_line(input)).child(diff_lines("", input["new_source"].as_str().unwrap_or(""), t)),
        "Read" => el.children(path_line(input)),
        "WebFetch" => el
            .child(code(input["url"].as_str().unwrap_or("")))
            .children(input["prompt"].as_str().map(|p| div().pt_1().text_sm().text_color(t.text_muted).child(p.to_string()))),
        _ => el.child(code(&serde_json::to_string_pretty(input).unwrap_or_default())),
    }
}

/// Which lines of a line diff to show: the shared lines at either end as (up
/// to 3 lines of) context, the rest as removed and added.
pub fn diff(old: &str, new: &str) -> Vec<(char, String)> {
    const CONTEXT: usize = 3;
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    let head = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let tail = a[head..].iter().rev().zip(b[head..].iter().rev()).take_while(|(x, y)| x == y).count();
    let mut lines = Vec::new();
    lines.extend(a[head.saturating_sub(CONTEXT)..head].iter().map(|l| (' ', l.to_string())));
    lines.extend(a[head..a.len() - tail].iter().map(|l| ('-', l.to_string())));
    lines.extend(b[head..b.len() - tail].iter().map(|l| ('+', l.to_string())));
    lines.extend(a[a.len() - tail..(a.len() - tail + CONTEXT).min(a.len())].iter().map(|l| (' ', l.to_string())));
    lines
}

fn diff_lines(old: &str, new: &str, t: &Palette) -> impl IntoElement + use<> {
    const MAX_LINES: usize = 400;
    let lines = diff(old, new);
    let more = lines.len().saturating_sub(MAX_LINES);
    let (add_bg, del_bg) = (t.success.opacity(0.12), t.error.opacity(0.12));
    div()
        .flex()
        .flex_col()
        .font_family(mono())
        .text_size(px(13.))
        .children(lines.into_iter().take(MAX_LINES).map(move |(sign, line)| {
            let row = div().flex().px_1().child(div().w(px(14.)).flex_none().child(sign.to_string())).child(line);
            match sign {
                '+' => row.bg(add_bg).text_color(t.success),
                '-' => row.bg(del_bg).text_color(t.error),
                _ => row.text_color(t.text_muted),
            }
        }))
        .when(more > 0, |d| d.child(div().pt_1().text_xs().text_color(t.text_muted).child(format!("… {more} more lines"))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn session_choice_says_what_it_unlocks() {
        let edits = json!([{ "type": "setMode", "mode": "acceptEdits", "destination": "session" }]);
        assert_eq!(session_label(&edits).unwrap(), "Yes, allow all edits this session");
        let bash = json!([{ "type": "addRules", "behavior": "allow", "destination": "localSettings",
            "rules": [{ "toolName": "Bash", "ruleContent": "npm view *" }] }]);
        assert_eq!(session_label(&bash).unwrap(), "Yes, and don't ask again for `npm view *` commands this session");
        assert_eq!(session_label(&json!([])), None);
        assert_eq!(session_label(&Value::Null), None);
    }

    #[test]
    fn titles_name_the_target() {
        assert_eq!(title("Edit", &json!({ "file_path": "C:\\src\\main.rs" })), "Make this edit to main.rs?");
        assert_eq!(title("mcp__github__create_issue", &json!({})), "Allow Claude to use create_issue from github?");
    }

    #[test]
    fn diff_keeps_three_lines_of_context() {
        let old = "a\nb\nc\nd\ne\nf\ng";
        let new = "a\nb\nc\nd\nE\nf\ng";
        let d = diff(old, new);
        assert_eq!(d.first().unwrap(), &(' ', "b".to_string()));
        assert!(d.contains(&('-', "e".to_string())) && d.contains(&('+', "E".to_string())));
        assert_eq!(d.len(), 3 + 2 + 2);
    }
}
