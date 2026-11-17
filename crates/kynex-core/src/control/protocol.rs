//! The wire format: one JSON object per line each way.
//!
//! A request is `{"id": 1, "cmd": "session_prompt", "session": 3, "text": "hi", "wait": true}`
//! (`id` is echoed back, and optional; `caller`, also optional, is the session a `knx` run
//! inside one says it comes from). An answer is `{"id": 1, "ok": true, "result": …}` or
//! `{"id": 1, "ok": false, "error": "…"}`. After `{"cmd": "events"}` the connection streams
//! `{"event": "turn_end", …}` lines until it closes.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The longest line read, in bytes. A longer one is an error and ends the connection.
pub const MAX_LINE: usize = 1 << 20;

/// What a client can ask. A project is a tab of the window; its workspaces are its checkouts
/// (worktrees and folders added by hand).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    ProjectList,
    /// Makes the folder if it isn't there, then opens it.
    ProjectCreate {
        path: String,
    },
    ProjectOpen {
        path: String,
    },
    ProjectRemove {
        path: String,
    },
    /// The shown project's workspaces.
    WorkspaceList,
    /// Shows a workspace of the shown project.
    WorkspaceOpen {
        path: String,
    },
    /// Makes a workspace on `branch` (an existing branch, or a new one from the default branch);
    /// with `session`, a session in it too, sent `prompt` if given.
    WorkspaceCreate {
        branch: String,
        #[serde(default)]
        session: bool,
        #[serde(default)]
        prompt: Option<String>,
        /// Why the asking session wants it, for the card and the new session's first message.
        #[serde(default)]
        why: Option<String>,
    },
    /// Removes a workspace; git refuses while it has changes, unless `force`.
    WorkspaceRemove {
        path: String,
        #[serde(default)]
        force: bool,
    },
    /// The panes of the shown workspace's tab; with `all`, of every tab of every workspace
    /// shown since Kynex started.
    PaneList {
        #[serde(default)]
        all: bool,
    },
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
    /// What a terminal or agent pane shows: its screen, or the last `lines` of its scrollback
    /// and screen.
    PaneRead {
        pane: u64,
        #[serde(default)]
        lines: Option<usize>,
    },
    /// Types a command into a terminal pane's shell (a new terminal pane's, with no `pane`).
    /// With `wait`, the answer comes when it finished, with its exit code and what it printed.
    /// `timeout`, in seconds, for every wait: then the answer is an error.
    PaneRun {
        #[serde(default)]
        pane: Option<u64>,
        command: String,
        #[serde(default)]
        wait: bool,
        #[serde(default)]
        timeout: Option<u64>,
    },
    /// Sends a message to an agent pane (codex, opencode…). With `wait`, the answer comes when
    /// it's done or blocked.
    PanePrompt {
        pane: u64,
        text: String,
        #[serde(default)]
        wait: bool,
        #[serde(default)]
        timeout: Option<u64>,
    },
    /// Answers when an agent pane isn't working any more.
    PaneWait {
        pane: u64,
        #[serde(default)]
        timeout: Option<u64>,
    },
    SessionList,
    /// Sends a message to an open session. With `wait`, the answer comes when the turn ends
    /// (or the session needs you: permissions are still answered in the app).
    SessionPrompt {
        session: SessionRef,
        text: String,
        #[serde(default)]
        wait: bool,
        #[serde(default)]
        timeout: Option<u64>,
    },
    /// The last thing the session said.
    SessionRead {
        session: SessionRef,
    },
    /// Answers when the session isn't working any more.
    SessionWait {
        session: SessionRef,
        #[serde(default)]
        timeout: Option<u64>,
    },
    /// A new session in a workspace of the project (`workspace`: its folder; the asking
    /// session's, else the shown one, by default), sent `prompt` if given.
    SessionNew {
        #[serde(default)]
        workspace: Option<String>,
        #[serde(default)]
        prompt: Option<String>,
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        pane: Option<Place>,
        /// Why the asking session wants it, for the card and the new session's first message.
        #[serde(default)]
        why: Option<String>,
    },
    /// A copy of a session's conversation as a new session; with `at`, only what came before
    /// its `at`th message (from 1), as a rewind there would leave it.
    SessionFork {
        session: SessionRef,
        #[serde(default)]
        at: Option<usize>,
        #[serde(default)]
        pane: Option<Place>,
    },
    /// Opens a past session of the shown project by its CLI id (`id` is the envelope's).
    SessionOpen {
        session: String,
        #[serde(default)]
        pane: Option<Place>,
    },
    /// Closes an open session that isn't working (its transcript stays).
    SessionClose {
        session: SessionRef,
    },
    /// Names a session; an empty title goes back to the generated one.
    SessionRename {
        session: SessionRef,
        title: String,
    },
    SessionArchive {
        session: SessionRef,
    },
    /// Pins a session above the rest; with `off`, unpins it.
    SessionPin {
        session: SessionRef,
        #[serde(default)]
        off: bool,
    },
    /// Shows a toast, or an OS notification when Kynex is in the background; a click goes to
    /// `pane` or `session`.
    Notify {
        title: String,
        #[serde(default)]
        body: Option<String>,
        #[serde(default)]
        pane: Option<u64>,
        #[serde(default)]
        session: Option<SessionRef>,
    },
    /// Answers when a request that waited for your OK (see `approval`) has its outcome.
    RequestWait {
        request: u64,
        #[serde(default)]
        timeout: Option<u64>,
    },
    /// How a request that waited for your OK stands, at once.
    RequestStatus {
        request: u64,
    },
    Events,
}

/// A session, by its uid in this run (a number) or its CLI session id, which survives a restart.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SessionRef {
    Uid(u64),
    Id(String),
}

impl SessionRef {
    /// A word from the command line: a number is a uid, anything else a CLI id.
    pub fn parse(word: &str) -> Result<Self, String> {
        let word = word.trim();
        if word.is_empty() {
            return Err("a session is a number or a session id".into());
        }
        Ok(word.parse().map_or_else(|_| Self::Id(word.to_string()), Self::Uid))
    }

    /// Whether this names the session with uid `uid` and CLI id `id`.
    pub fn names(&self, uid: u64, id: Option<&str>) -> bool {
        match self {
            Self::Uid(u) => *u == uid,
            Self::Id(s) => id == Some(s.as_str()),
        }
    }
}

impl std::fmt::Display for SessionRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Uid(u) => write!(f, "{u}"),
            Self::Id(s) => f.write_str(s),
        }
    }
}

/// Where a session a request makes is shown: in a pane at the right of or below the asking
/// (else the focused) pane, in a new pane tab, or nowhere (`none`: only in the sessions list).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Place {
    #[default]
    Right,
    Down,
    Tab,
    #[serde(rename = "none")]
    Hidden,
}

impl Place {
    pub fn parse(word: &str) -> Result<Self, String> {
        match word {
            "right" => Ok(Self::Right),
            "down" => Ok(Self::Down),
            "tab" => Ok(Self::Tab),
            "none" => Ok(Self::Hidden),
            _ => Err(format!("`{word}` isn't right, down, tab or none")),
        }
    }
}

/// A line from a client.
#[derive(Debug, PartialEq, Deserialize)]
pub struct Incoming {
    /// Echoed back with the answer.
    #[serde(default)]
    pub id: Option<Value>,
    /// The session the request says it comes from (a `knx` run inside one), if any.
    #[serde(default)]
    pub caller: Option<u64>,
    #[serde(flatten)]
    pub request: Request,
}

/// A line from a client: its `id`, and what it asks.
pub fn parse_request(line: &str) -> Result<(Option<Value>, Request), String> {
    parse_incoming(line).map(|i| (i.id, i.request))
}

/// A line from a client, with the session it says it comes from.
pub fn parse_incoming(line: &str) -> Result<Incoming, String> {
    serde_json::from_str(line.trim()).map_err(|e| format!("not a request: {e}"))
}

/// A request as a line to send, with `id`.
pub fn request_line(id: u64, request: &Request) -> String {
    request_line_from(id, request, None)
}

/// [`request_line`] from session `caller`, when sent from inside one.
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
        assert_eq!(
            r,
            Request::SessionPrompt { session: SessionRef::Uid(3), text: "hi".into(), wait: true, timeout: None }
        );
        let (id, r) = parse_request(r#"{"cmd":"pane_list"}"#).unwrap();
        assert_eq!((id, r), (None, Request::PaneList { all: false }));
        let (_, r) = parse_request(r#"{"cmd":"session_prompt","session":1,"text":"x"}"#).unwrap();
        assert_eq!(
            r,
            Request::SessionPrompt { session: SessionRef::Uid(1), text: "x".into(), wait: false, timeout: None }
        );
    }

    #[test]
    fn a_session_is_a_uid_or_a_cli_id() {
        let (_, r) = parse_request(r#"{"cmd":"session_read","session":"5f3c-ab"}"#).unwrap();
        assert_eq!(r, Request::SessionRead { session: SessionRef::Id("5f3c-ab".into()) });
        assert_eq!(SessionRef::parse("12"), Ok(SessionRef::Uid(12)));
        assert_eq!(SessionRef::parse("5f3c"), Ok(SessionRef::Id("5f3c".into())));
        assert!(SessionRef::parse(" ").is_err());
        assert!(SessionRef::Uid(4).names(4, None));
        assert!(SessionRef::Id("a".into()).names(9, Some("a")));
        assert!(!SessionRef::Id("a".into()).names(9, None));
        assert_eq!(SessionRef::Id("a".into()).to_string(), "a");
    }

    #[test]
    fn new_session_requests_parse_with_their_defaults() {
        let (_, r) = parse_request(r#"{"cmd":"session_new"}"#).unwrap();
        assert_eq!(r, Request::SessionNew { workspace: None, prompt: None, title: None, pane: None, why: None });
        let (_, r) = parse_request(r#"{"cmd":"session_new","pane":"none","prompt":"go"}"#).unwrap();
        assert!(matches!(r, Request::SessionNew { pane: Some(Place::Hidden), prompt: Some(_), .. }));
        let (_, r) = parse_request(r#"{"cmd":"workspace_create","branch":"x"}"#).unwrap();
        assert_eq!(r, Request::WorkspaceCreate { branch: "x".into(), session: false, prompt: None, why: None });
        assert!(parse_request(r#"{"cmd":"session_new","pane":"left"}"#).is_err());
        assert_eq!(Place::parse("tab"), Ok(Place::Tab));
        assert!(Place::parse("left").is_err());
    }

    #[test]
    fn a_request_says_which_session_it_comes_from() {
        let line = request_line_from(3, &Request::SessionList, Some(12));
        let i = parse_incoming(&line).unwrap();
        assert_eq!((i.id, i.caller, i.request), (Some(json!(3)), Some(12), Request::SessionList));
        assert_eq!(parse_incoming(&request_line(3, &Request::SessionList)).unwrap().caller, None);
        assert!(parse_incoming(r#"{"cmd":"session_list","caller":"me"}"#).is_err(), "a caller is a uid");
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
            Request::ProjectList,
            Request::ProjectOpen { path: "/a".into() },
            Request::WorkspaceCreate {
                branch: "x".into(),
                session: true,
                prompt: Some("go".into()),
                why: Some("w".into()),
            },
            Request::SessionNew {
                workspace: Some("/w".into()),
                prompt: Some("fix it".into()),
                title: Some("Fix".into()),
                pane: Some(Place::Tab),
                why: None,
            },
            Request::SessionFork { session: SessionRef::Id("abc".into()), at: Some(2), pane: Some(Place::Hidden) },
            Request::SessionOpen { session: "abc".into(), pane: None },
            Request::SessionClose { session: SessionRef::Uid(3) },
            Request::SessionRename { session: SessionRef::Uid(3), title: "t".into() },
            Request::SessionArchive { session: SessionRef::Id("abc".into()) },
            Request::SessionPin { session: SessionRef::Uid(3), off: true },
            Request::Notify { title: "Done".into(), body: Some("all green".into()), pane: Some(2), session: None },
            Request::Notify { title: "Hi".into(), body: None, pane: None, session: Some(SessionRef::Uid(3)) },
            Request::RequestWait { request: 4, timeout: Some(10) },
            Request::RequestStatus { request: 4 },
            Request::WorkspaceRemove { path: "/w".into(), force: true },
            Request::PaneSplit { axis: Some("row".into()) },
            Request::PaneClose { pane: None },
            Request::PaneRead { pane: 4, lines: Some(50) },
            Request::PaneRun { pane: Some(4), command: "cargo test".into(), wait: true, timeout: Some(30) },
            Request::PaneRun { pane: None, command: "ls".into(), wait: false, timeout: None },
            Request::PaneList { all: true },
            Request::PanePrompt { pane: 5, text: "fix it".into(), wait: false, timeout: None },
            Request::PaneWait { pane: 5, timeout: Some(1) },
            Request::SessionWait { session: SessionRef::Uid(2), timeout: None },
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
