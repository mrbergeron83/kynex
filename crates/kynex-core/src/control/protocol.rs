//! The wire format: one JSON object per line each way.
//!
//! A request is `{"id": 1, "cmd": "session_prompt", "session": 3, "text": "hi", "wait": true}`
//! (`id` is echoed back, and optional). An answer is `{"id": 1, "ok": true, "result": …}` or
//! `{"id": 1, "ok": false, "error": "…"}`. After `{"cmd": "events"}` the connection streams
//! `{"event": "turn_end", …}` lines until it closes.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The longest line read, in bytes. A longer one is an error and ends the connection.
pub const MAX_LINE: usize = 1 << 20;

/// What a client can ask. "Context" is a project: a tab of the window.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    ContextList,
    /// Makes the folder if it isn't there, then opens it.
    ContextCreate {
        path: String,
    },
    ContextOpen {
        path: String,
    },
    ContextRemove {
        path: String,
    },
    /// The panes of the shown workspace.
    PaneList,
    /// Splits the focused pane (`row`: side by side, `column`: one above the other).
    PaneSplit {
        #[serde(default)]
        axis: Option<String>,
    },
    PaneFocus {
        pane: u64,
    },
    /// Closes a pane, the focused one if none is named.
    PaneClose {
        #[serde(default)]
        pane: Option<u64>,
    },
    SessionList,
    /// Sends a message to an open session. With `wait`, the answer comes when the turn ends
    /// (or the session needs you: permissions are still answered in the app).
    SessionPrompt {
        session: u64,
        text: String,
        #[serde(default)]
        wait: bool,
    },
    /// The last thing the session said.
    SessionRead {
        session: u64,
    },
    /// Answers when the session isn't working any more.
    SessionWait {
        session: u64,
    },
    Events,
}

#[derive(Deserialize)]
struct Envelope {
    #[serde(default)]
    id: Option<Value>,
    #[serde(flatten)]
    request: Request,
}

/// A line from a client: its `id`, and what it asks.
pub fn parse_request(line: &str) -> Result<(Option<Value>, Request), String> {
    let e: Envelope = serde_json::from_str(line.trim()).map_err(|e| format!("not a request: {e}"))?;
    Ok((e.id, e.request))
}

/// A request as a line to send, with `id`.
pub fn request_line(id: u64, request: &Request) -> String {
    let mut v = serde_json::to_value(request).unwrap_or(Value::Null);
    if let Value::Object(m) = &mut v {
        m.insert("id".into(), json!(id));
    }
    v.to_string()
}

/// The answer line for a request.
pub fn answer(id: &Option<Value>, result: Result<Value, String>) -> String {
    let mut m = serde_json::Map::new();
    if let Some(id) = id {
        m.insert("id".into(), id.clone());
    }
    match result {
        Ok(v) => {
            m.insert("ok".into(), Value::Bool(true));
            m.insert("result".into(), v);
        }
        Err(e) => {
            m.insert("ok".into(), Value::Bool(false));
            m.insert("error".into(), Value::String(e));
        }
    }
    Value::Object(m).to_string()
}

/// A line of the event stream.
pub fn event_line(name: &str, data: Value) -> String {
    let mut m = serde_json::Map::new();
    m.insert("event".into(), Value::String(name.to_string()));
    if let Value::Object(d) = data {
        m.extend(d);
    }
    Value::Object(m).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_parses_with_and_without_an_id() {
        let (id, r) =
            parse_request(r#"{"id": 7, "cmd": "session_prompt", "session": 3, "text": "hi", "wait": true}"#).unwrap();
        assert_eq!(id, Some(json!(7)));
        assert_eq!(r, Request::SessionPrompt { session: 3, text: "hi".into(), wait: true });
        let (id, r) = parse_request(r#"{"cmd":"pane_list"}"#).unwrap();
        assert_eq!((id, r), (None, Request::PaneList));
        let (_, r) = parse_request(r#"{"cmd":"session_prompt","session":1,"text":"x"}"#).unwrap();
        assert_eq!(r, Request::SessionPrompt { session: 1, text: "x".into(), wait: false });
    }

    #[test]
    fn nonsense_is_an_error_not_a_panic() {
        for bad in ["", "{", "[]", r#"{"cmd":"nope"}"#, r#"{"cmd":"pane_focus"}"#, r#"{"cmd":"pane_focus","pane":-1}"#]
        {
            assert!(parse_request(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn every_request_survives_a_round_trip() {
        let all = [
            Request::ContextList,
            Request::ContextOpen { path: "/a".into() },
            Request::PaneSplit { axis: Some("row".into()) },
            Request::PaneClose { pane: None },
            Request::SessionWait { session: 2 },
            Request::Events,
        ];
        for r in all {
            let (id, back) = parse_request(&request_line(9, &r)).unwrap();
            assert_eq!((id, back), (Some(json!(9)), r));
        }
    }

    #[test]
    fn answers_and_events_are_one_line_of_json() {
        let ok = answer(&Some(json!(1)), Ok(json!({"n": 1})));
        assert_eq!(serde_json::from_str::<Value>(&ok).unwrap(), json!({"id": 1, "ok": true, "result": {"n": 1}}));
        let bad = answer(&None, Err("no such session".into()));
        assert_eq!(serde_json::from_str::<Value>(&bad).unwrap(), json!({"ok": false, "error": "no such session"}));
        let ev = event_line("turn_end", json!({"session": 3}));
        assert_eq!(serde_json::from_str::<Value>(&ev).unwrap(), json!({"event": "turn_end", "session": 3}));
        assert!(!ok.contains('\n') && !ev.contains('\n'));
    }
}
