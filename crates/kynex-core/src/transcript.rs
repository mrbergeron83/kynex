//! How a session's items are laid out as transcript rows: each tool result
//! folded into its call (by `tool_use_id`), and runs of tool calls with no
//! text between them gathered into one row with a summary, e.g. "Edited
//! app.rs ×3, read 2 files". The app draws the rows; these are the rules.

use std::path::Path;

use serde_json::Value;

use crate::session::Item;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
    /// One item drawn on its own: a message, a result, an error, a prompt
    /// record, a lone "Thinking…", or a tool result whose call isn't known.
    Item(usize),
    /// Consecutive tool calls; a single call draws as just its own row.
    Tools(Run),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    pub calls: Vec<Call>,
    /// What the run did, e.g. "Edited app.rs ×3, read 2 files".
    pub summary: String,
}

impl Run {
    /// The index of its first call: a stable key, since items only grow.
    pub fn first(&self) -> usize {
        self.calls[0].ix
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Call {
    /// The `ToolUse` item.
    pub ix: usize,
    /// Its `ToolResult`, once it came back.
    pub result: Option<usize>,
    /// The permission prompt it raised, if any.
    pub permission: Option<usize>,
}

/// The rows for `items`. Cheap enough to redo whenever an item is added.
pub fn rows(items: &[Item]) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut run: Vec<Call> = Vec::new();
    // "Thinking…" placeholders: they join a run if a tool call follows.
    let mut thinking: Vec<usize> = Vec::new();
    fn close(rows: &mut Vec<Row>, run: &mut Vec<Call>, items: &[Item]) {
        if !run.is_empty() {
            let calls = std::mem::take(run);
            let summary = summary(items, &calls);
            rows.push(Row::Tools(Run { calls, summary }));
        }
    }
    for (ix, item) in items.iter().enumerate() {
        match item {
            Item::Thinking => thinking.push(ix),
            Item::ToolUse { .. } => {
                thinking.clear();
                run.push(Call { ix, result: None, permission: None });
            }
            Item::ToolResult { id, .. } => {
                if let Some(call) = find_call(items, &mut run, &mut rows, id) {
                    call.result = Some(ix);
                    continue;
                }
                close(&mut rows, &mut run, items);
                rows.extend(thinking.drain(..).map(Row::Item));
                rows.push(Row::Item(ix));
            }
            // A prompt for a call in this run stays with it.
            Item::Permission { tool, .. } if !run.is_empty() => {
                let call = run.iter_mut().rev().find(|c| {
                    c.result.is_none()
                        && c.permission.is_none()
                        && matches!(&items[c.ix], Item::ToolUse { name, .. } if name == tool)
                });
                match call {
                    Some(call) => call.permission = Some(ix),
                    None => {
                        close(&mut rows, &mut run, items);
                        rows.push(Row::Item(ix));
                    }
                }
            }
            _ => {
                close(&mut rows, &mut run, items);
                rows.extend(thinking.drain(..).map(Row::Item));
                rows.push(Row::Item(ix));
            }
        }
    }
    close(&mut rows, &mut run, items);
    rows.extend(thinking.into_iter().map(Row::Item));
    rows
}

impl Row {
    /// Whether item `ix` is drawn in this row: the item itself, or a call, its
    /// result or its prompt in a run.
    pub fn holds(&self, ix: usize) -> bool {
        match self {
            Row::Item(i) => *i == ix,
            Row::Tools(run) => run.calls.iter().any(|c| c.ix == ix || c.result == Some(ix) || c.permission == Some(ix)),
        }
    }

    /// The first item drawn in this row.
    pub fn first_item(&self) -> usize {
        match self {
            Row::Item(i) => *i,
            Row::Tools(run) => run.first(),
        }
    }
}

/// The row that draws item `ix`.
pub fn row_of(rows: &[Row], ix: usize) -> Option<usize> {
    rows.iter().position(|r| r.holds(ix))
}

/// The call named `id`, in the open run or a recent one (a result can arrive
/// after a prompt cut its run short). Older rows aren't searched, so a long
/// transcript stays linear.
fn find_call<'a>(items: &[Item], run: &'a mut [Call], rows: &'a mut [Row], id: &str) -> Option<&'a mut Call> {
    if id.is_empty() {
        return None;
    }
    let named = |c: &Call| c.result.is_none() && matches!(&items[c.ix], Item::ToolUse { id: i, .. } if i == id);
    if let Some(at) = run.iter().position(named) {
        return Some(&mut run[at]);
    }
    rows.iter_mut().rev().take(8).find_map(|r| match r {
        Row::Tools(r) => r.calls.iter_mut().find(|c| named(c)),
        Row::Item(_) => None,
    })
}

/// File edits, whose success message says nothing the diff doesn't.
pub fn edits(tool: &str) -> bool {
    matches!(tool, "Edit" | "MultiEdit" | "Write" | "NotebookEdit")
}

/// Whether a result is worth showing: errors always, an edit's "updated
/// successfully" never.
pub fn shows_result(tool: &str, is_error: bool) -> bool {
    is_error || !edits(tool)
}

/// What a run did: "Edited app.rs ×3", "Read 2 files, ran a command".
fn summary(items: &[Item], calls: &[Call]) -> String {
    // (verb, noun, targets in order) per kind of call, in order of first use.
    let mut kinds: Vec<(&str, &str, Vec<String>)> = Vec::new();
    for call in calls {
        let Item::ToolUse { name, input, .. } = &items[call.ix] else { continue };
        let v: Value = serde_json::from_str(input).unwrap_or_default();
        let file = || {
            let path = v["file_path"].as_str().or(v["notebook_path"].as_str()).unwrap_or("");
            Path::new(path).file_name().map_or_else(|| path.to_string(), |f| f.to_string_lossy().into_owned())
        };
        let (verb, noun, target) = match name.as_str() {
            n if edits(n) => ("edited", "file", file()),
            "Read" => ("read", "file", file()),
            "Grep" | "Glob" => ("searched", "pattern", v["pattern"].as_str().unwrap_or("").to_string()),
            "Bash" | "PowerShell" => ("ran", "command", String::new()),
            "WebFetch" | "WebSearch" => ("looked up", "page", String::new()),
            "Task" | "Agent" => ("ran", "agent", String::new()),
            // An MCP server's calls count together, screenshots marked: "Edge: 14 actions (3 screenshots)".
            n => match crate::mcp::split(n) {
                Some((server, tool)) => {
                    ("mcp", server, if tool == "browser_take_screenshot" { "shot" } else { "" }.into())
                }
                None => ("used", n, String::new()),
            },
        };
        match kinds.iter_mut().find(|k| k.0 == verb && k.1 == noun) {
            Some(k) => k.2.push(target),
            None => kinds.push((verb, noun, vec![target])),
        }
    }
    let parts: Vec<String> = kinds
        .iter()
        .map(|(verb, noun, targets)| {
            let n = targets.len();
            let mut distinct: Vec<&String> = Vec::new();
            for t in targets {
                if !distinct.contains(&t) {
                    distinct.push(t);
                }
            }
            match (*noun, distinct.as_slice()) {
                // Any tool's name reads as itself: "used Foo ×2".
                (_, _) if *verb == "used" => times(format!("used {noun}"), n),
                (server, _) if *verb == "mcp" => {
                    crate::mcp::run_summary(server, n, targets.iter().filter(|t| *t == "shot").count())
                }
                ("file", [one]) if !one.is_empty() => times(format!("{verb} {one}"), n),
                ("file", files) => format!("{verb} {}", count(files.len(), noun)),
                _ => format!("{verb} {}", count(n, noun)),
            }
        })
        .collect();
    let text = parts.join(", ");
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |c| c.to_uppercase().chain(chars).collect())
}

/// "a file", "3 files".
fn count(n: usize, noun: &str) -> String {
    match n {
        1 if noun.starts_with(['a', 'e', 'i', 'o', 'u']) => format!("an {noun}"),
        1 => format!("a {noun}"),
        _ => format!("{n} {noun}s"),
    }
}

/// "edited app.rs", "edited app.rs ×3".
fn times(text: String, n: usize) -> String {
    if n > 1 { format!("{text} ×{n}") } else { text }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::PermState;

    fn call(id: &str, name: &str, input: Value) -> Item {
        let input = input.to_string();
        Item::ToolUse { id: id.into(), name: name.into(), summary: String::new(), input }
    }

    fn result(id: &str, is_error: bool) -> Item {
        Item::ToolResult { id: id.into(), content: "ok".into(), images: Vec::new(), is_error }
    }

    fn edit(id: &str, path: &str) -> Item {
        call(id, "Edit", serde_json::json!({ "file_path": path }))
    }

    fn run(row: &Row) -> &Run {
        match row {
            Row::Tools(r) => r,
            Row::Item(ix) => panic!("item {ix}, not a run"),
        }
    }

    #[test]
    fn results_fold_into_their_calls() {
        // Parallel calls come first, their results after, in any order.
        let items = [
            Item::Assistant("Looking".into()),
            edit("a", "/src/app.rs"),
            edit("b", "/src/app.rs"),
            result("b", false),
            result("a", true),
            Item::Assistant("Done".into()),
        ];
        let rows = rows(&items);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0], Row::Item(0));
        let r = run(&rows[1]);
        assert_eq!(
            r.calls,
            [Call { ix: 1, result: Some(4), permission: None }, Call { ix: 2, result: Some(3), permission: None }]
        );
        assert_eq!(r.summary, "Edited app.rs ×2");
        assert_eq!(rows[2], Row::Item(5));

        // Every item is found in the row that draws it.
        assert_eq!(row_of(&rows, 0), Some(0));
        assert_eq!(row_of(&rows, 3), Some(1), "a result is in its call's row");
        assert_eq!(row_of(&rows, 2), Some(1));
        assert_eq!(row_of(&rows, 5), Some(2));
        assert_eq!(row_of(&rows, 9), None);
        assert_eq!(rows[1].first_item(), 1);
    }

    #[test]
    fn thinking_between_calls_joins_the_run() {
        let items = [
            Item::Thinking,
            edit("a", "a.rs"),
            result("a", false),
            Item::Thinking,
            edit("b", "b.rs"),
            result("b", false),
            Item::Thinking,
        ];
        let rows = rows(&items);
        assert_eq!(rows.len(), 2, "{rows:?}");
        assert_eq!(run(&rows[0]).calls.len(), 2);
        assert_eq!(rows[1], Row::Item(6), "a trailing Thinking still shows");
    }

    #[test]
    fn prompts_stay_with_their_call() {
        let items = [
            call("a", "Bash", serde_json::json!({ "command": "ls" })),
            Item::Permission {
                request_id: "r".into(),
                tool: "Bash".into(),
                input: Value::Null,
                suggestions: Value::Null,
                description: String::new(),
                state: PermState::Pending,
            },
            result("a", false),
            call("b", "Read", serde_json::json!({ "file_path": "x.rs" })),
        ];
        let rows = rows(&items);
        assert_eq!(rows.len(), 1);
        let r = run(&rows[0]);
        assert_eq!(r.calls[0], Call { ix: 0, result: Some(2), permission: Some(1) });
        assert_eq!(r.calls[1].result, None);
        assert_eq!(r.summary, "Ran a command, read x.rs");
    }

    #[test]
    fn unknown_results_get_their_own_row() {
        let items = [result("", false), result("nope", true)];
        assert_eq!(rows(&items), [Row::Item(0), Row::Item(1)]);
    }

    #[test]
    fn summaries() {
        let s = |items: &[Item]| run(&rows(items)[0]).summary.clone();
        let read = |id: &str, p: &str| call(id, "Read", serde_json::json!({ "file_path": p }));
        let grep = |id: &str| call(id, "Grep", serde_json::json!({ "pattern": id }));
        assert_eq!(s(&[edit("a", "a.rs")]), "Edited a.rs");
        assert_eq!(s(&[edit("a", "a.rs"), edit("b", "b.rs"), read("c", "a.rs")]), "Edited 2 files, read a.rs");
        assert_eq!(
            s(&[read("a", "a.rs"), read("b", "b.rs"), grep("c"), grep("d")]),
            "Read 2 files, searched 2 patterns"
        );
        let mcp = call("a", "mcp__x__y", Value::Null);
        assert_eq!(s(&[mcp, call("b", "mcp__x__y", Value::Null)]), "X: 2 actions");
        let browse = |id: &str, tool: &str| call(id, &format!("mcp__playwright-edge__browser_{tool}"), Value::Null);
        assert_eq!(
            s(&[browse("a", "navigate"), browse("b", "take_screenshot"), browse("c", "click")]),
            "Edge: 3 actions (1 screenshot)"
        );
        assert_eq!(s(&[call("a", "Foo", Value::Null), call("b", "Foo", Value::Null)]), "Used Foo ×2");
        assert_eq!(s(&[call("a", "Task", Value::Null)]), "Ran an agent");
    }

    #[test]
    fn edit_results_say_nothing() {
        assert!(!shows_result("Edit", false));
        assert!(shows_result("Edit", true));
        assert!(shows_result("Bash", false));
    }
}
