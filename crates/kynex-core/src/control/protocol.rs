//! The wire format: one JSON object per line each way.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::hooks::HookState;
use crate::layout::Direction;

/// The longest line read, in bytes. A longer one is an error and ends the connection.
pub const MAX_LINE: usize = 1 << 20;

/// What a client can ask over the control socket.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    /// Internal callback sent by `knx hook <provider> [event]` from inside an `agent_cli` pane.
    Hook {
        pane: u64,
        provider: String,
        #[serde(default)]
        state: Option<HookState>,
        #[serde(default)]
        session_id: Option<String>,
        #[serde(default)]
        detail: Option<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        subagent: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        background: bool,
    },
    ProjectList,
    ProjectCreate {
        path: String,
    },
    ProjectOpen {
        path: String,
    },
    ProjectRemove {
        path: String,
    },
    WorkspaceList,
    WorkspaceOpen {
        path: String,
    },
    WorkspaceCreate {
        branch: String,
        #[serde(default)]
        provider: Option<String>,
    },
    WorkspaceRemove {
        path: String,
        #[serde(default)]
        force: bool,
    },
    PaneList {
        #[serde(default)]
        all: bool,
    },
    PaneSplit {
        #[serde(default)]
        axis: Option<String>,
    },
    PaneFocus {
        pane: u64,
    },
    PaneMove {
        pane: u64,
        to: PaneTarget,
    },
    PaneSwap {
        pane: u64,
        other: u64,
    },
    PaneResize {
        pane: u64,
        dir: Direction,
        #[serde(default)]
        by: Option<u32>,
    },
    PaneZoom {
        pane: u64,
        #[serde(default)]
        off: bool,
    },
    PaneClose {
        #[serde(default)]
        pane: Option<u64>,
    },
    PaneRead {
        pane: u64,
        #[serde(default)]
        lines: Option<usize>,
    },
    PaneRun {
        #[serde(default)]
        pane: Option<u64>,
        command: String,
        #[serde(default)]
        wait: bool,
        #[serde(default)]
        timeout: Option<u64>,
    },
    PanePrompt {
        pane: u64,
        text: String,
        #[serde(default)]
        wait: bool,
        #[serde(default)]
        timeout: Option<u64>,
    },
    PaneWait {
        pane: u64,
        #[serde(default)]
        timeout: Option<u64>,
    },
    SessionList,
    SessionNew {
        #[serde(default)]
        workspace: Option<String>,
        #[serde(default)]
        provider: Option<String>,
        #[serde(default)]
        model: Option<String>,
        #[serde(default)]
        pane: Option<Place>,
    },
    SessionOpen {
        session: String,
        #[serde(default)]
        pane: Option<Place>,
    },
    SessionRename {
        session: String,
        title: String,
    },
    SessionArchive {
        session: String,
    },
    SessionPin {
        session: String,
        #[serde(default)]
        off: bool,
    },
    Events,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PaneTarget {
    Dir(Direction),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Place {
    #[default]
    Right,
    Down,
}

impl Place {
    pub fn parse(word: &str) -> Result<Self, String> {
        match word {
            "right" => Ok(Self::Right),
            "down" => Ok(Self::Down),
            _ => Err(format!("`{word}` isn't right or down")),
        }
    }
}

#[derive(Debug, PartialEq, Deserialize, JsonSchema)]
pub struct Incoming {
    #[serde(default)]
    pub id: Option<Value>,
    #[serde(default)]
    pub caller: Option<u64>,
    #[serde(flatten)]
    pub request: Request,
}

pub fn parse_request(line: &str) -> Result<(Option<Value>, Request), String> {
    parse_incoming(line).map(|i| (i.id, i.request))
}

pub fn parse_incoming(line: &str) -> Result<Incoming, String> {
    serde_json::from_str(line.trim()).map_err(|e| format!("not a request: {e}"))
}

pub fn request_line(id: u64, request: &Request) -> String {
    request_line_from(id, request, None)
}

pub fn request_line_from(id: u64, request: &Request, caller: Option<u64>) -> String {
    let mut v = serde_json::to_value(request).unwrap_or(Value::Null);
    if let Value::Object(m) = &mut v {
        m.insert("id".into(), json!(id));
        if let Some(c) = caller {
            m.insert("caller".into(), json!(c));
        }
    }
    v.to_string()
}

#[derive(Serialize, JsonSchema)]
pub struct Answer {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub fn answer(id: &Option<Value>, result: Result<Value, String>) -> String {
    let (ok, result, error) = match result {
        Ok(v) => (true, Some(v), None),
        Err(e) => (false, None, Some(e)),
    };
    serde_json::to_string(&Answer { id: id.clone(), ok, result, error }).unwrap_or_default()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    TurnEnd,
    NeedsYou,
    AgentState,
}

impl Event {
    pub const ALL: [Event; 3] = [Self::TurnEnd, Self::NeedsYou, Self::AgentState];

    pub const fn name(self) -> &'static str {
        match self {
            Self::TurnEnd => "turn_end",
            Self::NeedsYou => "needs_you",
            Self::AgentState => "agent_state",
        }
    }

    pub const fn description(self) -> &'static str {
        match self {
            Self::TurnEnd => "An agent_cli pane finished a turn. Fields: `pane`, `provider`, `state`.",
            Self::NeedsYou => "An agent_cli pane needs your input. Fields: `pane`, `provider`.",
            Self::AgentState => {
                "An agent_cli pane changed state. Fields: `pane`, `provider`, `session_id`, `state`, `detail`."
            }
        }
    }
}

pub fn event_line(event: Event, data: Value) -> String {
    let mut m = serde_json::Map::new();
    m.insert("event".into(), Value::String(event.name().to_string()));
    if let Value::Object(d) = data {
        m.extend(d);
    }
    Value::Object(m).to_string()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn every_request_survives_a_round_trip() {
        for r in sample_requests() {
            let (id, back) = parse_request(&request_line(9, &r)).unwrap();
            assert_eq!((id, back), (Some(json!(9)), r));
        }
    }

    pub(crate) fn sample_requests() -> Vec<Request> {
        vec![
            Request::Hook {
                pane: 2,
                provider: "claude".into(),
                state: Some(HookState::Working),
                session_id: Some("s1".into()),
                detail: Some("Using Bash".into()),
                subagent: false,
                background: false,
            },
            Request::ProjectList,
            Request::ProjectCreate { path: "/a".into() },
            Request::ProjectOpen { path: "/a".into() },
            Request::ProjectRemove { path: "/a".into() },
            Request::WorkspaceList,
            Request::WorkspaceOpen { path: "/w".into() },
            Request::WorkspaceCreate { branch: "x".into(), provider: Some("claude".into()) },
            Request::WorkspaceRemove { path: "/w".into(), force: true },
            Request::PaneList { all: true },
            Request::PaneSplit { axis: Some("row".into()) },
            Request::PaneFocus { pane: 1 },
            Request::PaneMove { pane: 2, to: PaneTarget::Dir(Direction::Left) },
            Request::PaneSwap { pane: 2, other: 3 },
            Request::PaneResize { pane: 2, dir: Direction::Down, by: Some(80) },
            Request::PaneZoom { pane: 2, off: true },
            Request::PaneClose { pane: None },
            Request::PaneRead { pane: 4, lines: Some(50) },
            Request::PaneRun { pane: Some(4), command: "cargo test".into(), wait: true, timeout: Some(30) },
            Request::PanePrompt { pane: 5, text: "fix it".into(), wait: false, timeout: None },
            Request::PaneWait { pane: 5, timeout: Some(1) },
            Request::SessionList,
            Request::SessionNew {
                workspace: Some("/w".into()),
                provider: Some("claude-openrouter".into()),
                model: Some("qwen/qwen3-coder".into()),
                pane: Some(Place::Down),
            },
            Request::SessionOpen { session: "abc".into(), pane: Some(Place::Right) },
            Request::SessionRename { session: "abc".into(), title: "t".into() },
            Request::SessionArchive { session: "abc".into() },
            Request::SessionPin { session: "abc".into(), off: true },
            Request::Events,
        ]
    }
}
