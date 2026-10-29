//! What a request will do, in words and as a preview: titles, the "don't ask
//! again" wording, and diffs of the edits it makes.

use gpui::{StyledText, div, prelude::*, px};
use serde_json::Value;

use kynex_core::highlight::{Lang, Side};
use kynex_core::{mcp, project};

use crate::theme::{Palette, mono};
use crate::ui::code;

/// The headline for a request, e.g. "Allow Claude to run this command?".
pub fn title(tool: &str, input: &Value) -> String {
    let file = || input["file_path"].as_str().or(input["notebook_path"].as_str()).map(file_name).unwrap_or("this file");
    match tool {
        "Bash" | "PowerShell" => "Allow Claude to run this command?".into(),
        "Edit" | "MultiEdit" | "NotebookEdit" => format!("Make this edit to {}?", file()),
        "Write" => format!("Allow Claude to write {}?", file()),
        "Read" => format!("Allow Claude to read {}?", file()),
        "WebFetch" => format!("Allow Claude to fetch {}?", input["url"].as_str().unwrap_or("this page")),
        "WebSearch" => {
            format!("Allow Claude to search the web for \u{201c}{}\u{201d}?", input["query"].as_str().unwrap_or(""))
        }
        "ExitPlanMode" => "Ready to code? Here is Claude's plan:".into(),
        "AskUserQuestion" => "Claude has a question".into(),
        // MCP calls say what they'll do: `Click "Delete repository" in Edge?`.
        _ => mcp::ask(tool, input).unwrap_or_else(|| format!("Allow Claude to use {tool}?")),
    }
}

fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().filter(|n| !n.is_empty()).unwrap_or(path)
}

/// The "don't ask again this session" choice, worded after what it unlocks.
pub(super) fn session_label(suggestions: &Value) -> Option<String> {
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
                let dirs: Vec<&str> =
                    s["directories"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
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
pub(super) fn rules_label(rules: &[String]) -> String {
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

/// The (old, new) text pairs a file-editing tool call changes; `None` for
/// tools that don't edit files.
fn edits(tool: &str, input: &Value) -> Option<Vec<(String, String)>> {
    let s = |v: &Value| v.as_str().unwrap_or("").to_string();
    Some(match tool {
        "Edit" => vec![(s(&input["old_string"]), s(&input["new_string"]))],
        "MultiEdit" => input["edits"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|e| (s(&e["old_string"]), s(&e["new_string"])))
            .collect(),
        "Write" => vec![(String::new(), s(&input["content"]))],
        "NotebookEdit" => vec![(String::new(), s(&input["new_source"]))],
        _ => return None,
    })
}

/// Lines added and removed by a file-editing tool call, or `None` for other tools.
pub fn diff_stat(tool: &str, input: &Value) -> Option<(usize, usize)> {
    let lines = edits(tool, input)?.iter().flat_map(|(old, new)| diff(old, new)).collect::<Vec<_>>();
    let count = |sign| lines.iter().filter(|(s, _)| *s == sign).count();
    Some((count('+'), count('-')))
}

/// What the tool will do: the command, the edit as a diff, the file to write…
pub fn preview(tool: &str, input: &Value, t: &Palette) -> impl IntoElement + use<> {
    let code = |s: &str| div().font_family(mono()).text_size(px(13.)).child(s.to_string());
    let path_line = |input: &Value| {
        input["file_path"]
            .as_str()
            .or(input["notebook_path"].as_str())
            .map(|p| div().font_family(mono()).text_xs().text_color(t.text_muted).pb_1().child(p.to_string()))
    };
    let el = div().flex().flex_col();
    let path = input["file_path"].as_str();
    match tool {
        "Bash" | "PowerShell" => el.child(code(input["command"].as_str().unwrap_or(""))),
        "Edit" => el.children(path_line(input)).child(diff_lines(
            path,
            input["old_string"].as_str().unwrap_or(""),
            input["new_string"].as_str().unwrap_or(""),
            t,
        )),
        "MultiEdit" => el.children(path_line(input)).children(
            input["edits"].as_array().into_iter().flatten().enumerate().map(|(n, e)| {
                let (old, new) = (e["old_string"].as_str().unwrap_or(""), e["new_string"].as_str().unwrap_or(""));
                div()
                    .when(n > 0, |d| d.mt_2().pt_2().border_t_1().border_color(t.border_variant))
                    .child(diff_lines(path, old, new, t))
            }),
        ),
        "Write" => {
            el.children(path_line(input)).child(diff_lines(path, "", input["content"].as_str().unwrap_or(""), t))
        }
        "NotebookEdit" => {
            el.children(path_line(input)).child(diff_lines(None, "", input["new_source"].as_str().unwrap_or(""), t))
        }
        "Read" => el.children(path_line(input)),
        "WebFetch" => el.child(code(input["url"].as_str().unwrap_or(""))).children(
            input["prompt"].as_str().map(|p| div().pt_1().text_sm().text_color(t.text_muted).child(p.to_string())),
        ),
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

/// The edit as diff lines, highlighted for `path`'s language, on the add / remove colours.
fn diff_lines(path: Option<&str>, old: &str, new: &str, t: &Palette) -> impl IntoElement + use<> {
    const MAX_LINES: usize = 400;
    let mut lines = diff(old, new);
    let more = lines.len().saturating_sub(MAX_LINES);
    lines.truncate(MAX_LINES);
    let tokens = path.and_then(Lang::from_path).map(|lang| {
        let sides: Vec<_> = lines.iter().map(|(sign, line)| (side(*sign), line.as_str())).collect();
        code::diff_tokens(lang, &sides)
    });
    let syntax = t.syntax;
    div()
        .flex()
        .flex_col()
        .font_family(mono())
        .text_size(px(12.))
        .line_height(px(18.))
        .children(lines.into_iter().enumerate().map(move |(i, (sign, line))| {
            let styles = tokens.as_ref().map(|all| code::styles(&all[i], &syntax)).unwrap_or_default();
            let text = StyledText::new(line).with_highlights(styles);
            let (bg, mark) = match sign {
                '+' => (Some(t.diff_add), t.success),
                '-' => (Some(t.diff_del), t.error),
                _ => (None, t.text_placeholder),
            };
            div()
                .flex()
                .min_w_full()
                .whitespace_nowrap()
                .text_color(t.text)
                .when_some(bg, |d, bg| d.bg(bg))
                .child(div().w(px(14.)).flex_none().flex().justify_center().text_color(mark).child(sign.to_string()))
                .child(div().pr_2().child(text))
        }))
        .when(more > 0, |d| {
            d.child(div().px_2().py_1().text_xs().text_color(t.text_muted).child(format!("… {more} more lines")))
        })
}

/// Which side of a diff a line of [`diff`] is on.
fn side(sign: char) -> Side {
    match sign {
        '+' => Side::New,
        '-' => Side::Old,
        _ => Side::Both,
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
        assert_eq!(
            title("mcp__playwright-edge__browser_navigate", &json!({ "url": "https://x.dev" })),
            "Open https://x.dev in Edge?"
        );
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

    #[test]
    fn diff_stat_counts_edited_lines() {
        let edit = json!({ "file_path": "a.rs", "old_string": "x\ny", "new_string": "x\nY\nZ" });
        assert_eq!(diff_stat("Edit", &edit), Some((2, 1)));
        let multi =
            json!({ "edits": [{ "old_string": "a", "new_string": "b" }, { "old_string": "c", "new_string": "" }] });
        assert_eq!(diff_stat("MultiEdit", &multi), Some((1, 2)));
        assert_eq!(diff_stat("Write", &json!({ "content": "1\n2\n3" })), Some((3, 0)));
        assert_eq!(diff_stat("Bash", &json!({ "command": "ls" })), None);
    }
}
