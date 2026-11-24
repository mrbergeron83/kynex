//! What the process says, line by line: the transcript, its usage, its prompts.

use std::time::Instant;

use serde_json::{Value, json};

use super::*;

impl Session {
    pub(crate) fn handle(&mut self, v: Value) {
        // The CLI started a turn of its own for a steer that came too late for the last one.
        if self.late_steer
            && !self.busy
            && matches!(v["type"].as_str(), Some("assistant" | "stream_event"))
            && v["parent_tool_use_id"].is_null()
        {
            self.late_steer = false;
            self.busy = true;
            self.finished = false;
        }
        if let Some(id) = v["session_id"].as_str() {
            self.session_id = Some(id.to_string());
        }
        // `system` init and status messages report the mode, e.g. after a plan is approved.
        if let Some(mode) = v["permissionMode"].as_str().and_then(project::Mode::from_cli)
            && self.is_running()
        {
            self.mode = mode;
        }
        match v["type"].as_str().unwrap_or("") {
            "stream_event" => self.handle_stream(&v["event"]),
            "control_response"
                if v["response"]["request_id"]
                    .as_str()
                    .is_some_and(|r| self.mode_requests.iter().any(|m| m.0 == r)) =>
            {
                self.mode_answered(&v["response"]);
            }
            "control_response" if v["response"]["request_id"] == INIT_ID => {
                let r = &v["response"]["response"];
                self.init = Some(Init { commands: crate::claude::commands::from_init(r), models: models_from_init(r) });
                self.process_up();
            }
            // The answer to a Stop: on success the task's end follows as a notification.
            "control_response"
                if v["response"]["request_id"].as_str().is_some_and(|r| self.stopping.contains_key(r)) =>
            {
                let request = v["response"]["request_id"].as_str().unwrap_or("");
                let task = self.stopping.remove(request).unwrap_or_default();
                if v["response"]["subtype"] == "error" {
                    if let Some(t) = self.tasks.running.iter_mut().find(|t| t.id == task) {
                        t.stopping = false;
                    }
                    let why = v["response"]["error"].as_str().unwrap_or("the CLI refused");
                    self.items.push(Item::Error(format!("Couldn't stop the background task: {why}")));
                }
            }
            // A server restarted, or how they all are now.
            "control_response"
                if v["response"]["request_id"].as_str().is_some_and(|r| self.mcp_requests.iter().any(|m| m == r)) =>
            {
                self.mcp_requests.retain(|m| v["response"]["request_id"] != m.as_str());
                let r = &v["response"];
                if r["subtype"] == "error" {
                    let why = r["error"].as_str().unwrap_or("the CLI refused");
                    self.items.push(Item::Error(format!("Couldn't restart the MCP server: {why}")));
                } else if let Some(list) = r["response"]["mcpServers"].as_array() {
                    self.mcp = crate::claude::mcp::servers(&json!({ "mcp_servers": list }));
                }
            }
            // A CLI that can't take settings live (an older one) gets them on a restart.
            "control_response"
                if v["response"]["request_id"].as_str().is_some_and(|r| self.applying.iter().any(|a| a == r)) =>
            {
                self.applying.retain(|a| v["response"]["request_id"] != a.as_str());
                if v["response"]["subtype"] == "error" {
                    self.stale = true;
                }
            }
            "rate_limit_event" => {
                if let Some(limits) = crate::claude::usage::parse(&v) {
                    self.limits = Some(limits);
                }
            }
            // `/compact` finished: the context is down to its summary.
            // Without a size after it, the meter waits for the next reply.
            "system" if v["subtype"] == "compact_boundary" => {
                self.context.tokens = v["compact_metadata"]["post_tokens"].as_u64().unwrap_or(0);
            }
            // Each process start reports its MCP servers and whether they connected.
            "system" if v["subtype"] == "init" => {
                self.mcp = crate::claude::mcp::servers(&v);
                self.mcp_tools = crate::claude::mcp::tools(&v);
                self.ultrathink.follow(&v);
            }
            "system" if v["subtype"] == "per_turn_effort_changed" => self.ultrathink.follow(&v),
            // A task that ended shows in the transcript, and the session wants
            // you even if Claude's turn about it says nothing.
            "system" => {
                if let Some(notice) = self.tasks.apply(&v) {
                    self.items.push(Item::Task(notice));
                    self.finished = true;
                }
            }
            "assistant" => {
                // Subagents' replies have their own context; only the main thread's counts.
                // Synthetic ones (a slash command's answer, an error) carry no real usage.
                let usage = &v["message"]["usage"];
                if v["parent_tool_use_id"].is_null() && usage.is_object() && v["message"]["model"] != "<synthetic>" {
                    let n = |k: &str| usage[k].as_u64().unwrap_or(0);
                    if let Some(model) = v["message"]["model"].as_str() {
                        self.reply_model = model.to_string();
                    }
                    self.context = ContextUse {
                        tokens: n("input_tokens") + n("cache_creation_input_tokens") + n("cache_read_input_tokens"),
                        window: known_window(&self.reply_model).or(self.context.window),
                    };
                }
                // OpenRouter refused the request: the model takes no tool calls.
                if v["message"]["model"] == "<synthetic>"
                    && self.is_running()
                    && self.provider.provider().is_openrouter()
                {
                    let said =
                        v["message"]["content"].as_array().into_iter().flatten().filter_map(|b| b["text"].as_str());
                    if said.into_iter().any(crate::openrouter::says_no_tools) {
                        self.no_tools = self.provider.with_model(&self.model).model().map(str::to_string);
                    }
                }
                for block in v["message"]["content"].as_array().into_iter().flatten() {
                    match block["type"].as_str() {
                        // Already rendered live from the stream deltas.
                        Some("text") if self.streaming_text => {}
                        Some("text") => {
                            let text = block["text"].as_str().unwrap_or("");
                            // Hit live: say what the daily free cap is and when it resets.
                            let text = if v["message"]["model"] == "<synthetic>"
                                && self.is_running()
                                && self.provider.provider().is_openrouter()
                                && crate::openrouter::says_free_daily_limit(text)
                            {
                                format!(
                                    "{text}\n\n{}",
                                    crate::openrouter::free_daily_limit_help(crate::claude::usage::now())
                                )
                            } else {
                                text.to_string()
                            };
                            self.items.push(Item::Assistant(text));
                        }
                        // Streamed in already, or the block right before: finish
                        // that one. Otherwise (a log, no partial messages) it's new.
                        Some("thinking") => {
                            let text = block["thinking"].as_str().unwrap_or("");
                            match self.items.last_mut() {
                                Some(Item::Thinking(t)) => t.finish(text),
                                _ => self.items.push(Item::Thinking(Thought::logged(text))),
                            }
                        }
                        Some("tool_use") => {
                            let name = block["name"].as_str().unwrap_or("tool").to_string();
                            let summary = tool_summary(&name, &block["input"]);
                            let input = serde_json::to_string_pretty(&block["input"]).unwrap_or_default();
                            let id = block["id"].as_str().unwrap_or("").to_string();
                            if self.is_running()
                                && let Some((_, tool)) = crate::claude::mcp::split(&name)
                                && crate::claude::mcp::is_browser(tool)
                            {
                                self.browser = true;
                            }
                            // Streamed in already: it gets its whole input.
                            if let Some(Item::ToolUse { name: n, summary: s, input: i, .. }) = self.streamed_call(&id) {
                                (*n, *s, *i) = (name, summary, input);
                                self.revision += 1;
                                continue;
                            }
                            // Only a call watched live has a start; a log's are long done.
                            let started = self.is_running().then(Instant::now);
                            let timing = Timing { started, took: None };
                            self.items.push(Item::ToolUse { id, name, summary, input, timing });
                        }
                        _ => {}
                    }
                }
                self.streaming_text = false;
                self.answered = true;
            }
            "user" => {
                for (k, block) in v["message"]["content"].as_array().into_iter().flatten().enumerate() {
                    if block["type"] == "tool_result" {
                        self.quiet_since = Instant::now();
                        let id = block["tool_use_id"].as_str().unwrap_or("").to_string();
                        let content = tool_result_text(&block["content"]);
                        self.tasks.tool_result(&id, &content);
                        self.finish_call(&id);
                        // Read back from a log, screenshots stay there until shown.
                        let images = match &self.origin {
                            Some(origin) => {
                                logged_in(&block["content"], origin, &format!("/message/content/{k}/content"))
                            }
                            None => previews_in(&block["content"]),
                        };
                        self.items.push(Item::ToolResult {
                            id,
                            content,
                            images,
                            is_error: block["is_error"].as_bool().unwrap_or(false),
                        });
                    }
                }
            }
            "control_request" if v["request"]["subtype"] == "can_use_tool" => {
                self.items.push(Item::Permission {
                    request_id: v["request_id"].as_str().unwrap_or("").into(),
                    tool: v["request"]["tool_name"]
                        .as_str()
                        .or(v["request"]["display_name"].as_str())
                        .unwrap_or("tool")
                        .into(),
                    input: v["request"]["input"].clone(),
                    suggestions: v["request"]["permission_suggestions"].clone(),
                    description: v["request"]["description"].as_str().unwrap_or("").into(),
                    why: crate::claude::why::explain(&v["request"]),
                    state: PermState::Pending,
                });
            }
            // One this engine doesn't know: refused, so the CLI doesn't wait on it forever.
            "control_request" => {
                let subtype = v["request"]["subtype"].as_str().unwrap_or("unknown");
                self.write(json!({
                    "type": "control_response",
                    "response": {
                        "subtype": "error",
                        "request_id": v["request_id"].clone(),
                        "error": format!("Kynex doesn't handle the `{subtype}` request"),
                    }
                }));
            }
            // An error outside any turn's messages (a failed request, a bad line).
            "error" => {
                let error = &v["error"];
                let text = error["message"]
                    .as_str()
                    .or(error.as_str())
                    .or(v["message"].as_str())
                    .map(str::to_string)
                    .unwrap_or_else(|| v.to_string());
                self.items.push(Item::Error(text));
            }
            // The CLI dropped a request it sent, e.g. on an interrupt.
            "control_cancel_request" => {
                let id = v["request_id"].as_str().unwrap_or("");
                self.cancel_permissions(|r| r == id);
            }
            "result" => {
                if let Some(models) = v["modelUsage"].as_object()
                    && let Ok(mut known) = windows().lock()
                {
                    for (model, u) in models {
                        if let Some(w) = u["contextWindow"].as_u64() {
                            known.insert(model.clone(), w);
                        }
                    }
                }
                // The reply's model, else the biggest window reported (side models like
                // Haiku are smaller, and a `[1m]` id may not match the reply's model).
                let biggest = v["modelUsage"]
                    .as_object()
                    .and_then(|m| m.values().filter_map(|u| u["contextWindow"].as_u64()).max());
                self.context.window = known_window(&self.reply_model).or(biggest).or(self.context.window);
                let cost = v["total_cost_usd"].as_f64().unwrap_or(0.0);
                self.run_cost = cost.max(self.run_cost);
                self.total_cost = self.carried_cost + self.run_cost;
                // Kept for when it's reopened. Only for a process that really ran (not tests).
                if let Some(id) = self.session_id.clone().filter(|_| self.runs > 0 && self.total_cost > 0.0) {
                    save_cost(id, self.total_cost);
                }
                self.spent.add_turn(&v);
                self.settle_thoughts();
                let (tools, files) = self.turn_recap();
                self.items.push(Item::Result {
                    cost,
                    duration_ms: v["duration_ms"].as_u64().unwrap_or(0),
                    is_error: v["is_error"].as_bool().unwrap_or(false),
                    tools,
                    files,
                });
                self.busy = false;
                self.finished = true;
                self.late_steer = std::mem::take(&mut self.steered);
                if !self.hold_queue
                    && let Some(next) = self.queue.take_all()
                {
                    self.send(next.text, next.attachments);
                }
            }
            _ => {}
        }
    }

    /// Tool calls since the last user message, and the paths they wrote to.
    pub(super) fn turn_recap(&self) -> (usize, Vec<String>) {
        let start = self.items.iter().rposition(|i| matches!(i, Item::User { .. })).map_or(0, |i| i + 1);
        let mut tools = 0;
        let mut files: Vec<String> = Vec::new();
        for item in &self.items[start..] {
            let Item::ToolUse { name, input, .. } = item else { continue };
            tools += 1;
            if !matches!(
                name.as_str(),
                "Edit" | "MultiEdit" | "Write" | "NotebookEdit" | "search_replace" | "StrReplace" | "write"
            ) {
                continue;
            }
            let v: Value = serde_json::from_str(input).unwrap_or_default();
            let path = v["file_path"].as_str().or(v["path"].as_str()).or(v["notebook_path"].as_str()).unwrap_or("");
            if !path.is_empty() && !files.iter().any(|f| f == path) {
                files.push(path.to_string());
            }
        }
        (tools, files)
    }

    /// Thinking and tool calls cut short (the turn was stopped, the process
    /// ended) stop counting where they are.
    pub(super) fn settle_thoughts(&mut self) {
        for item in self.items.iter_mut().rev().take_while(|i| !matches!(i, Item::Result { .. })) {
            match item {
                Item::Thinking(t) if t.is_live() => t.finish(""),
                Item::ToolUse { timing, .. } => timing.finish(),
                _ => {}
            }
        }
    }

    /// Call `id`'s result came: its time stops.
    pub(super) fn finish_call(&mut self, id: &str) {
        let call = self.items.iter_mut().rev().find_map(|i| match i {
            Item::ToolUse { id: c, timing, .. } if c == id => Some(timing),
            _ => None,
        });
        if let Some(timing) = call {
            timing.finish();
        }
    }

    /// The process said something: it's up. A message already sent now waits on Claude.
    pub(super) fn process_up(&mut self) {
        if !self.ready {
            self.ready = true;
            if self.busy && !self.answered {
                self.quiet_since = Instant::now();
            }
        }
    }

    /// The call `id` this turn, if it streamed in before its message came whole.
    pub(super) fn streamed_call(&mut self, id: &str) -> Option<&mut Item> {
        if id.is_empty() {
            return None;
        }
        let mut turn = self.items.iter_mut().rev().take_while(|i| !matches!(i, Item::User { .. }));
        turn.find(|i| matches!(i, Item::ToolUse { id: c, .. } if c == id))
    }

    pub(super) fn handle_stream(&mut self, ev: &Value) {
        match ev["type"].as_str() {
            Some("content_block_start") => {
                self.answered = true;
                self.in_block = true;
                let block = &ev["content_block"];
                match block["type"].as_str() {
                    Some("text") => {
                        self.items.push(Item::Assistant(String::new()));
                        self.streaming_text = true;
                    }
                    Some("thinking") => self.items.push(Item::Thinking(Thought::live())),
                    // A call shows as soon as Claude starts writing it, named, with
                    // its target once that much of the input is in.
                    Some("tool_use") => {
                        self.tool_input.clear();
                        self.items.push(Item::ToolUse {
                            id: block["id"].as_str().unwrap_or("").into(),
                            name: block["name"].as_str().unwrap_or("tool").into(),
                            summary: String::new(),
                            input: String::new(),
                            timing: Timing { started: Some(Instant::now()), took: None },
                        });
                    }
                    // Redacted thinking and the like: nothing to show, so the pulse does.
                    _ => self.in_block = false,
                }
            }
            Some("content_block_delta") => match self.items.last_mut() {
                Some(Item::Assistant(s)) => s.push_str(ev["delta"]["text"].as_str().unwrap_or("")),
                Some(Item::Thinking(t)) => t.text.push_str(ev["delta"]["thinking"].as_str().unwrap_or("")),
                // Big inputs (a `Write`) stream long: read the target from the first
                // part, and stop reading once it's found or it's clearly not coming.
                Some(Item::ToolUse { name, summary, input, .. })
                    if summary.is_empty() && self.tool_input.len() < PARTIAL_INPUT_MAX =>
                {
                    self.tool_input.push_str(ev["delta"]["partial_json"].as_str().unwrap_or(""));
                    if let Some(v) = crate::claude::pulse::partial_input(&self.tool_input) {
                        *summary = tool_summary(name, &v);
                        *input = serde_json::to_string_pretty(&v).unwrap_or_default();
                        self.revision += 1;
                    }
                }
                _ => {}
            },
            Some("content_block_stop") => {
                self.in_block = false;
                self.quiet_since = Instant::now();
                self.tool_input.clear();
                if let Some(Item::Thinking(t)) = self.items.last_mut() {
                    t.finish("");
                }
            }
            _ => {}
        }
    }
}

/// A user-role log entry the CLI wrote itself, like the `<task-notification>`
/// it sends Claude when a background command ends: Claude reads it, but it
/// isn't something the user said.
pub(crate) fn written_by_cli(v: &Value) -> bool {
    let text = v["message"]["content"].as_str().unwrap_or("");
    v["promptSource"] == "system" || v["origin"]["kind"].is_string() || text.starts_with("<task-notification>")
}

/// One line for a tool call: Claude's own `description` of it when there is one
/// (Bash, PowerShell, Agent…), else the most telling argument.
pub(super) fn tool_summary(name: &str, input: &Value) -> String {
    let field = match name {
        _ if input["description"].as_str().is_some_and(|d| !d.trim().is_empty()) => "description",
        "Bash" | "PowerShell" | "run_terminal_cmd" | "run_terminal_command" | "Shell" => "command",
        "Read" | "Write" | "Edit" | "NotebookEdit" | "read_file" | "write" | "search_replace" | "StrReplace"
            if input["file_path"].is_string() =>
        {
            "file_path"
        }
        "Read" | "Write" | "Edit" | "NotebookEdit" | "read_file" | "write" | "search_replace" | "StrReplace"
        | "list_dir" => "path",
        "Glob" | "Grep" | "glob" | "grep" => "pattern",
        "WebFetch" => "url",
        "WebSearch" => "query",
        "Task" | "Agent" | "spawn_subagent" => "description",
        "TodoWrite" => return crate::claude::todos::summary(input),
        "TaskCreate" | "TaskUpdate" => "subject",
        // MCP calls read as what they do: "Click “Sign in”".
        _ => return crate::claude::mcp::describe(name, input).map(|(_, target)| target).unwrap_or_default(),
    };
    let s = input[field].as_str().unwrap_or("");
    let first = s.lines().next().unwrap_or("");
    if first.chars().count() > 120 {
        format!("{}…", first.chars().take(120).collect::<String>())
    } else {
        first.into()
    }
}

pub(super) fn tool_result_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts.iter().filter_map(|p| p["text"].as_str()).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
}

/// Saves `total` as what session `id` has cost so far ([`crate::claude::costs`]), on a
/// thread of its own so the caller doesn't wait on the disk.
fn save_cost(id: String, total: f64) {
    use crate::claude::costs;
    let save = move || {
        let mut cost = costs::load(&id);
        cost.total_usd = total;
        cost.turns += 1;
        let _ = costs::save(&id, cost);
    };
    let _ = std::thread::Builder::new().name("save-cost".into()).spawn(save);
}
