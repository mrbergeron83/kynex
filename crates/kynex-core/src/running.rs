//! The Agents pane's list: what is running or wants you, across every open project. Claude
//! and Grok sessions that work, wait on you or finished unseen, sessions with background
//! tasks, and the other agent CLIs in terminal panes. Idle ones aren't listed.
//!
//! Waiting first, then finished, then working; inside each, the order things were opened
//! in, so rows don't jump as replies stream. A child session sits under its parent,
//! wherever each runs.

use std::collections::HashSet;

use crate::layout::PaneId;
use crate::live::Key;
use crate::session::{Item, PermState, Session};

/// Where a row stands, in the order rows are listed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum State {
    /// A permission, a question, a plan, or the browser waits on you.
    Waiting,
    /// A turn ended and you haven't looked at it.
    Finished,
    /// Working, or background tasks still running.
    Working,
}

impl State {
    /// The state of something with these flags, or `None` when it's idle (not listed).
    /// A question waiting wins over the rest; work under way over a finished turn.
    pub fn of(waiting: bool, busy: bool, finished: bool, tasks: bool) -> Option<Self> {
        if waiting {
            Some(Self::Waiting)
        } else if busy {
            Some(Self::Working)
        } else if finished {
            Some(Self::Finished)
        } else if tasks {
            Some(Self::Working)
        } else {
            None
        }
    }

    /// A session's state: [`Self::of`] with the browser handoff counting as waiting.
    pub fn of_session(s: &Session) -> Option<Self> {
        let browser = !s.busy && crate::mcp::handoff(&s.items).is_some();
        Self::of(s.waiting() || browser, s.busy, s.finished, s.has_tasks())
    }
}

/// What a row is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Who {
    /// An open Claude or Grok session.
    Session(Key),
    /// Another agent CLI, in a terminal pane.
    Agent(PaneId),
}

/// One thing running, as the pane lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub who: Who,
    pub state: State,
    /// Its place in the steady order: when it was opened (a session's uid, a pane's id).
    pub order: u64,
    pub name: String,
    /// Its workspace, and project when that isn't the one shown.
    pub place: String,
    /// What it's doing, in a few words.
    pub doing: String,
    /// The session that started it, if any.
    pub parent: Option<Who>,
}

/// `rows` in the order shown, each with how deep it is nested: parents first by state, then
/// steady order; children right under their parent, sorted the same way. A row whose parent
/// isn't listed stands on its own.
pub fn arrange(rows: Vec<Row>) -> Vec<(usize, Row)> {
    let listed: HashSet<Who> = rows.iter().map(|r| r.who).collect();
    let key = |r: &Row| (r.state, r.order);
    let mut roots: Vec<&Row> = rows.iter().filter(|r| r.parent.is_none_or(|p| !listed.contains(&p))).collect();
    roots.sort_by_key(|r| key(r));
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    fn walk<'a>(r: &'a Row, depth: usize, rows: &'a [Row], seen: &mut HashSet<Who>, out: &mut Vec<(usize, Row)>) {
        if !seen.insert(r.who) {
            return;
        }
        out.push((depth, r.clone()));
        let mut children: Vec<&Row> = rows.iter().filter(|c| c.parent == Some(r.who)).collect();
        children.sort_by_key(|c| (c.state, c.order));
        for c in children {
            walk(c, depth + 1, rows, seen, out);
        }
    }
    for r in roots {
        walk(r, 0, &rows, &mut seen, &mut out);
    }
    // Parents that name each other in a loop: listed once each, on their own.
    let mut left: Vec<&Row> = rows.iter().filter(|r| !seen.contains(&r.who)).collect();
    left.sort_by_key(|r| key(r));
    for r in left {
        walk(r, 0, &rows, &mut seen, &mut out);
    }
    out
}

/// What session `s` is doing, in a few words: what it waits for, the tool in use, or that
/// it finished.
pub fn doing(s: &Session) -> String {
    let pending = s.items.iter().rev().find_map(|i| match i {
        Item::Permission { tool, state: PermState::Pending, .. } => Some(tool.as_str()),
        _ => None,
    });
    if let Some(tool) = pending {
        return match tool {
            "AskUserQuestion" => "Asks you a question".into(),
            "ExitPlanMode" => "Has a plan for you".into(),
            tool => format!("Asks to use {}", tool_name(tool)),
        };
    }
    if !s.busy && crate::mcp::handoff(&s.items).is_some() {
        return "Waiting for you in the browser".into();
    }
    if s.busy {
        return match s.items.last() {
            Some(Item::ToolUse { name, summary, .. }) if summary.is_empty() => tool_name(name),
            Some(Item::ToolUse { name, summary, .. }) => short(&format!("{}: {summary}", tool_name(name))),
            Some(Item::Thinking(_)) => "Thinking".into(),
            Some(Item::Assistant(_)) => "Writing".into(),
            _ => "Working".into(),
        };
    }
    if s.finished {
        let failed = s.items.iter().rev().find_map(|i| match i {
            Item::Result { is_error, .. } => Some(*is_error),
            _ => None,
        });
        return if failed == Some(true) { "Stopped on an error".into() } else { "Finished".into() };
    }
    match s.tasks.running.len() {
        0 => String::new(),
        1 => "1 background task".into(),
        n => format!("{n} background tasks"),
    }
}

/// A tool's name as people say it: an MCP tool's own name, without its server prefix.
fn tool_name(tool: &str) -> String {
    match tool.strip_prefix("mcp__") {
        Some(rest) => rest.split_once("__").map_or(rest, |(_, t)| t).to_string(),
        None => tool.to_string(),
    }
}

/// At most a line's worth.
fn short(s: &str) -> String {
    const MAX: usize = 60;
    let line = s.lines().next().unwrap_or("");
    if line.chars().count() > MAX { format!("{}…", line.chars().take(MAX).collect::<String>()) } else { line.into() }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::session::Kind;

    fn row(who: Who, state: State, order: u64, parent: Option<Who>) -> Row {
        Row { who, state, order, name: String::new(), place: String::new(), doing: String::new(), parent }
    }

    fn order(rows: &[(usize, Row)]) -> Vec<(usize, Who)> {
        rows.iter().map(|(d, r)| (*d, r.who)).collect()
    }

    #[test]
    fn waiting_comes_first_then_finished_then_working() {
        let rows = vec![
            row(Who::Session(1), State::Working, 1, None),
            row(Who::Agent(7), State::Finished, 7, None),
            row(Who::Session(2), State::Waiting, 2, None),
        ];
        assert_eq!(order(&arrange(rows)), [(0, Who::Session(2)), (0, Who::Agent(7)), (0, Who::Session(1))]);
    }

    #[test]
    fn the_order_inside_a_state_is_steady() {
        let a = vec![row(Who::Session(3), State::Working, 3, None), row(Who::Session(1), State::Working, 1, None)];
        let mut b = a.clone();
        b.reverse();
        assert_eq!(order(&arrange(a.clone())), [(0, Who::Session(1)), (0, Who::Session(3))]);
        assert_eq!(order(&arrange(a)), order(&arrange(b)), "whatever order they come in");
    }

    #[test]
    fn a_child_sits_under_its_parent_wherever_each_runs() {
        let parent = Who::Session(1);
        let rows = vec![
            row(Who::Session(5), State::Waiting, 5, Some(parent)),
            row(Who::Session(4), State::Working, 4, Some(parent)),
            row(Who::Session(9), State::Waiting, 9, None),
            row(parent, State::Working, 1, None),
            // A parent that isn't listed (idle, or closed): the child stands on its own.
            row(Who::Session(6), State::Finished, 6, Some(Who::Session(99))),
        ];
        assert_eq!(
            order(&arrange(rows)),
            [(0, Who::Session(9)), (0, Who::Session(6)), (0, parent), (1, Who::Session(5)), (1, Who::Session(4)),]
        );
    }

    #[test]
    fn a_loop_of_parents_lists_nothing_twice() {
        let (a, b) = (Who::Session(1), Who::Session(2));
        let rows = vec![row(a, State::Working, 1, Some(b)), row(b, State::Working, 2, Some(a))];
        // Neither is a root: both have a listed parent. Each shows once, the loop broken.
        assert_eq!(order(&arrange(rows)), [(0, a), (1, b)]);
    }

    #[test]
    fn nothing_running_lists_nothing() {
        assert!(arrange(Vec::new()).is_empty());
        let s = Session::new(Kind::Chat, PathBuf::from("."));
        assert_eq!(State::of_session(&s), None, "an idle session isn't listed");
    }

    #[test]
    fn states_from_flags() {
        assert_eq!(State::of(true, true, false, false), Some(State::Waiting));
        assert_eq!(State::of(false, true, true, false), Some(State::Working));
        assert_eq!(State::of(false, false, true, true), Some(State::Finished));
        assert_eq!(State::of(false, false, false, true), Some(State::Working), "only background tasks");
        assert_eq!(State::of(false, false, false, false), None);
    }

    #[test]
    fn what_a_session_is_doing() {
        let mut s = Session::new(Kind::Chat, PathBuf::from("."));
        s.busy = true;
        s.items.push(Item::ToolUse {
            id: "t".into(),
            name: "mcp__playwright__browser_click".into(),
            summary: "Click “Sign in”".into(),
            input: String::new(),
            timing: Default::default(),
        });
        assert_eq!(doing(&s), "browser_click: Click “Sign in”");
        s.items.push(Item::Assistant("…".into()));
        assert_eq!(doing(&s), "Writing");
        s.items.push(Item::Permission {
            request_id: "r".into(),
            tool: "Bash".into(),
            input: serde_json::Value::Null,
            suggestions: serde_json::Value::Null,
            description: String::new(),
            why: String::new(),
            state: PermState::Pending,
        });
        assert_eq!(doing(&s), "Asks to use Bash");
        s.items.pop();
        s.busy = false;
        s.finished = true;
        assert_eq!(doing(&s), "Finished");
        s.finished = false;
        s.tasks.apply(&serde_json::json!({ "subtype": "task_started", "task_id": "b1", "task_type": "local_bash" }));
        assert_eq!(doing(&s), "1 background task");
        assert_eq!(State::of_session(&s), Some(State::Working));
    }
}
