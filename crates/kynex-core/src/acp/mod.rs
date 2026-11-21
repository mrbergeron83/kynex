//! The Agent Client Protocol (ACP) as the agent CLIs Kynex drives speak it
//! (`grok agent stdio`, `agy acp`), turned into the stream-json a session
//! already handles, and back.
//!
//! The session talks to it as it talks to `claude`: [`Acp::outgoing`] takes the
//! messages it would write (a user message, a permission answer, an interrupt,
//! a mode switch) and gives the JSON-RPC lines to write instead;
//! [`Acp::incoming`] takes a line from the agent and gives stream-json events to
//! handle, errors to show, and lines to write back (answers Kynex gives itself).
//!
//! One process holds one conversation: `initialize`, then `session/new` (or
//! `session/load` to continue one) when the first message goes, then a
//! `session/prompt` per message. Each agent's extras differ ([`Agent`]):
//! Grok's `_x.ai/…` notifications, Antigravity's placeholder model ids.

pub(crate) mod usage;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Instant;

use serde_json::{Value, json};

use crate::project::Mode;

/// Which agent is on the other end: where their ACP dialects differ.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Agent {
    /// Grok Build (`grok agent stdio`): `_meta` modes, `_x.ai/…` usage and models.
    #[default]
    Grok,
    /// Antigravity (`agy acp`): plain ACP, `usage_update`, models named by id.
    Antigravity,
}

impl Agent {
    /// Its name in messages: "Grok couldn't start a session".
    pub fn label(self) -> &'static str {
        match self {
            Agent::Grok => "Grok",
            Agent::Antigravity => "Antigravity",
        }
    }
}

/// What [`Acp::incoming`] makes of a line.
#[derive(Debug, PartialEq)]
pub enum Out {
    /// A stream-json event for [`crate::session::Session`] to handle.
    Handle(Value),
    /// Something to show as an error in the transcript.
    Error(String),
    /// A line to write to `grok`.
    Write(Value),
}

/// The block a reply is streaming into.
#[derive(Debug, PartialEq)]
enum Block {
    None,
    Text(String),
    Thinking,
}

/// One agent conversation over ACP.
#[derive(Debug)]
pub struct Acp {
    agent: Agent,
    cwd: PathBuf,
    /// The model to switch to once the session is open; empty is the agent's default.
    model: String,
    mode: Mode,
    /// The conversation to continue, if any.
    resume: Option<String>,
    next_id: u64,
    init: Option<u64>,
    initialized: bool,
    /// The `session/new` or `session/load` in flight.
    opening: Option<u64>,
    /// While a `session/load` replays history the transcript already shows.
    loading: bool,
    session: Option<String>,
    /// Prompts written before the session was open.
    pending: Vec<Value>,
    /// The `session/prompt` in flight, and when it went.
    prompt: Option<(u64, Instant)>,
    block: Block,
    /// Permission requests waiting on the user: Kynex's request id → JSON-RPC id and options.
    asks: HashMap<String, (Value, Vec<Value>)>,
    /// Tool calls whose result has gone out.
    done: HashSet<String>,
    /// The models the agent offers, the one in use and their context windows.
    models: usage::Models,
    /// What the session's turns have cost so far, in USD.
    total_cost: f64,
    /// The reasoning effort to set once the session is open; empty is the model's own.
    effort: String,
    /// The project's allow and deny rules (Kynex's Permissions), and those allowed
    /// for this session: Antigravity's prompts offer no "always", so Kynex keeps
    /// them and answers for them ([`Self::ask`]).
    rules: Vec<crate::rules::Rule>,
    session_rules: Vec<String>,
}

/// A JSON-RPC request.
fn request(id: u64, method: &str, params: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
}

/// A JSON-RPC notification.
fn notification(method: &str, params: Value) -> Value {
    json!({ "jsonrpc": "2.0", "method": method, "params": params })
}

fn stream(event: Value) -> Out {
    Out::Handle(json!({ "type": "stream_event", "event": event }))
}

impl Acp {
    pub fn new(cwd: PathBuf, model: &str, mode: Mode, resume: Option<&str>) -> Self {
        Self {
            agent: Agent::Grok,
            cwd,
            model: model.trim().to_string(),
            mode,
            resume: resume.filter(|r| !r.is_empty()).map(str::to_string),
            next_id: 0,
            init: None,
            initialized: false,
            opening: None,
            loading: false,
            session: None,
            pending: Vec::new(),
            prompt: None,
            block: Block::None,
            asks: HashMap::new(),
            done: HashSet::new(),
            models: Default::default(),
            total_cost: 0.0,
            effort: String::new(),
            rules: Vec::new(),
            session_rules: Vec::new(),
        }
    }

    /// With the project's permission rules ([`Self::set_rules`]).
    pub fn with_rules(mut self, perms: &crate::project::Permissions) -> Self {
        self.set_rules(perms);
        self
    }

    /// Takes the project's permission rules, after a change to them.
    pub fn set_rules(&mut self, perms: &crate::project::Permissions) {
        self.rules = crate::rules::kynex_rules(perms);
    }

    /// For `agent` rather than Grok.
    pub fn for_agent(mut self, agent: Agent) -> Self {
        self.agent = agent;
        self
    }

    /// With a reasoning effort (`low`, `high`…) to set on the session; empty is the model's own.
    pub fn with_effort(mut self, effort: &str) -> Self {
        self.effort = effort.trim().to_string();
        self
    }

    /// The agent's models, when the list came or changed since last taken, for the model picker.
    pub fn take_models(&mut self) -> Option<Vec<crate::session::ModelInfo>> {
        std::mem::take(&mut self.models.fresh).then(|| self.models.list.clone())
    }

    /// Switches the open session to `model` (empty: leaves it as it is); before it opens, it's used then.
    pub fn set_model(&mut self, model: &str) -> Vec<Value> {
        self.model = model.trim().to_string();
        let Some(sid) = self.session.clone().filter(|_| !self.model.is_empty()) else { return Vec::new() };
        self.models.current = self.model.clone();
        let id = self.id();
        let params = json!({ "sessionId": sid, "modelId": self.models.wire_id(&self.model) });
        let mut lines = vec![request(id, "session/set_model", params)];
        // A model starts on its own effort: the chosen one goes again.
        lines.extend(self.effort_lines());
        lines
    }

    /// Sets the reasoning effort; empty goes back to the model's own.
    pub fn set_effort(&mut self, effort: &str) -> Vec<Value> {
        let effort = effort.trim().to_string();
        if effort == self.effort {
            return Vec::new();
        }
        self.effort = effort;
        self.effort_lines()
    }

    /// The line that puts the open session on `self.effort`, or the model's default.
    fn effort_lines(&mut self) -> Vec<Value> {
        let Some(sid) = self.session.clone() else { return Vec::new() };
        let value = match self.effort.as_str() {
            "" => match self.models.default_effort() {
                Some(d) => d.to_string(),
                None => return Vec::new(),
            },
            e => e.to_string(),
        };
        // A level the model doesn't have would be refused: leave it on its own.
        let current = self.models.list.iter().find(|m| m.value == self.models.current);
        if current.is_some_and(|m| !m.efforts.is_empty() && !m.efforts.contains(&value)) {
            return Vec::new();
        }
        let id = self.id();
        let params = json!({ "sessionId": sid, "configId": "reasoning_effort", "value": value });
        vec![request(id, "session/set_config_option", params)]
    }

    fn id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    /// The first line to write: `initialize`. Kynex reads and writes no files
    /// for the agent and runs no terminals for it; the agent does both itself.
    pub fn start(&mut self) -> Vec<Value> {
        let id = self.id();
        self.init = Some(id);
        vec![request(id, "initialize", initialize_params())]
    }

    /// Whether a turn is running.
    pub fn in_turn(&self) -> bool {
        self.prompt.is_some()
    }

    /// Opens the conversation: continues `resume`'s, else starts one.
    fn open(&mut self) -> Vec<Value> {
        if self.opening.is_some() || self.session.is_some() || !self.initialized {
            return Vec::new();
        }
        let id = self.id();
        self.opening = Some(id);
        let mut params = json!({ "cwd": self.cwd.to_string_lossy(), "mcpServers": [] });
        if self.agent == Agent::Grok {
            // Asking is the default; `yoloMode` (approve everything) stays off, and `autoMode` is Grok's auto.
            params["_meta"] = json!({ "yoloMode": false, "autoMode": self.mode == Mode::Auto });
        }
        match &self.resume {
            Some(sid) => {
                self.loading = true;
                params["sessionId"] = json!(sid);
                vec![request(id, "session/load", params)]
            }
            None => vec![request(id, "session/new", params)],
        }
    }

    /// The lines that put an open session in `self.mode`.
    fn mode_lines(&mut self) -> Vec<Value> {
        let Some(sid) = self.session.clone() else { return Vec::new() };
        let id = self.id();
        if self.agent == Agent::Antigravity {
            // Its modes: `default` asks, `accept-edits` doesn't for edits, `plan` plans. Bypass
            // and Auto answer the rest of the asks here ([`Self::ask`]).
            let mode = match self.mode {
                Mode::Plan => "plan",
                Mode::AcceptEdits | Mode::Auto | Mode::Bypass => "accept-edits",
                Mode::Manual => "default",
            };
            return vec![request(id, "session/set_mode", json!({ "sessionId": sid, "modeId": mode }))];
        }
        let word = if self.mode == Mode::Auto { "auto" } else { "ask" };
        vec![
            notification(
                "_x.ai/yolo_mode_changed",
                json!({ "yolo_mode": false, "auto_mode": word == "auto", "permission_mode": word }),
            ),
            request(
                id,
                "session/set_mode",
                json!({ "sessionId": sid, "modeId": if self.mode == Mode::Plan { "plan" } else { "default" } }),
            ),
        ]
    }

    /// `prompt` as a `session/prompt`, now.
    fn prompt_line(&mut self, prompt: Value) -> Value {
        let id = self.id();
        self.prompt = Some((id, Instant::now()));
        let sid = self.session.clone().unwrap_or_default();
        request(id, "session/prompt", json!({ "sessionId": sid, "prompt": prompt }))
    }

    /// What to write for `msg`, a message the session would send `claude`.
    pub fn outgoing(&mut self, msg: Value) -> Vec<Value> {
        match msg["type"].as_str().unwrap_or("") {
            "user" => {
                // A comment on an answer while the turn runs: Grok has no place for it mid-turn.
                if self.prompt.is_some() {
                    return Vec::new();
                }
                let prompt = prompt_blocks(&msg["message"]["content"]);
                if self.session.is_some() && self.opening.is_none() {
                    return vec![self.prompt_line(prompt)];
                }
                self.pending.push(prompt);
                self.open()
            }
            "control_request" => match msg["request"]["subtype"].as_str().unwrap_or("") {
                "interrupt" => self.cancel(),
                "set_permission_mode" => {
                    if let Some(mode) = msg["request"]["mode"].as_str().and_then(Mode::from_cli) {
                        self.mode = mode;
                    }
                    self.mode_lines()
                }
                // `initialize`, MCP and model requests are `claude`'s; Grok has its own.
                _ => Vec::new(),
            },
            "control_response" => {
                let response = &msg["response"];
                let Some((rpc, options)) = response["request_id"].as_str().and_then(|r| self.asks.remove(r)) else {
                    return Vec::new();
                };
                let answer = &response["response"];
                // "This session" or "always in this project": kept here, for the next asks.
                for rule in crate::project::suggested_rules(&answer["updatedPermissions"]) {
                    if !self.session_rules.contains(&rule) {
                        self.session_rules.push(rule);
                    }
                }
                let (kinds, stop): (&[&str], bool) = match answer["behavior"].as_str() {
                    Some("allow") if answer["updatedPermissions"].as_array().is_some_and(|u| !u.is_empty()) => {
                        (&["allow_always", "allow_once"], false)
                    }
                    Some("allow") => (&["allow_once", "allow_always"], false),
                    _ => (&["reject_once", "reject_always"], answer["interrupt"] == true),
                };
                let mut lines = vec![choose(rpc, &options, kinds)];
                if stop {
                    lines.extend(self.cancel());
                }
                lines
            }
            _ => Vec::new(),
        }
    }

    /// Stops the turn: `session/cancel`, and every question still open is withdrawn.
    fn cancel(&mut self) -> Vec<Value> {
        let mut lines: Vec<Value> = self
            .asks
            .drain()
            .map(|(_, (rpc, _))| {
                json!({ "jsonrpc": "2.0", "id": rpc, "result": { "outcome": { "outcome": "cancelled" } } })
            })
            .collect();
        if let (Some(sid), Some(_)) = (&self.session, self.prompt) {
            lines.push(notification("session/cancel", json!({ "sessionId": sid })));
        }
        lines
    }

    /// Closes the block a reply was streaming into.
    fn close(&mut self) -> Vec<Out> {
        match std::mem::replace(&mut self.block, Block::None) {
            Block::None => Vec::new(),
            Block::Thinking => vec![stream(json!({ "type": "content_block_stop" }))],
            // The whole message after its deltas, as `claude` sends it, so the session settles the text.
            Block::Text(text) => vec![
                stream(json!({ "type": "content_block_stop" })),
                Out::Handle(
                    json!({ "type": "assistant", "message": { "content": [{ "type": "text", "text": text }] } }),
                ),
            ],
        }
    }

    /// What a line from `grok` means for the session.
    pub fn incoming(&mut self, v: Value) -> Vec<Out> {
        let method = v["method"].as_str();
        let has_id = v.get("id").is_some_and(|id| !id.is_null());
        match method {
            None if has_id => self.response(&v),
            Some("session/update") if !self.loading => self.update(&v["params"]["update"]),
            Some("session/request_permission") if has_id => self.ask(&v),
            // The model list, again when it changes (a switch, a refresh).
            Some("_x.ai/models/update") => {
                self.models.read(&v["params"]);
                Vec::new()
            }
            // One model call done: how much context it used.
            Some("_x.ai/session_notification")
                if !self.loading && v["params"]["update"]["sessionUpdate"] == "response_completed" =>
            {
                let Some(usage) = usage::context(&self.models.current, &v["params"]["update"]["usage"]) else {
                    return Vec::new();
                };
                // A text block streaming ends with its call; close it first so the text settles once.
                let mut out = self.close();
                out.push(Out::Handle(usage));
                out
            }
            // A request Kynex doesn't serve (it offered no files or terminals).
            Some(_) if has_id => vec![Out::Write(json!({
                "jsonrpc": "2.0", "id": v["id"], "error": { "code": -32601, "message": "method not found" }
            }))],
            _ => Vec::new(),
        }
    }

    fn response(&mut self, v: &Value) -> Vec<Out> {
        let id = v["id"].as_u64();
        let error = v["error"]["message"].as_str().or(v["error"]["data"].as_str()).map(str::to_string);
        if id.is_some() && id == self.init {
            self.init = None;
            if let Some(e) = error {
                return vec![Out::Error(format!("{} didn't start: {e}", self.agent.label()))];
            }
            self.initialized = true;
            // Continuing a conversation: open it now, it takes a moment to load.
            if self.resume.is_some() || !self.pending.is_empty() {
                return self.open().into_iter().map(Out::Write).collect();
            }
            return Vec::new();
        }
        if id.is_some() && id == self.opening {
            self.opening = None;
            self.loading = false;
            let mut out = Vec::new();
            if let Some(e) = error {
                if self.resume.take().is_some() {
                    // The conversation is gone on Grok's side: start a new one rather than fail.
                    out.push(Out::Error(format!(
                        "Couldn't continue the {} session ({e}); starting a new one.",
                        self.agent.label()
                    )));
                    out.extend(self.open().into_iter().map(Out::Write));
                } else {
                    out.push(Out::Error(format!("{} couldn't start a session: {e}", self.agent.label())));
                    self.pending.clear();
                    out.push(Out::Handle(json!({ "type": "result", "is_error": true, "duration_ms": 0 })));
                }
                return out;
            }
            let sid = v["result"]["sessionId"].as_str().map(str::to_string).or_else(|| self.resume.clone());
            self.session = sid.clone();
            match self.agent {
                Agent::Grok => self.models.read(&v["result"]["models"]),
                Agent::Antigravity => self.models.read_named(&v["result"]["models"]),
            }
            if !self.model.is_empty() {
                self.models.current = self.model.clone();
            }
            if let Some(sid) = &sid {
                out.push(Out::Handle(json!({ "type": "grok_session", "session_id": sid })));
            }
            out.extend(self.mode_lines().into_iter().map(Out::Write));
            if !self.model.is_empty() {
                let id = self.id();
                let params = json!({ "sessionId": sid, "modelId": self.models.wire_id(&self.model) });
                out.push(Out::Write(request(id, "session/set_model", params)));
            }
            if !self.effort.is_empty() {
                out.extend(self.effort_lines().into_iter().map(Out::Write));
            }
            if !self.pending.is_empty() {
                let prompt = self.pending.remove(0);
                // Anything more written meanwhile goes with it.
                let mut prompt = prompt.as_array().cloned().unwrap_or_default();
                for more in self.pending.drain(..) {
                    prompt.extend(more.as_array().cloned().unwrap_or_default());
                }
                let line = self.prompt_line(Value::Array(prompt));
                out.push(Out::Write(line));
            }
            return out;
        }
        if let Some((prompt, at)) = self.prompt
            && id == Some(prompt)
        {
            self.prompt = None;
            let mut out = self.close();
            let stop = v["result"]["stopReason"].as_str().unwrap_or("");
            if let Some(e) = &error {
                out.push(Out::Error(format!("{}: {e}", self.agent.label())));
            } else if stop == "refusal" {
                out.push(Out::Error(format!("{} refused this request.", self.agent.label())));
            } else if matches!(stop, "max_tokens" | "max_turn_requests") {
                out.push(Out::Error(format!("{} stopped early ({}).", self.agent.label(), stop.replace('_', " "))));
            }
            let mut result = match self.agent {
                Agent::Grok => usage::turn(&v["result"]["_meta"]["usage"], &self.models, &mut self.total_cost, true),
                // Antigravity counts cached tokens apart from its input, and sends no cost.
                Agent::Antigravity => usage::turn(&v["result"]["usage"], &self.models, &mut self.total_cost, false),
            };
            result.insert("type".into(), json!("result"));
            result.insert("duration_ms".into(), json!(at.elapsed().as_millis() as u64));
            result.insert("is_error".into(), json!(error.is_some() || stop == "refusal"));
            out.push(Out::Handle(Value::Object(result)));
            return out;
        }
        Vec::new()
    }

    fn update(&mut self, u: &Value) -> Vec<Out> {
        match u["sessionUpdate"].as_str().unwrap_or("") {
            "agent_message_chunk" => {
                let text = u["content"]["text"].as_str().unwrap_or("");
                let mut out = Vec::new();
                if !matches!(self.block, Block::Text(_)) {
                    out = self.close();
                    out.push(stream(json!({ "type": "content_block_start", "content_block": { "type": "text" } })));
                    self.block = Block::Text(String::new());
                }
                if let Block::Text(all) = &mut self.block {
                    all.push_str(text);
                }
                out.push(stream(
                    json!({ "type": "content_block_delta", "delta": { "type": "text_delta", "text": text } }),
                ));
                out
            }
            "agent_thought_chunk" => {
                let text = u["content"]["text"].as_str().unwrap_or("");
                let mut out = Vec::new();
                if self.block != Block::Thinking {
                    out = self.close();
                    out.push(stream(json!({ "type": "content_block_start", "content_block": { "type": "thinking" } })));
                    self.block = Block::Thinking;
                }
                let delta = json!({ "type": "thinking_delta", "thinking": text });
                out.push(stream(json!({ "type": "content_block_delta", "delta": delta })));
                out
            }
            "tool_call" => {
                let mut out = self.close();
                let id = u["toolCallId"].as_str().unwrap_or("");
                let (name, input) = tool(u);
                out.push(Out::Handle(json!({ "type": "assistant", "message": { "content": [{
                    "type": "tool_use", "id": id, "name": name, "input": input,
                }] } })));
                out.extend(self.result(u));
                out
            }
            "tool_call_update" => self.result(u),
            // Antigravity: the context in use and the window, after each model call.
            "usage_update" if self.agent == Agent::Antigravity => {
                let (Some(used), Some(size)) = (u["used"].as_u64(), u["size"].as_u64()) else { return Vec::new() };
                self.models.set_window(size);
                if used == 0 {
                    return Vec::new();
                }
                let mut out = self.close();
                out.push(Out::Handle(usage::context_used(&self.models.current, used)));
                out
            }
            // Antigravity: the model in use, after a switch.
            "config_option_update" if self.agent == Agent::Antigravity => {
                self.models.read_config(&u["configOptions"]);
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    /// A finished call's result, once.
    fn result(&mut self, u: &Value) -> Vec<Out> {
        let status = u["status"].as_str().unwrap_or("");
        let id = u["toolCallId"].as_str().unwrap_or("").to_string();
        if !matches!(status, "completed" | "failed") || !self.done.insert(id.clone()) {
            return Vec::new();
        }
        vec![Out::Handle(json!({ "type": "user", "message": { "content": [{
            "type": "tool_result",
            "tool_use_id": id,
            "content": result_text(&u["content"]),
            "is_error": status == "failed",
        }] } }))]
    }

    /// A `session/request_permission`: answered here when the mode already
    /// allows it, otherwise shown as a permission prompt.
    fn ask(&mut self, v: &Value) -> Vec<Out> {
        let p = &v["params"];
        let options = p["options"].as_array().cloned().unwrap_or_default();
        let call = &p["toolCall"];
        let kind = call["kind"].as_str().or(call["_meta"]["x.ai/tool"]["kind"].as_str()).unwrap_or("");
        if self.mode == Mode::Bypass || (kind == "edit" && matches!(self.mode, Mode::AcceptEdits | Mode::Auto)) {
            return vec![Out::Write(choose(v["id"].clone(), &options, &["allow_once", "allow_always"]))];
        }
        let (name, input) = tool(call);
        // Antigravity: the project's rules and this session's decide first, then a
        // command that only reads goes, as Claude Code's would. Grok keeps its own.
        let mut suggestions = Value::Null;
        if self.agent == Agent::Antigravity {
            match self.decide(&name, &input) {
                Some(true) => return vec![Out::Write(choose(v["id"].clone(), &options, &["allow_once"]))],
                Some(false) => return vec![Out::Write(choose(v["id"].clone(), &options, &["reject_once"]))],
                None if matches!(kind, "read" | "search") => {
                    return vec![Out::Write(choose(v["id"].clone(), &options, &["allow_once"]))];
                }
                None => suggestions = suggestion(&name, &input),
            }
        }
        let request_id = format!("acp-{}", v["id"]);
        self.asks.insert(request_id.clone(), (v["id"].clone(), options));
        let mut out = self.close();
        out.push(Out::Handle(json!({
            "type": "control_request",
            "request_id": request_id,
            "request": {
                "subtype": "can_use_tool",
                "tool_name": name,
                "input": input,
                "description": call["title"].as_str().unwrap_or(""),
                "permission_suggestions": suggestions,
            },
        })));
        out
    }

    /// What the rules say about a call: allow, deny, or `None` to ask.
    fn decide(&self, name: &str, input: &Value) -> Option<bool> {
        use crate::rules::{Kind, Rule, Source};
        let session =
            self.session_rules.iter().map(|t| Rule { kind: Kind::Allow, text: t.clone(), source: Source::Kynex });
        let rules: Vec<Rule> = self.rules.iter().cloned().chain(session).collect();
        if name == "Bash" {
            let command = input["command"].as_str().unwrap_or("").trim();
            if command.is_empty() {
                return None;
            }
            return match crate::rules::test(&rules, command).decision {
                Some(Kind::Deny) => Some(false),
                Some(Kind::Allow) => Some(true),
                Some(Kind::Ask) => None,
                None => crate::read_only::is_read_only(command).then_some(true),
            };
        }
        // Other tools: a rule naming the tool alone, like `Write`.
        let named = |kind: Kind| rules.iter().any(|r| r.kind == kind && r.text.trim() == name);
        if named(Kind::Deny) {
            Some(false)
        } else if named(Kind::Allow) {
            Some(true)
        } else {
            None
        }
    }
}

/// The rule the prompt offers to keep for a call, as Claude's `permission_suggestions`:
/// a command's program (with its subcommand for tools that have them, like
/// `git log`) as `Bash(git log:*)`, any other tool by its name.
fn suggestion(name: &str, input: &Value) -> Value {
    let (tool, content) = if name == "Bash" {
        let command = input["command"].as_str().unwrap_or("");
        let words: Vec<&str> = command.split_whitespace().collect();
        let Some(first) = words.first() else { return Value::Null };
        // A chained or redirected command gets no rule: one for its first program would say too much.
        if command.contains(['|', ';', '&', '>', '`', '\n']) || command.contains("$(") {
            return Value::Null;
        }
        let with_sub = ["git", "npm", "pnpm", "yarn", "cargo", "dotnet", "gh", "go", "docker", "kubectl"];
        let prefix = match words.get(1) {
            Some(sub) if with_sub.contains(&first.to_ascii_lowercase().as_str()) && !sub.starts_with('-') => {
                format!("{first} {sub}")
            }
            _ => first.to_string(),
        };
        ("Bash", Some(format!("{prefix}:*")))
    } else {
        (name, None)
    };
    let mut rule = json!({ "toolName": tool });
    if let Some(c) = content {
        rule["ruleContent"] = json!(c);
    }
    json!([{ "type": "addRules", "rules": [rule], "behavior": "allow", "destination": "session" }])
}

/// The answer to permission request `rpc`: the first of `options` whose kind is
/// in `kinds`, in that order, else the first option.
fn choose(rpc: Value, options: &[Value], kinds: &[&str]) -> Value {
    let pick = kinds
        .iter()
        .find_map(|k| options.iter().find(|o| o["kind"] == *k))
        .or(options.first())
        .and_then(|o| o["optionId"].as_str());
    let outcome = match pick {
        Some(id) => json!({ "outcome": "selected", "optionId": id }),
        None => json!({ "outcome": "cancelled" }),
    };
    json!({ "jsonrpc": "2.0", "id": rpc, "result": { "outcome": outcome } })
}

/// A tool call as the transcript names it: the agents' shell, edit, write and
/// read tools under Claude's names and argument names, so they draw the same
/// (a command, a diff, a path). Grok names its tool in `_meta`, Antigravity in `name`.
fn tool(call: &Value) -> (String, Value) {
    let agent_name = call["_meta"]["x.ai/tool"]["name"]
        .as_str()
        .or(call["name"].as_str())
        .or(call["title"].as_str())
        .unwrap_or("tool");
    let mut input = match &call["rawInput"] {
        Value::Object(m) => Value::Object(m.clone()),
        Value::Null => json!({}),
        other => other.clone(),
    };
    if let Some(m) = input.as_object_mut() {
        m.remove("variant");
    }
    // Renames the first of `from` found to `to`.
    let rename = |input: &mut Value, from: &[&str], to: &str| {
        if let Some(m) = input.as_object_mut()
            && let Some(v) = from.iter().find_map(|k| m.remove(*k))
        {
            m.insert(to.into(), v);
        }
    };
    let name = match agent_name {
        "run_terminal_command" | "run_terminal_cmd" => "Bash",
        "run_command" => {
            rename(&mut input, &["CommandLine"], "command");
            "Bash"
        }
        "search_replace" => "Edit",
        "read_file" | "view_file" => {
            rename(&mut input, &["target_file", "Path", "AbsolutePath"], "file_path");
            "Read"
        }
        "write_file" | "write_to_file" => {
            rename(&mut input, &["Path", "TargetFile"], "file_path");
            rename(&mut input, &["Content", "CodeContent"], "content");
            "Write"
        }
        other => {
            if call["kind"].as_str() == Some("execute")
                && input.as_object().is_some_and(|m| m.contains_key("CommandLine") || m.contains_key("command"))
            {
                rename(&mut input, &["CommandLine"], "command");
                "Bash"
            } else {
                other
            }
        }
    };
    (name.to_string(), input)
}

/// A tool call's content, as text: what it printed, or the files it changed.
fn result_text(content: &Value) -> String {
    content
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| match c["type"].as_str() {
            Some("content") => c["content"]["text"].as_str().map(str::to_string),
            Some("diff") => c["path"].as_str().map(|p| format!("Edited {p}")),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A user message's content blocks as ACP prompt blocks: text, and images inline.
fn prompt_blocks(content: &Value) -> Value {
    let blocks = match content {
        Value::String(s) => vec![json!({ "type": "text", "text": s })],
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|b| match b["type"].as_str() {
                Some("text") => Some(json!({ "type": "text", "text": b["text"] })),
                Some("image") if b["source"]["type"] == "base64" => Some(json!({
                    "type": "image",
                    "mimeType": b["source"]["media_type"],
                    "data": b["source"]["data"],
                })),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    };
    Value::Array(blocks)
}

/// What Kynex tells Grok about itself in `initialize`.
pub(crate) fn initialize_params() -> Value {
    json!({
        "protocolVersion": 1,
        "clientCapabilities": { "fs": { "readTextFile": false, "writeTextFile": false }, "terminal": false },
        "_meta": { "clientIdentifier": "kynex" },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn writes(out: &[Out]) -> Vec<&Value> {
        out.iter().filter_map(|o| if let Out::Write(v) = o { Some(v) } else { None }).collect()
    }

    fn handles(out: &[Out]) -> Vec<&Value> {
        out.iter().filter_map(|o| if let Out::Handle(v) = o { Some(v) } else { None }).collect()
    }

    fn user(text: &str) -> Value {
        json!({ "type": "user", "message": { "role": "user", "content": [{ "type": "text", "text": text }] } })
    }

    fn update(u: Value) -> Value {
        json!({ "jsonrpc": "2.0", "method": "session/update", "params": { "sessionId": "s1", "update": u } })
    }

    /// An `Acp` with its session open, as after `initialize` and `session/new`.
    fn open(mode: Mode) -> Acp {
        let mut a = Acp::new(PathBuf::from("C:/work"), "", mode, None);
        let init = a.start();
        a.incoming(json!({ "jsonrpc": "2.0", "id": init[0]["id"], "result": {} }));
        let new = a.outgoing(user("hi"));
        assert_eq!(new[0]["method"], "session/new");
        let out = a.incoming(json!({ "jsonrpc": "2.0", "id": new[0]["id"], "result": { "sessionId": "s1" } }));
        assert!(writes(&out).iter().any(|w| w["method"] == "session/prompt"));
        a
    }

    #[test]
    fn the_first_message_opens_the_session_then_goes() {
        let mut a = Acp::new(PathBuf::from("C:/work"), "grok-4.7", Mode::Manual, None);
        let init = a.start();
        assert_eq!(init[0]["method"], "initialize");
        // Nothing to open until there's a message.
        assert!(a.incoming(json!({ "jsonrpc": "2.0", "id": init[0]["id"], "result": {} })).is_empty());
        let new = a.outgoing(user("hello"));
        assert_eq!(new[0]["method"], "session/new");
        assert_eq!(new[0]["params"]["_meta"]["yoloMode"], false);
        let out = a.incoming(json!({ "jsonrpc": "2.0", "id": new[0]["id"], "result": { "sessionId": "s1" } }));
        assert_eq!(handles(&out)[0]["session_id"], "s1");
        let w = writes(&out);
        assert!(w.iter().any(|w| w["method"] == "_x.ai/yolo_mode_changed" && w["params"]["permission_mode"] == "ask"));
        assert!(w.iter().any(|w| w["method"] == "session/set_model" && w["params"]["modelId"] == "grok-4.7"));
        let prompt = w.iter().find(|w| w["method"] == "session/prompt").unwrap();
        assert_eq!(prompt["params"]["prompt"][0]["text"], "hello");
        assert!(a.in_turn());
    }

    #[test]
    fn resuming_loads_at_once_and_skips_the_replay() {
        let mut a = Acp::new(PathBuf::from("C:/work"), "", Mode::Manual, Some("old"));
        let init = a.start();
        let out = a.incoming(json!({ "jsonrpc": "2.0", "id": init[0]["id"], "result": {} }));
        let load = writes(&out)[0].clone();
        assert_eq!(load["method"], "session/load");
        assert_eq!(load["params"]["sessionId"], "old");
        // History the transcript already has.
        let replay =
            update(json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": "old" } }));
        assert!(a.incoming(replay).is_empty());
        // A message written meanwhile waits for the load.
        assert!(a.outgoing(user("next")).is_empty());
        let out = a.incoming(json!({ "jsonrpc": "2.0", "id": load["id"], "result": {} }));
        assert_eq!(handles(&out)[0]["session_id"], "old");
        assert!(writes(&out).iter().any(|w| w["method"] == "session/prompt" && w["params"]["sessionId"] == "old"));
    }

    #[test]
    fn a_lost_session_starts_a_new_one() {
        let mut a = Acp::new(PathBuf::from("C:/work"), "", Mode::Manual, Some("gone"));
        let init = a.start();
        let out = a.incoming(json!({ "jsonrpc": "2.0", "id": init[0]["id"], "result": {} }));
        let load = writes(&out)[0].clone();
        let out = a
            .incoming(json!({ "jsonrpc": "2.0", "id": load["id"], "error": { "code": -32602, "message": "unknown" } }));
        assert!(matches!(&out[0], Out::Error(e) if e.contains("starting a new one")));
        assert_eq!(writes(&out)[0]["method"], "session/new");
    }

    #[test]
    fn a_reply_streams_and_the_turn_ends_with_a_result() {
        let mut a = open(Mode::Manual);
        let chunk = |t: &str| {
            update(json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": t } }))
        };
        let thought =
            update(json!({ "sessionUpdate": "agent_thought_chunk", "content": { "type": "text", "text": "hmm" } }));
        let out = a.incoming(thought);
        assert_eq!(handles(&out)[0]["event"]["content_block"]["type"], "thinking");
        let out = a.incoming(chunk("Hel"));
        // The thought closes, the text opens.
        assert_eq!(handles(&out)[0]["event"]["type"], "content_block_stop");
        assert_eq!(handles(&out)[1]["event"]["content_block"]["type"], "text");
        a.incoming(chunk("lo"));
        let prompt = a.prompt.unwrap().0;
        let out = a.incoming(json!({ "jsonrpc": "2.0", "id": prompt, "result": { "stopReason": "end_turn" } }));
        let h = handles(&out);
        assert_eq!(h[1]["message"]["content"][0]["text"], "Hello");
        assert_eq!(h[2]["type"], "result");
        assert!(!a.in_turn());
    }

    #[test]
    fn tool_calls_read_as_claudes_and_end_once() {
        let mut a = open(Mode::Manual);
        let call = update(json!({
            "sessionUpdate": "tool_call", "toolCallId": "c1", "title": "run_terminal_command",
            "rawInput": { "command": "echo hi", "description": "Say hi" },
            "_meta": { "x.ai/tool": { "name": "run_terminal_command", "kind": "execute" } }
        }));
        let out = a.incoming(call);
        let block = &handles(&out)[0]["message"]["content"][0];
        assert_eq!(block["name"], "Bash");
        assert_eq!(block["input"]["command"], "echo hi");
        let done = |status: &str| {
            update(json!({ "sessionUpdate": "tool_call_update", "toolCallId": "c1", "status": status,
                "content": [{ "type": "content", "content": { "type": "text", "text": "hi" } }] }))
        };
        assert!(a.incoming(done("in_progress")).is_empty());
        let out = a.incoming(done("completed"));
        let result = &handles(&out)[0]["message"]["content"][0];
        assert_eq!((result["tool_use_id"].as_str(), result["content"].as_str()), (Some("c1"), Some("hi")));
        assert!(a.incoming(done("completed")).is_empty(), "once");
        // Grok's read names its path `target_file`.
        let (name, input) =
            tool(&json!({ "title": "read_file", "rawInput": { "variant": "ReadFile", "target_file": "a.rs" } }));
        assert_eq!((name.as_str(), input), ("Read", json!({ "file_path": "a.rs" })));
    }

    fn permission(kind: &str) -> Value {
        json!({ "jsonrpc": "2.0", "id": 0, "method": "session/request_permission", "params": {
            "sessionId": "s1",
            "toolCall": { "toolCallId": "c2", "kind": kind, "title": "Edit `a.txt`",
                "rawInput": { "variant": "SearchReplace", "file_path": "a.txt", "old_string": "a", "new_string": "b" },
                "_meta": { "x.ai/tool": { "name": "search_replace", "kind": kind } } },
            "options": [
                { "optionId": "allow-edits-session", "name": "Yes, all edits", "kind": "allow_always" },
                { "optionId": "allow-once", "name": "Yes", "kind": "allow_once" },
                { "optionId": "reject-once", "name": "No", "kind": "reject_once" }
            ]
        } })
    }

    #[test]
    fn a_permission_request_becomes_a_prompt_and_its_answer_goes_back() {
        let mut a = open(Mode::Manual);
        let out = a.incoming(permission("edit"));
        let ask = handles(&out)[0].clone();
        assert_eq!(ask["request"]["subtype"], "can_use_tool");
        assert_eq!(ask["request"]["tool_name"], "Edit");
        assert_eq!(ask["request"]["input"]["file_path"], "a.txt");
        let answer = |behavior: Value| {
            json!({ "type": "control_response", "response": {
                "subtype": "success", "request_id": ask["request_id"], "response": behavior } })
        };
        let lines = a.outgoing(answer(json!({ "behavior": "allow", "updatedInput": {} })));
        assert_eq!(lines[0]["id"], 0);
        assert_eq!(lines[0]["result"]["outcome"]["optionId"], "allow-once");
        // For the session: the "always" option.
        a.incoming(permission("edit"));
        let session = json!({ "behavior": "allow", "updatedPermissions": [{ "type": "addRules" }] });
        assert_eq!(a.outgoing(answer(session))[0]["result"]["outcome"]["optionId"], "allow-edits-session");
        // A plain no stops the turn too.
        a.incoming(permission("edit"));
        let lines = a.outgoing(answer(json!({ "behavior": "deny", "interrupt": true })));
        assert_eq!(lines[0]["result"]["outcome"]["optionId"], "reject-once");
        assert_eq!(lines[1]["method"], "session/cancel");
    }

    #[test]
    fn modes_that_allow_edits_answer_for_themselves() {
        let mut a = open(Mode::AcceptEdits);
        let out = a.incoming(permission("edit"));
        assert_eq!(writes(&out)[0]["result"]["outcome"]["optionId"], "allow-once");
        assert!(handles(&out).is_empty());
        // A command still asks.
        assert_eq!(handles(&a.incoming(permission("execute")))[0]["type"], "control_request");
        // A live switch reaches Grok.
        let lines = a.outgoing(json!({ "type": "control_request", "request_id": "k1",
            "request": { "subtype": "set_permission_mode", "mode": "plan" } }));
        assert!(lines.iter().any(|l| l["method"] == "session/set_mode" && l["params"]["modeId"] == "plan"));
    }

    #[test]
    fn an_interrupt_cancels_and_withdraws_open_questions() {
        let mut a = open(Mode::Manual);
        a.incoming(permission("execute"));
        let lines =
            a.outgoing(json!({ "type": "control_request", "request_id": "k2", "request": { "subtype": "interrupt" } }));
        assert_eq!(lines[0]["result"]["outcome"]["outcome"], "cancelled");
        assert_eq!(lines[1]["method"], "session/cancel");
    }

    #[test]
    fn requests_kynex_does_not_serve_get_an_error() {
        let mut a = open(Mode::Manual);
        let out = a.incoming(json!({ "jsonrpc": "2.0", "id": 9, "method": "fs/read_text_file", "params": {} }));
        assert_eq!(writes(&out)[0]["error"]["code"], -32601);
        assert!(a.incoming(json!({ "jsonrpc": "2.0", "method": "_x.ai/queue/changed", "params": {} })).is_empty());
    }

    #[test]
    fn usage_reaches_the_session_as_context_and_cost() {
        let mut a = Acp::new(PathBuf::from("C:/work"), "", Mode::Manual, None);
        let init = a.start();
        a.incoming(json!({ "jsonrpc": "2.0", "id": init[0]["id"], "result": {} }));
        let new = a.outgoing(user("hi"));
        let models = json!({ "currentModelId": "grok-4.7",
            "availableModels": [{ "modelId": "grok-4.7", "_meta": { "totalContextTokens": 256000 } }] });
        let opened = json!({ "jsonrpc": "2.0", "id": new[0]["id"], "result": { "sessionId": "s1", "models": models } });
        let mut s = crate::session::Session::new(crate::session::Kind::Code, std::env::temp_dir());
        s.busy = true;
        fn feed(a: &mut Acp, s: &mut crate::session::Session, v: Value) {
            for out in a.incoming(v) {
                if let Out::Handle(v) = out {
                    s.handle(v);
                }
            }
        }
        feed(&mut a, &mut s, opened);
        // A reply streams, then its model call reports: the text settles once, the meter moves.
        let chunk = json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": "ok" } });
        feed(&mut a, &mut s, update(chunk));
        let call = json!({ "sessionUpdate": "response_completed",
            "usage": { "input_tokens": 20000, "output_tokens": 30, "cache_read_input_tokens": 19000 } });
        let note = json!({ "jsonrpc": "2.0", "method": "_x.ai/session_notification", "params": { "update": call } });
        feed(&mut a, &mut s, note);
        assert_eq!(s.context.tokens, 20000);
        let texts = s.items.iter().filter(|i| matches!(i, crate::session::Item::Assistant(_))).count();
        assert_eq!(texts, 1);
        let usage = json!({ "inputTokens": 20000, "outputTokens": 30, "cachedReadTokens": 19000,
            "costUsdTicks": 100_000_000 });
        let prompt = a.prompt.unwrap().0;
        let done = json!({ "jsonrpc": "2.0", "id": prompt,
            "result": { "stopReason": "end_turn", "_meta": { "usage": usage } } });
        feed(&mut a, &mut s, done);
        assert!(!s.busy);
        assert_eq!(s.context.window, Some(256_000));
        assert!((s.total_cost - 0.01).abs() < 1e-9);
        assert_eq!((s.spent.input, s.spent.cache_read, s.spent.output), (1000, 19000, 30));
        let texts = s.items.iter().filter(|i| matches!(i, crate::session::Item::Assistant(_))).count();
        assert_eq!(texts, 1, "the text isn't repeated at the turn's end");
    }

    #[test]
    fn models_and_effort_reach_grok_and_the_picker() {
        let mut a = Acp::new(PathBuf::from("C:/work"), "", Mode::Manual, None).with_effort("low");
        let init = a.start();
        a.incoming(json!({ "jsonrpc": "2.0", "id": init[0]["id"], "result": {} }));
        let new = a.outgoing(user("hi"));
        let efforts = json!([{ "value": "high", "default": true }, { "value": "low", "default": false }]);
        let models = json!({ "currentModelId": "grok-4.7", "availableModels": [
            { "modelId": "grok-4.7", "name": "Grok 4.7", "_meta": { "reasoningEfforts": efforts } },
            { "modelId": "grok-4.5", "name": "Grok 4.5", "_meta": { "reasoningEfforts": [] } }] });
        let out = a.incoming(
            json!({ "jsonrpc": "2.0", "id": new[0]["id"], "result": { "sessionId": "s1", "models": models } }),
        );
        let effort = |lines: &[&Value]| {
            lines.iter().find(|w| w["method"] == "session/set_config_option").map(|w| w["params"]["value"].clone())
        };
        assert_eq!(effort(&writes(&out)), Some(json!("low")));
        let list = a.take_models().unwrap();
        assert_eq!((list[0].value.as_str(), list[0].efforts.len()), ("grok-4.7", 2));
        assert!(a.take_models().is_none(), "taken once");
        // Back to the default: the model's own.
        let lines = a.set_effort("");
        assert_eq!(effort(&lines.iter().collect::<Vec<_>>()), Some(json!("high")));
        // A level the model doesn't take isn't sent.
        assert!(a.set_effort("max").is_empty());
        // A switch goes live.
        let lines = a.set_model("grok-4.5");
        assert_eq!(lines[0]["method"], "session/set_model");
        assert_eq!(lines[0]["params"]["modelId"], "grok-4.5");
    }

    #[test]
    fn images_go_inline() {
        let content = json!([
            { "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": "AAAA" } },
            { "type": "text", "text": "what is this" }
        ]);
        let blocks = prompt_blocks(&content);
        assert_eq!(blocks[0], json!({ "type": "image", "mimeType": "image/png", "data": "AAAA" }));
        assert_eq!(blocks[1]["text"], "what is this");
    }

    /// What `agy acp` sent in a real session on OpenRouter (agy 1.5.12), trimmed.
    #[test]
    fn antigravity_speaks_plain_acp_with_named_models() {
        let mut a = Acp::new(PathBuf::from("C:/work"), "google/gemini-4-carbon", Mode::AcceptEdits, None)
            .for_agent(Agent::Antigravity);
        let init = a.start();
        a.incoming(json!({ "jsonrpc": "2.0", "id": init[0]["id"], "result": {} }));
        let new = a.outgoing(user("hi"));
        assert_eq!(new[0]["method"], "session/new");
        assert!(new[0]["params"].get("_meta").is_none(), "no Grok extras");
        let out = a.incoming(json!({ "jsonrpc": "2.0", "id": new[0]["id"], "result": { "sessionId": "s1",
            "models": { "availableModels": [
                { "modelId": "MODEL_PLACEHOLDER_M364", "name": "google/gemini-4-hydrogen" },
                { "modelId": "MODEL_PLACEHOLDER_M365", "name": "google/gemini-4-carbon" }] } } }));
        let w = writes(&out);
        assert!(!w.iter().any(|w| w["method"].as_str().is_some_and(|m| m.starts_with("_x.ai"))));
        let mode = w.iter().find(|w| w["method"] == "session/set_mode").unwrap();
        assert_eq!(mode["params"]["modeId"], "accept-edits");
        // The model goes by its placeholder id on the wire, and by its name in the picker.
        let model = w.iter().find(|w| w["method"] == "session/set_model").unwrap();
        assert_eq!(model["params"]["modelId"], "MODEL_PLACEHOLDER_M365");
        assert!(w.iter().any(|w| w["method"] == "session/prompt"));
        assert_eq!(a.take_models().unwrap()[0].value, "google/gemini-4-hydrogen");
        assert_eq!(a.set_model("google/gemini-4-hydrogen")[0]["params"]["modelId"], "MODEL_PLACEHOLDER_M364");

        // Its shell tool reads as Bash, with the command where Claude's goes.
        let call = json!({ "sessionUpdate": "tool_call", "toolCallId": "c1", "name": "run_command",
            "status": "in_progress", "kind": "execute", "title": "echo done",
            "rawInput": { "CommandLine": "echo done", "WaitMsBeforeAsync": 5000 } });
        let out = a.incoming(update(call));
        let used = handles(&out)[0]["message"]["content"][0].clone();
        assert_eq!((used["name"].as_str(), used["input"]["command"].as_str()), (Some("Bash"), Some("echo done")));
        // A write reads as Write.
        let call = json!({ "sessionUpdate": "tool_call", "toolCallId": "c2", "name": "write_file",
            "status": "in_progress", "kind": "edit", "rawInput": { "Content": "hi", "Path": "C:/work/a.txt" } });
        let out = a.incoming(update(call));
        let used = handles(&out)[0]["message"]["content"][0].clone();
        assert_eq!(used["name"], "Write");
        assert_eq!(
            (used["input"]["file_path"].as_str(), used["input"]["content"].as_str()),
            (Some("C:/work/a.txt"), Some("hi"))
        );

        // The context meter moves on `usage_update`, and the turn's tokens come on its result.
        let out = a.incoming(update(json!({ "sessionUpdate": "usage_update", "used": 8904, "size": 200000 })));
        assert_eq!(handles(&out)[0]["message"]["usage"]["input_tokens"], 8904);
        let prompt = new_prompt_id(&a);
        let out = a.incoming(json!({ "jsonrpc": "2.0", "id": prompt, "result": { "stopReason": "end_turn",
            "usage": { "inputTokens": 13320, "outputTokens": 243, "cachedReadTokens": 13785 } } }));
        let result = handles(&out).into_iter().find(|h| h["type"] == "result").unwrap().clone();
        let u = &result["model_usage_since_last_result"]["google/gemini-4-hydrogen"];
        assert_eq!(u["inputTokens"], 13320);
        assert_eq!(result["modelUsage"]["google/gemini-4-hydrogen"]["contextWindow"], 200000);
    }

    /// The id of the `session/prompt` in flight.
    fn new_prompt_id(a: &Acp) -> u64 {
        a.prompt.map(|(id, _)| id).unwrap()
    }

    /// Antigravity's prompt: rules decide first, reads go, and "always" is kept here.
    #[test]
    fn antigravity_asks_follow_the_rules_and_keep_always() {
        let perms = crate::project::Permissions {
            allow: vec!["Bash(cargo test:*)".into()],
            deny: vec!["Bash(git push:*)".into()],
        };
        let mut a =
            Acp::new(PathBuf::from("C:/work"), "", Mode::Manual, None).for_agent(Agent::Antigravity).with_rules(&perms);
        let ask = |id: u64, cmd: &str| {
            json!({ "jsonrpc": "2.0", "id": id.to_string(), "method": "session/request_permission", "params": {
                "toolCall": { "toolCallId": format!("c{id}"), "name": "run_command", "kind": "execute",
                    "rawInput": { "CommandLine": cmd } },
                "options": [{ "optionId": "allow", "kind": "allow_once" }, { "optionId": "deny", "kind": "reject_once" }] } })
        };
        let picked = |out: &[Out]| writes(out).first().map(|w| w["result"]["outcome"]["optionId"].clone());
        // A read goes, an allow rule goes, a deny rule is refused: no prompt.
        assert_eq!(
            picked(&a.incoming(ask(1, "Get-ChildItem -Recurse | Select-Object -First 5"))),
            Some(json!("allow"))
        );
        assert_eq!(picked(&a.incoming(ask(2, "cargo test -p core"))), Some(json!("allow")));
        assert_eq!(picked(&a.incoming(ask(3, "git push origin main"))), Some(json!("deny")));
        // Anything else asks, offering to keep its program's rule.
        let out = a.incoming(ask(4, "npm run build"));
        let req = handles(&out).into_iter().find(|h| h["type"] == "control_request").unwrap().clone();
        let rules = crate::project::suggested_rules(&req["request"]["permission_suggestions"]);
        assert_eq!(rules, ["Bash(npm run:*)"]);
        // Allowed for the session: the next one like it doesn't ask.
        let answer = json!({ "type": "control_response", "response": { "request_id": req["request_id"],
            "response": { "behavior": "allow", "updatedPermissions": req["request"]["permission_suggestions"] } } });
        assert_eq!(a.outgoing(answer)[0]["result"]["outcome"]["optionId"], "allow");
        assert_eq!(picked(&a.incoming(ask(5, "npm run lint"))), Some(json!("allow")));
        // Grok keeps its own prompts.
        let mut g = Acp::new(PathBuf::from("C:/work"), "", Mode::Manual, None).with_rules(&perms);
        assert!(writes(&g.incoming(ask(6, "Get-ChildItem"))).is_empty());
    }
}
