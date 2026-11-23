//! How the chat transcript's rows are gathered for display: tool calls with no
//! text between them form one run under a summary row ("Edited app.rs ×3, ran
//! 2 commands"), thinking in between included. The app keeps its rows flat and
//! draws the list from [`group`]'s items.

use std::ops::Range;

/// What a transcript row is, as far as grouping goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowKind {
    /// A tool call (or a group of reads/searches drawn as one row).
    Tool,
    /// A thinking block: joins a run when a tool call follows it.
    Thought,
    /// Anything else (a message, prose, a notice) ends a run.
    Other,
}

/// One entry of the drawn list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ListItem {
    /// Row `n`, drawn on its own.
    Row(usize),
    /// Rows `range`, a run of at least two tool calls (with any thinking between
    /// them), drawn under one summary.
    Run(Range<usize>),
}

impl ListItem {
    /// The first row it draws.
    pub fn start(&self) -> usize {
        match self {
            Self::Row(n) => *n,
            Self::Run(r) => r.start,
        }
    }
}

/// The list items for rows of `kinds`. Linear; cheap to redo on every change.
pub fn group(kinds: &[RowKind]) -> Vec<ListItem> {
    let mut out = Vec::new();
    // The open run: its first row, its last tool row, and how many tool rows.
    let mut run: Option<(usize, usize, usize)> = None;
    let close = |out: &mut Vec<ListItem>, run: &mut Option<(usize, usize, usize)>, upto: usize| {
        if let Some((start, last, tools)) = run.take() {
            if tools >= 2 {
                out.push(ListItem::Run(start..last + 1));
            } else {
                out.extend((start..=last).map(ListItem::Row));
            }
            // Thinking after the run's last call stays on its own.
            out.extend((last + 1..upto).map(ListItem::Row));
        }
    };
    for (i, kind) in kinds.iter().enumerate() {
        match kind {
            RowKind::Tool => match &mut run {
                Some((_, last, tools)) => {
                    *last = i;
                    *tools += 1;
                }
                None => run = Some((i, i, 1)),
            },
            RowKind::Thought if run.is_some() => {}
            RowKind::Thought | RowKind::Other => {
                close(&mut out, &mut run, i);
                out.push(ListItem::Row(i));
            }
        }
    }
    close(&mut out, &mut run, kinds.len());
    out
}

/// What a run did, from its calls as `(tool name, target)`, the target being a
/// file name for edits and reads and the pattern for searches: "Edited app.rs
/// ×3, read 2 files, ran a command".
pub fn summary(calls: &[(&str, &str)]) -> String {
    // (verb, noun, targets in order) per kind of call, in order of first use.
    let mut kinds: Vec<(&str, String, Vec<&str>)> = Vec::new();
    for &(name, target) in calls {
        let (verb, noun): (&str, String) = match name {
            "Edit" | "MultiEdit" | "Write" | "NotebookEdit" => ("edited", "file".into()),
            "Read" | "NotebookRead" => ("read", "file".into()),
            "Grep" | "Glob" | "Search" | "ToolSearch" => ("searched", "pattern".into()),
            "LS" | "ListDir" => ("listed", "folder".into()),
            "Bash" | "PowerShell" | "Shell" | "Terminal" => ("ran", "command".into()),
            "WebFetch" | "WebSearch" => ("looked up", "page".into()),
            "Task" | "Agent" => ("ran", "agent".into()),
            n => match n.strip_prefix("mcp__").and_then(|rest| rest.split_once("__")) {
                Some((server, _)) => ("used", server.to_string()),
                None => ("used", n.to_string()),
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
            let mut distinct: Vec<&str> = Vec::new();
            for t in targets {
                if !distinct.contains(t) {
                    distinct.push(t);
                }
            }
            match (noun.as_str(), distinct.as_slice()) {
                _ if *verb == "used" => times(format!("used {noun}"), n),
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

/// "a file", "an agent", "3 files".
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
    use super::RowKind::{Other as O, Thought as H, Tool as T};
    use super::*;

    #[test]
    fn consecutive_calls_form_one_run() {
        assert_eq!(group(&[O, T, T, T, O]), vec![ListItem::Row(0), ListItem::Run(1..4), ListItem::Row(4)]);
    }

    #[test]
    fn a_lone_call_stays_a_row() {
        assert_eq!(group(&[O, T, O, T]), vec![ListItem::Row(0), ListItem::Row(1), ListItem::Row(2), ListItem::Row(3)]);
    }

    #[test]
    fn thinking_between_calls_joins_but_not_around_them() {
        assert_eq!(
            group(&[H, T, H, T, H, O]),
            vec![ListItem::Row(0), ListItem::Run(1..4), ListItem::Row(4), ListItem::Row(5)]
        );
        // Thinking after a lone call isn't swallowed either.
        assert_eq!(group(&[T, H]), vec![ListItem::Row(0), ListItem::Row(1)]);
    }

    #[test]
    fn a_growing_run_keeps_its_start() {
        let before = group(&[O, T, T]);
        let after = group(&[O, T, T, H, T]);
        assert_eq!(before, vec![ListItem::Row(0), ListItem::Run(1..3)]);
        assert_eq!(after, vec![ListItem::Row(0), ListItem::Run(1..5)]);
        assert!(group(&[]).is_empty());
    }

    #[test]
    fn summaries_read_like_the_old_transcript() {
        assert_eq!(summary(&[("Edit", "app.rs"), ("Edit", "app.rs"), ("Edit", "app.rs")]), "Edited app.rs ×3");
        assert_eq!(
            summary(&[("Read", "a.rs"), ("Read", "b.rs"), ("Bash", ""), ("Edit", "a.rs")]),
            "Read 2 files, ran a command, edited a.rs"
        );
        assert_eq!(summary(&[("Bash", ""), ("PowerShell", "")]), "Ran 2 commands");
        assert_eq!(summary(&[("Agent", "")]), "Ran an agent");
        assert_eq!(summary(&[("mcp__playwright__click", ""), ("mcp__playwright__type", "")]), "Used playwright ×2");
        assert_eq!(summary(&[("Grep", "fn main"), ("Glob", "*.rs")]), "Searched 2 patterns");
        assert_eq!(summary(&[]), "");
    }
}
