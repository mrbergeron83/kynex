//! The subagents of a session's current turn, for the strip over the composer:
//! each `Agent` (or older `Task`) call since the last message you sent, which
//! agent it went to, and whether it's still working. Background agents aren't
//! here; they're running tasks ([`crate::claude::tasks`]) and outlive the turn.

use std::cell::RefCell;
use std::collections::HashMap;
use std::time::Duration;

use serde_json::Value;

use crate::claude::session::Item;

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
    of_turn_with(items, |_, input| parse(input))
}

/// Which agent a call went to; `None` for one run in the background.
fn parse(input: &str) -> Option<String> {
    let input: Value = serde_json::from_str(input).unwrap_or(Value::Null);
    if input["run_in_background"] == true {
        return None;
    }
    Some(input["subagent_type"].as_str().unwrap_or("general-purpose").to_string())
}

/// [`of_turn`] for a strip drawn every frame: each call's input is parsed once
/// (again only if it grows, as a streamed one does), not on every frame.
#[derive(Debug, Default)]
pub struct Agents(RefCell<HashMap<String, (usize, Option<String>)>>);

impl Agents {
    pub fn of_turn(&self, items: &[Item]) -> Vec<Subagent> {
        let mut seen = self.0.borrow_mut();
        let out = of_turn_with(items, |id, input| match seen.get(id) {
            Some((len, agent)) if *len == input.len() => agent.clone(),
            _ => {
                let agent = parse(input);
                seen.insert(id.to_string(), (input.len(), agent.clone()));
                agent
            }
        });
        // A new turn's calls replace the last one's.
        if seen.len() > out.len() + 64 {
            seen.retain(|id, _| out.iter().any(|s| s.id == *id));
        }
        out
    }
}

fn of_turn_with(items: &[Item], mut agent_of: impl FnMut(&str, &str) -> Option<String>) -> Vec<Subagent> {
    let start = items.iter().rposition(|i| matches!(i, Item::User { .. })).map_or(0, |i| i + 1);
    let turn = &items[start..];
    let mut out: Vec<Subagent> = Vec::new();
    for item in turn {
        match item {
            Item::ToolUse { id, name, summary, input, timing } if name == "Agent" || name == "Task" => {
                let Some(agent) = agent_of(id, input) else { continue };
                out.push(Subagent {
                    id: id.clone(),
                    agent,
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
    use crate::claude::session::Timing;

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
    fn the_cached_list_matches_and_follows_a_streamed_input() {
        let agents = Agents::default();
        let mut items = vec![user(), call("a", "Agent", "{}"), call("bg", "Agent", r#"{"run_in_background":true}"#)];
        assert_eq!(agents.of_turn(&items), of_turn(&items));
        assert_eq!(agents.of_turn(&items)[0].agent, "general-purpose");
        // The call's input came in full: parsed again.
        items[1] = call("a", "Agent", r#"{"subagent_type":"Explore"}"#);
        assert_eq!(agents.of_turn(&items)[0].agent, "Explore");
        assert_eq!(agents.of_turn(&items), of_turn(&items));
    }

    #[test]
    fn a_session_without_agents_has_none() {
        assert!(of_turn(&[]).is_empty());
        assert!(of_turn(&[user(), Item::Assistant("hi".into())]).is_empty());
    }
}
