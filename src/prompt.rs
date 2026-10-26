//! The prompt Claude shows when it needs you: a tool to allow, a plan to approve
//! or questions to answer. Like the VS Code extension and Claude Desktop it takes
//! the composer's place until it's answered, with numbered choices: 1–9 picks
//! one, ↑↓ and Enter work too, Esc says no and stops the turn.

use eframe::egui::{self, RichText};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use serde_json::Value;

use crate::project::{self, Mode};
use crate::session::{Answer, Kind};
use crate::theme;

/// One pending `can_use_tool` request.
pub struct Request<'a> {
    pub id: &'a str,
    pub tool: &'a str,
    pub input: &'a Value,
    pub suggestions: &'a Value,
    pub description: &'a str,
    pub kind: Kind,
    /// More prompts queued behind this one (parallel tool calls).
    pub queued: usize,
}

/// What's picked so far, kept in egui's temp memory per request.
#[derive(Clone, Default)]
struct State {
    selected: usize,
    feedback: String,
    /// `AskUserQuestion`: the chosen options and the "Other" text, per question.
    picks: Vec<Vec<usize>>,
    other: Vec<String>,
}

const DIGITS: [egui::Key; 9] = [
    egui::Key::Num1, egui::Key::Num2, egui::Key::Num3, egui::Key::Num4, egui::Key::Num5,
    egui::Key::Num6, egui::Key::Num7, egui::Key::Num8, egui::Key::Num9,
];

/// Shows the prompt; returns the answer once the user gives one.
pub fn show(ui: &mut egui::Ui, md: &mut CommonMarkCache, req: &Request) -> Option<Answer> {
    let id = egui::Id::unique(("prompt", req.id));
    let mut state = ui.data(|d| d.get_temp::<State>(id)).unwrap_or_default();
    let answer = match req.tool {
        "AskUserQuestion" => questions(ui, id, req, &mut state),
        "ExitPlanMode" => plan(ui, md, id, req, &mut state),
        _ => permission(ui, id, req, &mut state),
    };
    ui.data_mut(|d| d.insert_temp(id, state));
    answer
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

fn header(ui: &mut egui::Ui, text: &str, queued: usize) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(text).strong().color(theme::p().peach));
        if queued > 0 {
            ui.label(RichText::new(format!("+{queued} more")).small().weak());
        }
    });
}

/// "Allow …?" with Yes / Yes for the session / Yes for the project / No + feedback.
fn permission(ui: &mut egui::Ui, id: egui::Id, req: &Request, state: &mut State) -> Option<Answer> {
    header(ui, &title(req.tool, req.input), req.queued);
    if !req.description.is_empty() && !matches!(req.tool, "Write" | "Edit" | "MultiEdit" | "Read") {
        ui.label(RichText::new(req.description).weak());
    }
    ui.add_space(4.0);
    preview(ui, req.tool, req.input);
    ui.add_space(6.0);

    let mut choices = vec![("Yes".to_string(), Answer::Once)];
    if let Some(label) = session_label(req.suggestions) {
        choices.push((label, Answer::Session));
    }
    let rules = project::suggested_rules(req.suggestions);
    if req.kind == Kind::Code && !rules.is_empty() {
        choices.push((format!("Yes, and always allow {} in this project", rules_label(&rules)), Answer::Project));
    }
    choose(ui, id, state, choices, "No, and tell Claude what to do differently")
}

/// The plan, then which mode to build it in, or keep planning with feedback.
fn plan(ui: &mut egui::Ui, md: &mut CommonMarkCache, id: egui::Id, req: &Request, state: &mut State) -> Option<Answer> {
    header(ui, &title(req.tool, req.input), req.queued);
    ui.add_space(4.0);
    let max = ui.ctx().content_rect().height() * 0.45;
    code_frame(ui, |ui| {
        egui::ScrollArea::vertical().id_salt(id.with("plan")).max_height(max).show(ui, |ui| {
            CommonMarkViewer::new().show(ui, md, req.input["plan"].as_str().unwrap_or(""));
        });
    });
    ui.add_space(6.0);
    let choices = vec![
        ("Yes, and use auto mode".to_string(), Answer::Plan(Mode::Auto)),
        ("Yes, and auto-accept edits".to_string(), Answer::Plan(Mode::AcceptEdits)),
        ("Yes, and manually approve edits".to_string(), Answer::Plan(Mode::Manual)),
    ];
    choose(ui, id, state, choices, "No, keep planning: tell Claude what to change")
}

/// Numbered choices plus a last "No" row with a feedback field. Handles the keys.
fn choose(ui: &mut egui::Ui, id: egui::Id, state: &mut State, choices: Vec<(String, Answer)>, no_hint: &str) -> Option<Answer> {
    let no = choices.len();
    let field_id = id.with("feedback");
    let field_focused = ui.memory(|m| m.has_focus(field_id));
    // Shortcuts apply unless some other text field has the keyboard.
    let keys = field_focused || !ui.ctx().egui_wants_keyboard_input();
    let mut picked = None;

    if keys && !field_focused {
        ui.input_mut(|i| {
            if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown) {
                state.selected = (state.selected + 1).min(no);
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp) {
                state.selected = state.selected.saturating_sub(1);
            }
            for (n, key) in DIGITS.iter().enumerate().take(no + 1) {
                if i.consume_key(egui::Modifiers::NONE, *key) {
                    state.selected = n;
                    picked = Some(n);
                }
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::Enter) {
                picked = Some(state.selected);
            }
        });
    }
    if keys && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
        return Some(Answer::Deny(String::new()));
    }

    for (n, (label, _)) in choices.iter().enumerate() {
        if choice_row(ui, n, state.selected == n, |ui| {
            ui.label(label);
        })
        .clicked()
        {
            picked = Some(n);
        }
    }
    let mut submit_feedback = false;
    let row = choice_row(ui, no, state.selected == no, |ui| {
        let r = ui.add(
            egui::TextEdit::singleline(&mut state.feedback)
                .id(field_id)
                .hint_text(no_hint)
                .desired_width(f32::INFINITY)
                .frame(egui::Frame::NONE),
        );
        if r.has_focus() {
            state.selected = no;
        }
        submit_feedback = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
    });
    if row.clicked() {
        picked = Some(no);
    }
    ui.label(RichText::new("1–9 or ↑↓ Enter to choose · Esc to say no and stop").small().weak());

    if submit_feedback {
        return Some(Answer::Deny(std::mem::take(&mut state.feedback)));
    }
    match picked {
        Some(n) if n < no => Some(choices.into_iter().nth(n).unwrap().1),
        // Picking "No" with nothing typed yet moves to the field; with text it sends.
        Some(_) if state.feedback.trim().is_empty() && !field_focused => {
            state.selected = no;
            ui.memory_mut(|m| m.request_focus(field_id));
            None
        }
        Some(_) => Some(Answer::Deny(std::mem::take(&mut state.feedback))),
        None => None,
    }
}

/// One numbered, clickable choice, highlighted while selected.
fn choice_row(ui: &mut egui::Ui, n: usize, selected: bool, add: impl FnOnce(&mut egui::Ui)) -> egui::Response {
    let fill = if selected { theme::p().surface0 } else { egui::Color32::TRANSPARENT };
    let stroke = if selected { egui::Stroke::new(1.0, theme::p().peach) } else { egui::Stroke::NONE };
    let r = egui::Frame::new()
        .fill(fill)
        .stroke(stroke)
        .corner_radius(4.0)
        .inner_margin(egui::Margin::symmetric(8, 4))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("{}", n + 1)).monospace().color(theme::p().overlay));
                add(ui);
            });
        })
        .response;
    r.interact(egui::Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// `AskUserQuestion`: each question with its options and an "Other" field.
fn questions(ui: &mut egui::Ui, id: egui::Id, req: &Request, state: &mut State) -> Option<Answer> {
    let qs = req.input["questions"].as_array().cloned().unwrap_or_default();
    state.picks.resize(qs.len(), Vec::new());
    state.other.resize(qs.len(), String::new());
    header(ui, &title(req.tool, req.input), req.queued);
    let single = qs.len() == 1 && qs[0]["multiSelect"] != true;
    let typing = ui.ctx().egui_wants_keyboard_input();

    let max = ui.ctx().content_rect().height() * 0.5;
    let mut quick = None;
    egui::ScrollArea::vertical().id_salt(id.with("questions")).max_height(max).show(ui, |ui| {
        for (q, question) in qs.iter().enumerate() {
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                if let Some(h) = question["header"].as_str() {
                    ui.label(RichText::new(h).small().color(theme::p().blue));
                }
                ui.label(RichText::new(question["question"].as_str().unwrap_or("")).strong());
            });
            let multi = question["multiSelect"] == true;
            let options = question["options"].as_array().cloned().unwrap_or_default();
            for (n, option) in options.iter().enumerate() {
                let picked = state.picks[q].contains(&n);
                let r = choice_row(ui, n, picked, |ui| {
                    ui.vertical(|ui| {
                        ui.label(option["label"].as_str().unwrap_or(""));
                        if let Some(d) = option["description"].as_str().filter(|d| !d.is_empty()) {
                            ui.label(RichText::new(d).small().weak());
                        }
                    });
                });
                if r.clicked() {
                    toggle(&mut state.picks[q], n, multi);
                    state.other[q].clear();
                    if single {
                        quick = Some(());
                    }
                }
            }
            let other_id = id.with(("other", q));
            let r = choice_row(ui, options.len(), !state.other[q].trim().is_empty(), |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut state.other[q])
                        .id(other_id)
                        .hint_text("Other: type your own answer")
                        .desired_width(f32::INFINITY)
                        .frame(egui::Frame::NONE),
                );
            });
            if r.clicked() {
                ui.memory_mut(|m| m.request_focus(other_id));
            }
            if !state.other[q].trim().is_empty() && !multi {
                state.picks[q].clear();
            }
            if single && ui.memory(|m| m.has_focus(other_id)) && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                quick = Some(());
            }
        }
    });

    // A single single-choice question answers with one key, like the CLI.
    if single && !typing {
        let count = qs[0]["options"].as_array().map_or(0, Vec::len);
        ui.input_mut(|i| {
            for (n, key) in DIGITS.iter().enumerate().take(count) {
                if i.consume_key(egui::Modifiers::NONE, *key) {
                    state.picks[0] = vec![n];
                    quick = Some(());
                }
            }
        });
    }
    if !typing && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
        return Some(Answer::Deny(String::new()));
    }

    let answers: Vec<Option<String>> = qs
        .iter()
        .enumerate()
        .map(|(q, question)| {
            let other = state.other[q].trim();
            if !other.is_empty() {
                return Some(other.to_string());
            }
            let labels: Vec<&str> =
                state.picks[q].iter().filter_map(|&n| question["options"][n]["label"].as_str()).collect();
            (!labels.is_empty()).then(|| labels.join(", "))
        })
        .collect();
    let complete = answers.iter().all(Option::is_some);

    ui.add_space(6.0);
    let mut submit = quick.is_some() && complete;
    let mut skip = false;
    ui.horizontal(|ui| {
        if !single {
            submit |= ui.add_enabled(complete, egui::Button::new("Submit answers")).clicked();
        }
        skip = ui.button("Skip").on_hover_text("Don't answer; Claude stops (Esc)").clicked();
        ui.label(RichText::new(if single { "1–9 to choose · Esc to skip" } else { "Esc to skip" }).small().weak());
    });
    if skip {
        return Some(Answer::Deny(String::new()));
    }
    if !submit {
        return None;
    }
    let map = qs
        .iter()
        .zip(answers)
        .filter_map(|(question, a)| Some((question["question"].as_str()?.to_string(), Value::String(a?))))
        .collect();
    Some(Answer::Questions(map))
}

fn toggle(picks: &mut Vec<usize>, n: usize, multi: bool) {
    if !multi {
        *picks = vec![n];
    } else if let Some(at) = picks.iter().position(|&p| p == n) {
        picks.remove(at);
    } else {
        picks.push(n);
    }
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

fn code_frame<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .fill(ui.visuals().extreme_bg_color)
        .corner_radius(4.0)
        .inner_margin(8.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// What the tool will do: the command, the edit as a diff, the file to write…
fn preview(ui: &mut egui::Ui, tool: &str, input: &Value) {
    let max = ui.ctx().content_rect().height() * 0.35;
    code_frame(ui, |ui| {
        egui::ScrollArea::vertical().id_salt(("prompt-preview", tool)).max_height(max).show(ui, |ui| {
            match tool {
                "Bash" | "PowerShell" => {
                    ui.label(RichText::new(input["command"].as_str().unwrap_or("")).monospace());
                }
                "Edit" => {
                    path_line(ui, input);
                    diff(ui, input["old_string"].as_str().unwrap_or(""), input["new_string"].as_str().unwrap_or(""));
                }
                "MultiEdit" => {
                    path_line(ui, input);
                    for (n, e) in input["edits"].as_array().into_iter().flatten().enumerate() {
                        if n > 0 {
                            ui.separator();
                        }
                        diff(ui, e["old_string"].as_str().unwrap_or(""), e["new_string"].as_str().unwrap_or(""));
                    }
                }
                "Write" => {
                    path_line(ui, input);
                    diff(ui, "", input["content"].as_str().unwrap_or(""));
                }
                "NotebookEdit" => {
                    path_line(ui, input);
                    diff(ui, "", input["new_source"].as_str().unwrap_or(""));
                }
                "Read" => path_line(ui, input),
                "WebFetch" => {
                    ui.label(RichText::new(input["url"].as_str().unwrap_or("")).monospace());
                    if let Some(p) = input["prompt"].as_str() {
                        ui.label(RichText::new(p).weak());
                    }
                }
                _ => {
                    ui.label(RichText::new(serde_json::to_string_pretty(input).unwrap_or_default()).monospace().small());
                }
            }
        });
    });
}

fn path_line(ui: &mut egui::Ui, input: &Value) {
    if let Some(p) = input["file_path"].as_str().or(input["notebook_path"].as_str()) {
        ui.label(RichText::new(p).monospace().small().weak());
    }
}

/// A line diff: the shared lines at either end shown as (up to 3 lines of)
/// context, the rest as removed and added.
fn diff(ui: &mut egui::Ui, old: &str, new: &str) {
    const CONTEXT: usize = 3;
    const MAX_LINES: usize = 400;
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    let head = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let tail = a[head..].iter().rev().zip(b[head..].iter().rev()).take_while(|(x, y)| x == y).count();
    let p = theme::p();
    let mut lines: Vec<(char, &str, egui::Color32)> = Vec::new();
    for l in &a[head.saturating_sub(CONTEXT)..head] {
        lines.push((' ', l, p.subtext));
    }
    for l in &a[head..a.len() - tail] {
        lines.push(('-', l, p.red));
    }
    for l in &b[head..b.len() - tail] {
        lines.push(('+', l, p.green));
    }
    for l in &a[a.len() - tail..(a.len() - tail + CONTEXT).min(a.len())] {
        lines.push((' ', l, p.subtext));
    }
    let more = lines.len().saturating_sub(MAX_LINES);
    for (sign, line, color) in lines.into_iter().take(MAX_LINES) {
        ui.label(RichText::new(format!("{sign} {line}")).monospace().small().color(color));
    }
    if more > 0 {
        ui.label(RichText::new(format!("… {more} more lines")).small().weak());
    }
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
}
