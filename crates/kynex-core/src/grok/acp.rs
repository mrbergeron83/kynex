//! The Agent Client Protocol (ACP) as `grok agent stdio` speaks it, turned
//! into the stream-json a session already handles, and back.
//!
//! The session talks to it as it talks to `claude`: [`Acp::outgoing`] takes the
//! messages it would write (a user message, a permission answer, an interrupt,
//! a mode switch) and gives the JSON-RPC lines to write instead;
//! [`Acp::incoming`] takes a line from `grok` and gives stream-json events to
//! handle, errors to show, and lines to write back (answers Kynex gives itself).
//!
//! One process holds one conversation: `initialize`, then `session/new` (or
//! `session/load` to continue one) when the first message goes, then a
//! `session/prompt` per message. Grok's own extras (`_x.ai/…`) are left out.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Instant;

use serde_json::{Value, json};

use crate::project::Mode;

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

/// One `grok agent stdio` conversation.
#[derive(Debug)]
pub struct Acp {
    cwd: PathBuf,
    /// The model to switch to once the session is open; empty is Grok's default.
    model: String,
    mode: Mode,
    /// A chief's worker: nobody to ask, so nothing asks.
    worker: bool,
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
    /// The models Grok offers, the one in use and their context windows.
    models: super::usage::Models,
    /// What the session's turns have cost so far, in USD.
    total_cost: f64,
    /// The reasoning effort to set once the session is open; empty is the model's own.
    effort: String,
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
    pub fn new(cwd: PathBuf, model: &str, mode: Mode, worker: bool, resume: Option<&str>) -> Self {
        Self {
            cwd,
            model: model.trim().to_string(),
            mode,
            worker,
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
        }
    }

    /// With a reasoning effort (`low`, `high`…) to set on the session; empty is the model's own.
    pub fn with_effort(mut self, effort: &str) -> Self {
        self.effort = effort.trim().to_string();
        self
    }

    /// Grok's models, when the list came or changed since last taken, for the model picker.
    pub fn take_models(&mut self) -> Option<Vec<crate::session::ModelInfo>> {
        std::mem::take(&mut self.models.fresh).then(|| self.models.list.clone())
    }

    /// Switches the open session to `model` (empty: leaves it as it is); before it opens, it's used then.
    pub fn set_model(&mut self, model: &str) -> Vec<Value> {
        self.model = model.trim().to_string();
        let Some(sid) = self.session.clone().filter(|_| !self.model.is_empty()) else { return Vec::new() };
        self.models.current = self.model.clone();
        let id = self.id();
        let mut lines = vec![request(id, "session/set_model", json!({ "sessionId": sid, "modelId": self.model }))];
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
    /// for Grok and runs no terminals for it; Grok does both itself.
    pub fn start(&mut self) -> Vec<Value> {
        let id = self.id();
        self.init = Some(id);
        vec![request(
            id,
            "initialize",
            json!({
                "protocolVersion": 1,
                "clientCapabilities": { "fs": { "readTextFile": false, "writeTextFile": false }, "terminal": false },
                "_meta": { "clientIdentifier": "kynex" },
            }),
        )]
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
        // Asking is the default; `yoloMode` approves everything (a worker), `autoMode` is Grok's auto.
        let meta = json!({ "yoloMode": self.worker, "autoMode": !self.worker && self.mode == Mode::Auto });
        let cwd = self.cwd.to_string_lossy();
        match &self.resume {
            Some(sid) => {
                self.loading = true;
                let params = json!({ "sessionId": sid, "cwd": cwd, "mcpServers": [], "_meta": meta });
                vec![request(id, "session/load", params)]
            }
            None => vec![request(id, "session/new", json!({ "cwd": cwd, "mcpServers": [], "_meta": meta }))],
        }
    }

    /// The lines that put an open session in `self.mode`.
    fn mode_lines(&mut self) -> Vec<Value> {
        let Some(sid) = self.session.clone() else { return Vec::new() };
        let word = match () {
            _ if self.worker => "always-approve",
            _ if self.mode == Mode::Auto => "auto",
            _ => "ask",
        };
        let id = self.id();
        vec![
            notification(
                "_x.ai/yolo_mode_changed",
                json!({ "yolo_mode": self.worker, "auto_mode": word == "auto", "permission_mode": word }),
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
                let Some(usage) = super::usage::context(&self.models.current, &v["params"]["update"]["usage"]) else {
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
                return vec![Out::Error(format!("Grok didn't start: {e}"))];
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
                    out.push(Out::Error(format!("Couldn't continue the Grok session ({e}); starting a new one.")));
                    out.extend(self.open().into_iter().map(Out::Write));
                } else {
                    out.push(Out::Error(format!("Grok couldn't start a session: {e}")));
                    self.pending.clear();
                    out.push(Out::Handle(json!({ "type": "result", "is_error": true, "duration_ms": 0 })));
                }
                return out;
            }
            let sid = v["result"]["sessionId"].as_str().map(str::to_string).or_else(|| self.resume.clone());
            self.session = sid.clone();
            self.models.read(&v["result"]["models"]);
            if !self.model.is_empty() {
                self.models.current = self.model.clone();
            }
            if let Some(sid) = &sid {
                out.push(Out::Handle(json!({ "type": "grok_session", "session_id": sid })));
            }
            out.extend(self.mode_lines().into_iter().map(Out::Write));
            if !self.model.is_empty() {
                let id = self.id();
                let params = json!({ "sessionId": sid, "modelId": self.model });
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
                out.push(Out::Error(format!("Grok: {e}")));
            } else if stop == "refusal" {
                out.push(Out::Error("Grok refused this request.".into()));
            } else if matches!(stop, "max_tokens" | "max_turn_requests") {
                out.push(Out::Error(format!("Grok stopped early ({}).", stop.replace('_', " "))));
            }
            let mut result = super::usage::turn(&v["result"]["_meta"]["usage"], &self.models, &mut self.total_cost);
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
        if self.worker || (kind == "edit" && matches!(self.mode, Mode::AcceptEdits | Mode::Auto)) {
            return vec![Out::Write(choose(v["id"].clone(), &options, &["allow_once", "allow_always"]))];
        }
        let request_id = format!("acp-{}", v["id"]);
        self.asks.insert(request_id.clone(), (v["id"].clone(), options));
        let mut out = self.close();
        let (name, input) = tool(call);
        out.push(Out::Handle(json!({
            "type": "control_request",
            "request_id": request_id,
            "request": {
                "subtype": "can_use_tool",
                "tool_name": name,
                "input": input,
                "description": call["title"].as_str().unwrap_or(""),
            },
        })));
        out
    }
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

/// A tool call as the transcript names it: Grok's shell, edit and read tools
/// under Claude's names, so they draw the same (a command, a diff, a path).
fn tool(call: &Value) -> (String, Value) {
    let grok = call["_meta"]["x.ai/tool"]["name"].as_str().or(call["title"].as_str()).unwrap_or("tool");
    let mut input = match &call["rawInput"] {
        Value::Object(m) => Value::Object(m.clone()),
        Value::Null => json!({}),
        other => other.clone(),
    };
    if let Some(m) = input.as_object_mut() {
        m.remove("variant");
    }
    let name = match grok {
        "run_terminal_command" | "run_terminal_cmd" => "Bash",
        "search_replace" => "Edit",
        "read_file" => {
            if let Some(m) = input.as_object_mut()
                && let Some(path) = m.remove("target_file")
            {
                m.insert("file_path".into(), path);
            }
            "Read"
        }
        other => other,
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
        let mut a = Acp::new(PathBuf::from("C:/work"), "", mode, false, None);
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
        let mut a = Acp::new(PathBuf::from("C:/work"), "grok-4.7", Mode::Manual, false, None);
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
        let mut a = Acp::new(PathBuf::from("C:/work"), "", Mode::Manual, false, Some("old"));
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
        let mut a = Acp::new(PathBuf::from("C:/work"), "", Mode::Manual, false, Some("gone"));
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
        let mut a = Acp::new(PathBuf::from("C:/work"), "", Mode::Manual, false, None);
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
        let mut a = Acp::new(PathBuf::from("C:/work"), "", Mode::Manual, false, None).with_effort("low");
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
}
