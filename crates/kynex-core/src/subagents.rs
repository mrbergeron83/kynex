//! The subagents of a session's current turn, for the strip over the composer:
//! each `Agent` (or older `Task`) call since the last message you sent, which
//! agent it went to, and whether it's still working. Background agents aren't
//! here; they're running tasks ([`crate::tasks`]) and outlive the turn.

use std::time::Duration;

use serde_json::Value;

use crate::session::Item;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Working,
    Done,
    Failed,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Subagent {
    /// The call's `tool_use_id`.
    pub id: String,
    /// The agent it went to: `haiku-worker`, `Explore`… (`general-purpose` when unnamed).
    pub agent: String,
    /// Its short description of the task.
    pub description: String,
    pub state: State,
    /// How long it has run, or ran; unknown for one read back from a log.
    pub elapsed: Option<Duration>,
}

/// How many subagents a session has called in all. Doesn't parse anything,
/// since the header counts them on every frame.
pub fn count(items: &[Item]) -> usize {
    items.iter().filter(|i| matches!(i, Item::ToolUse { name, .. } if name == "Agent" || name == "Task")).count()
}

/// The subagents called since the last message you sent, in order.
pub fn of_turn(items: &[Item]) -> Vec<Subagent> {
    let start = items.iter().rposition(|i| matches!(i, Item::User { .. })).map_or(0, |i| i + 1);
    let turn = &items[start..];
    let mut out: Vec<Subagent> = Vec::new();
    for item in turn {
        match item {
            Item::ToolUse { id, name, summary, input, timing } if name == "Agent" || name == "Task" => {
                let input: Value = serde_json::from_str(input).unwrap_or(Value::Null);
                if input["run_in_background"] == true {
                    continue;
                }
                out.push(Subagent {
                    id: id.clone(),
                    agent: input["subagent_type"].as_str().unwrap_or("general-purpose").to_string(),
                    description: summary.clone(),
                    state: State::Working,
                    elapsed: timing.elapsed(),
                });
            }
            Item::ToolResult { id, is_error, .. } => {
                if let Some(s) = out.iter_mut().find(|s| &s.id == id) {
                    s.state = if *is_error { State::Failed } else { State::Done };
                }
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::Timing;

    fn call(id: &str, name: &str, input: &str) -> Item {
        Item::ToolUse {
            id: id.into(),
            name: name.into(),
            summary: format!("task {id}"),
            input: input.into(),
            timing: Timing::default(),
        }
    }

    fn result(id: &str, is_error: bool) -> Item {
        Item::ToolResult { id: id.into(), content: String::new(), images: Vec::new(), is_error }
    }

    fn user() -> Item {
        Item::User { text: "go".into(), images: Vec::new() }
    }

    #[test]
    fn lists_the_turns_agents_with_their_state() {
        let items = [
            user(),
            call("old", "Agent", r#"{"subagent_type":"Explore"}"#),
            result("old", false),
            user(),
            call("a", "Agent", r#"{"subagent_type":"haiku-worker"}"#),
            call("b", "Task", r#"{"subagent_type":"checker"}"#),
            call("c", "Agent", r#"{"description":"look around"}"#),
            call("bash", "Bash", r#"{"command":"ls"}"#),
            call("bg", "Agent", r#"{"subagent_type":"haiku-worker","run_in_background":true}"#),
            result("a", false),
            result("b", true),
            result("bash", false),
        ];
        let subs = of_turn(&items);
        let got: Vec<_> = subs.iter().map(|s| (s.id.as_str(), s.agent.as_str(), s.state)).collect();
        assert_eq!(
            got,
            [
                ("a", "haiku-worker", State::Done),
                ("b", "checker", State::Failed),
                ("c", "general-purpose", State::Working),
            ]
        );
        assert_eq!(subs[0].description, "task a");
        assert_eq!(count(&items), 5);
    }

    #[test]
    fn a_session_without_agents_has_none() {
        assert!(of_turn(&[]).is_empty());
        assert!(of_turn(&[user(), Item::Assistant("hi".into())]).is_empty());
    }
}
