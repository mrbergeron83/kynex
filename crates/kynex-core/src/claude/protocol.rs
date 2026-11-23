//! Pure, panic-free NDJSON frame builders and parser for the Claude Code
//! bidirectional `stream-json` protocol.

use std::collections::{HashMap, HashSet};

use serde_json::{Map, Value, json};

use super::types::*;

/// Cap partial tool-input accumulation per content block to protect memory on
/// malformed or huge streams.
const MAX_TOOL_INPUT_BUF: usize = 8 * 1024 * 1024;

// ---------------------------------------------------------------------------
// Outgoing frame builders
// ---------------------------------------------------------------------------

fn control_req(request_id: &str, request: Value) -> String {
    json!({ "type": "control_request", "request_id": request_id, "request": request }).to_string()
}

pub fn build_initialize(request_id: &str) -> String {
    control_req(request_id, json!({ "subtype": "initialize" }))
}

pub fn build_user_message(blocks: &[PromptBlock]) -> String {
    let content: Vec<Value> = blocks
        .iter()
        .map(|b| match b {
            PromptBlock::Text(text) => json!({ "type": "text", "text": text }),
            PromptBlock::Image { media_type, base64 } => json!({
                "type": "image",
                "source": { "type": "base64", "media_type": media_type, "data": base64 }
            }),
            PromptBlock::FileContext { path, content } => {
                json!({ "type": "text", "text": format!("<file path=\"{path}\">\n{content}\n</file>") })
            }
        })
        .collect();
    json!({ "type": "user", "message": { "role": "user", "content": content } }).to_string()
}

/// Builds the `control_response` answering a `can_use_tool` request.
///
/// `orig_input` should be the original tool input received in the matching
/// `control_request` so `updatedInput` preserves required fields (especially
/// for `AskUserQuestion` where `answers` is merged alongside `questions`).
pub fn build_permission_response(request_id: &str, answer: &PermissionAnswer, orig_input: Option<&Value>) -> String {
    let allow_obj = |orig: Option<&Value>| {
        let mut obj = Map::new();
        obj.insert("behavior".into(), Value::String("allow".into()));
        if let Some(orig) = orig {
            obj.insert("updatedInput".into(), orig.clone());
        }
        obj
    };
    let body = match answer {
        PermissionAnswer::AllowOnce | PermissionAnswer::ApprovePlan { .. } => Value::Object(allow_obj(orig_input)),
        PermissionAnswer::AllowSession(suggestions) => {
            let mut obj = allow_obj(orig_input);
            if !suggestions.is_empty() {
                obj.insert("updatedPermissions".into(), Value::Array(suggestions.clone()));
            }
            Value::Object(obj)
        }
        PermissionAnswer::Deny { message, interrupt } => {
            let msg = message.as_deref().map(str::trim).filter(|s| !s.is_empty()).unwrap_or("User denied tool use.");
            json!({ "behavior": "deny", "message": msg, "interrupt": interrupt })
        }
        PermissionAnswer::Questions(answers) => {
            let mut updated = orig_input.and_then(Value::as_object).cloned().unwrap_or_default();
            updated.insert("answers".into(), Value::Object(answers.clone()));
            json!({ "behavior": "allow", "updatedInput": Value::Object(updated) })
        }
    };
    json!({
        "type": "control_response",
        "response": { "subtype": "success", "request_id": request_id, "response": body }
    })
    .to_string()
}

pub fn build_set_permission_mode(request_id: &str, mode: PermissionMode) -> String {
    control_req(request_id, json!({ "subtype": "set_permission_mode", "mode": mode.as_str() }))
}

pub fn build_set_model(request_id: &str, model: &str) -> String {
    control_req(request_id, json!({ "subtype": "set_model", "model": model }))
}

pub fn build_apply_flag_settings(request_id: &str, settings: Value) -> String {
    control_req(request_id, json!({ "subtype": "apply_flag_settings", "settings": settings }))
}

pub fn build_set_effort(request_id: &str, effort: EffortLevel) -> String {
    build_apply_flag_settings(request_id, json!({ "effortLevel": effort.as_str() }))
}

/// Thinking on (the session's default) or off: `set_max_thinking_tokens` with null or 0.
pub fn build_set_thinking(request_id: &str, on: bool) -> String {
    control_req(request_id, super::input::thinking::request(on))
}

pub fn build_set_fast_mode(request_id: &str, enabled: bool) -> String {
    build_apply_flag_settings(request_id, json!({ "fastMode": enabled }))
}

pub fn build_interrupt(request_id: &str) -> String {
    control_req(request_id, json!({ "subtype": "interrupt" }))
}

pub fn build_stop_task(request_id: &str, task_id: &str) -> String {
    control_req(request_id, json!({ "subtype": "stop_task", "task_id": task_id }))
}

pub fn build_rewind_files(request_id: &str, user_message_id: &str) -> String {
    control_req(request_id, json!({ "subtype": "rewind_files", "user_message_id": user_message_id }))
}

pub fn build_mcp_status(request_id: &str) -> String {
    control_req(request_id, json!({ "subtype": "mcp_status" }))
}

pub fn build_mcp_reconnect(request_id: &str, server_name: &str) -> String {
    control_req(request_id, json!({ "subtype": "mcp_reconnect", "serverName": server_name }))
}

pub fn build_mcp_toggle(request_id: &str, server_name: &str, enabled: bool) -> String {
    control_req(request_id, json!({ "subtype": "mcp_toggle", "serverName": server_name, "enabled": enabled }))
}

// ---------------------------------------------------------------------------
// Incoming NDJSON parser
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
enum ActiveBlock {
    Text,
    Thinking,
    ToolUse { id: String, name: String, input_buf: String },
}

/// Stateful per-stream context needed to stitch incremental `stream_event`
/// content blocks and avoid duplicating already-streamed text/thoughts when
/// the final `assistant` envelope arrives.
#[derive(Clone, Debug, Default)]
pub struct StreamParseState {
    active_blocks: HashMap<usize, ActiveBlock>,
    streamed_text: bool,
    streamed_thinking: bool,
    emitted_tool_ids: HashSet<String>,
    pending_mcp_requests: HashSet<String>,
}

impl StreamParseState {
    pub fn register_mcp_status_request(&mut self, request_id: impl Into<String>) {
        self.pending_mcp_requests.insert(request_id.into());
    }

    fn reset_message(&mut self) {
        self.active_blocks.clear();
        self.streamed_text = false;
        self.streamed_thinking = false;
        self.emitted_tool_ids.clear();
    }
}

/// Parses one NDJSON line emitted by `claude --output-format stream-json`.
/// Never panics on malformed, truncated, or unexpected input.
pub fn parse_line(line: &str, state: &mut StreamParseState) -> Vec<ClaudeEvent> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let Ok(val) = serde_json::from_str::<Value>(trimmed) else { return Vec::new() };
    let Some(msg_type) = val.get("type").and_then(Value::as_str) else { return Vec::new() };

    let mut out = Vec::new();
    match msg_type {
        "stream_event" => parse_stream_event(&val, state, &mut out),
        "assistant" => parse_assistant(&val, state, &mut out),
        "user" => parse_user(&val, &mut out),
        "system" => parse_system(&val, &mut out),
        "result" => parse_result(&val, state, &mut out),
        "control_request" => parse_control_request(&val, &mut out),
        "control_response" => parse_control_response(&val, state, &mut out),
        "prompt_suggestion" => {
            if let Some(text) = str_field(&val, &["suggestion", "text", "prompt"]) {
                out.push(ClaudeEvent::PromptSuggestion(text));
            }
        }
        "rate_limit_event" => parse_rate_limit(&val, &mut out),
        "error" => {
            let msg = val
                .get("error")
                .and_then(|e| e.as_str().or_else(|| e.get("message").and_then(Value::as_str)))
                .or_else(|| val.get("message").and_then(Value::as_str))
                .unwrap_or("Unknown stream error");
            out.push(ClaudeEvent::Error(msg.to_string()));
        }
        _ => {}
    }
    out
}

fn non_empty_str<'a>(val: &'a Value, key: &str) -> Option<&'a str> {
    val.get(key).and_then(Value::as_str).filter(|s| !s.is_empty())
}

fn str_field(val: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| val.get(*k).and_then(Value::as_str)).filter(|s| !s.is_empty()).map(str::to_string)
}

fn u64_field(val: &Value, keys: &[&str]) -> Option<u64> {
    keys.iter().find_map(|k| val.get(*k).and_then(Value::as_u64))
}

fn f64_field(val: &Value, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|k| val.get(*k).and_then(Value::as_f64))
}

fn bool_field(val: &Value, keys: &[&str]) -> bool {
    keys.iter().find_map(|k| val.get(*k).and_then(Value::as_bool)).unwrap_or(false)
}

fn str_vec(val: &Value, keys: &[&str]) -> Vec<String> {
    keys.iter()
        .find_map(|k| val.get(*k).and_then(Value::as_array))
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default()
}

fn val_vec(val: &Value, keys: &[&str]) -> Vec<Value> {
    keys.iter().find_map(|k| val.get(*k).and_then(Value::as_array)).cloned().unwrap_or_default()
}

fn mode_field(val: &Value, keys: &[&str]) -> Option<PermissionMode> {
    str_field(val, keys).as_deref().and_then(PermissionMode::from_wire)
}

fn parse_usage(val: &Value) -> TokenUsage {
    TokenUsage {
        input_tokens: u64_field(val, &["input_tokens", "inputTokens"]).unwrap_or(0),
        output_tokens: u64_field(val, &["output_tokens", "outputTokens"]).unwrap_or(0),
        cache_creation_input_tokens: u64_field(val, &["cache_creation_input_tokens", "cacheCreationInputTokens"])
            .unwrap_or(0),
        cache_read_input_tokens: u64_field(val, &["cache_read_input_tokens", "cacheReadInputTokens"]).unwrap_or(0),
    }
}

fn push_usage(val: &Value, model: Option<String>, out: &mut Vec<ClaudeEvent>) {
    if let Some(u) = val.get("usage") {
        let usage = parse_usage(u);
        if !usage.is_empty() {
            out.push(ClaudeEvent::MessageUsage { model, usage });
        }
    }
}

fn parse_stream_event(val: &Value, state: &mut StreamParseState, out: &mut Vec<ClaudeEvent>) {
    let Some(ev) = val.get("event") else { return };
    let Some(ev_type) = ev.get("type").and_then(Value::as_str) else { return };

    if let Some(parent_id) = non_empty_str(val, "parent_tool_use_id") {
        if ev_type == "content_block_delta"
            && let Some(delta) = ev.get("delta")
        {
            let msg = match delta.get("type").and_then(Value::as_str) {
                Some("text_delta") => non_empty_str(delta, "text").map(|t| SubagentMessage::Text(t.into())),
                Some("thinking_delta") => non_empty_str(delta, "thinking").map(|t| SubagentMessage::Thought(t.into())),
                _ => None,
            };
            if let Some(message) = msg {
                out.push(ClaudeEvent::SubagentEvent(SubagentEvent { parent_tool_use_id: parent_id.into(), message }));
            }
        }
        return;
    }

    match ev_type {
        "message_start" => {
            state.reset_message();
            if let Some(msg) = ev.get("message") {
                push_usage(msg, str_field(msg, &["model"]), out);
            }
        }
        "content_block_start" => {
            let idx = u64_field(ev, &["index"]).unwrap_or(0) as usize;
            let Some(cb) = ev.get("content_block") else { return };
            match cb.get("type").and_then(Value::as_str) {
                Some("text") => {
                    state.active_blocks.insert(idx, ActiveBlock::Text);
                    if let Some(t) = non_empty_str(cb, "text") {
                        state.streamed_text = true;
                        out.push(ClaudeEvent::TextDelta(t.into()));
                    }
                }
                Some("thinking") => {
                    state.active_blocks.insert(idx, ActiveBlock::Thinking);
                    if let Some(t) = non_empty_str(cb, "thinking") {
                        state.streamed_thinking = true;
                        out.push(ClaudeEvent::ThoughtDelta(t.into()));
                    }
                }
                Some("tool_use") => {
                    let id = str_field(cb, &["id"]).unwrap_or_default();
                    let name = str_field(cb, &["name"]).unwrap_or_default();
                    let input = cb.get("input").cloned().unwrap_or(Value::Object(Map::new()));
                    state.active_blocks.insert(
                        idx,
                        ActiveBlock::ToolUse { id: id.clone(), name: name.clone(), input_buf: String::new() },
                    );
                    if !id.is_empty() {
                        state.emitted_tool_ids.insert(id.clone());
                    }
                    out.push(ClaudeEvent::ToolCallStart { id, name, input });
                }
                _ => {}
            }
        }
        "content_block_delta" => {
            let idx = u64_field(ev, &["index"]).unwrap_or(0) as usize;
            let Some(delta) = ev.get("delta") else { return };
            match delta.get("type").and_then(Value::as_str) {
                Some("text_delta") => {
                    if let Some(t) = non_empty_str(delta, "text") {
                        state.streamed_text = true;
                        out.push(ClaudeEvent::TextDelta(t.into()));
                    }
                }
                Some("thinking_delta") => {
                    if let Some(t) = non_empty_str(delta, "thinking") {
                        state.streamed_thinking = true;
                        out.push(ClaudeEvent::ThoughtDelta(t.into()));
                    }
                }
                Some("input_json_delta") => {
                    if let Some(partial) = non_empty_str(delta, "partial_json")
                        && let Some(ActiveBlock::ToolUse { id, name, input_buf }) = state.active_blocks.get_mut(&idx)
                    {
                        if input_buf.len() + partial.len() <= MAX_TOOL_INPUT_BUF {
                            input_buf.push_str(partial);
                        }
                        out.push(ClaudeEvent::ToolInputDelta {
                            id: id.clone(),
                            name: name.clone(),
                            partial_json: partial.into(),
                        });
                    }
                }
                _ => {}
            }
        }
        "content_block_stop" => {
            let idx = u64_field(ev, &["index"]).unwrap_or(0) as usize;
            if let Some(ActiveBlock::ToolUse { id, name, input_buf }) = state.active_blocks.remove(&idx)
                && !input_buf.is_empty()
                && let Ok(input) = serde_json::from_str::<Value>(&input_buf)
            {
                out.push(ClaudeEvent::ToolCallUpdate { id, name, input });
            }
        }
        "message_delta" => push_usage(ev, None, out),
        _ => {}
    }
}

fn parse_assistant(val: &Value, state: &mut StreamParseState, out: &mut Vec<ClaudeEvent>) {
    let Some(msg) = val.get("message") else { return };
    let blocks = msg.get("content").and_then(Value::as_array);

    if let Some(parent_id) = non_empty_str(val, "parent_tool_use_id") {
        for b in blocks.into_iter().flatten() {
            let sub = match b.get("type").and_then(Value::as_str) {
                Some("text") => non_empty_str(b, "text").map(|t| SubagentMessage::Text(t.into())),
                Some("thinking") => non_empty_str(b, "thinking").map(|t| SubagentMessage::Thought(t.into())),
                Some("tool_use") => Some(SubagentMessage::ToolUse {
                    id: str_field(b, &["id"]).unwrap_or_default(),
                    name: str_field(b, &["name"]).unwrap_or_default(),
                    input: b.get("input").cloned().unwrap_or(Value::Object(Map::new())),
                }),
                _ => None,
            };
            if let Some(message) = sub {
                out.push(ClaudeEvent::SubagentEvent(SubagentEvent { parent_tool_use_id: parent_id.into(), message }));
            }
        }
        return;
    }

    push_usage(msg, str_field(msg, &["model"]), out);
    for b in blocks.into_iter().flatten() {
        match b.get("type").and_then(Value::as_str) {
            Some("text") if !state.streamed_text => {
                if let Some(t) = non_empty_str(b, "text") {
                    out.push(ClaudeEvent::TextDelta(t.into()));
                }
            }
            Some("thinking") if !state.streamed_thinking => {
                if let Some(t) = non_empty_str(b, "thinking") {
                    out.push(ClaudeEvent::ThoughtDelta(t.into()));
                }
            }
            Some("tool_use") => {
                let id = str_field(b, &["id"]).unwrap_or_default();
                let name = str_field(b, &["name"]).unwrap_or_default();
                let input = b.get("input").cloned().unwrap_or(Value::Object(Map::new()));
                if !id.is_empty() && state.emitted_tool_ids.contains(&id) {
                    out.push(ClaudeEvent::ToolCallUpdate { id, name, input });
                } else {
                    if !id.is_empty() {
                        state.emitted_tool_ids.insert(id.clone());
                    }
                    out.push(ClaudeEvent::ToolCallStart { id, name, input });
                }
            }
            _ => {}
        }
    }
    state.reset_message();
}

fn extract_tool_result_content(content_val: Option<&Value>) -> (String, Vec<ToolResultImage>) {
    let Some(val) = content_val else { return (String::new(), Vec::new()) };
    if let Some(s) = val.as_str() {
        return (s.to_string(), Vec::new());
    }
    let Some(arr) = val.as_array() else { return (String::new(), Vec::new()) };
    let mut texts = Vec::new();
    let mut images = Vec::new();
    for item in arr {
        match item.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(t) = item.get("text").and_then(Value::as_str) {
                    texts.push(t);
                }
            }
            Some("image") => {
                if let Some(src) = item.get("source")
                    && src.get("type").and_then(Value::as_str) == Some("base64")
                    && let (Some(media_type), Some(data)) =
                        (src.get("media_type").and_then(Value::as_str), src.get("data").and_then(Value::as_str))
                {
                    images.push(ToolResultImage { media_type: media_type.into(), base64: data.into() });
                }
            }
            _ => {}
        }
    }
    (texts.join("\n"), images)
}

fn parse_user(val: &Value, out: &mut Vec<ClaudeEvent>) {
    let parent = non_empty_str(val, "parent_tool_use_id");
    let uuid = str_field(val, &["uuid"]);
    let Some(content) = val.get("message").and_then(|m| m.get("content")) else { return };

    if let Some(text) = content.as_str() {
        if parent.is_none()
            && let Some(uuid) = uuid
        {
            out.push(ClaudeEvent::UserReplayed { uuid, text: text.into() });
        }
        return;
    }

    let Some(blocks) = content.as_array() else { return };
    let mut had_tool_result = false;
    let mut replayed_texts = Vec::new();
    for b in blocks {
        match b.get("type").and_then(Value::as_str) {
            Some("tool_result") => {
                had_tool_result = true;
                let tool_use_id = str_field(b, &["tool_use_id", "toolUseId"]).unwrap_or_default();
                let is_error = bool_field(b, &["is_error"]);
                let (text, images) = extract_tool_result_content(b.get("content"));
                if let Some(parent_id) = parent {
                    out.push(ClaudeEvent::SubagentEvent(SubagentEvent {
                        parent_tool_use_id: parent_id.into(),
                        message: SubagentMessage::ToolResult { tool_use_id, content: text, is_error, images },
                    }));
                } else {
                    out.push(ClaudeEvent::ToolResult { tool_use_id, content: text, is_error, images });
                }
            }
            Some("text") => {
                if let Some(t) = b.get("text").and_then(Value::as_str) {
                    replayed_texts.push(t);
                }
            }
            _ => {}
        }
    }
    if !had_tool_result
        && parent.is_none()
        && let Some(uuid) = uuid
    {
        out.push(ClaudeEvent::UserReplayed { uuid, text: replayed_texts.join("\n") });
    }
}

fn parse_system(val: &Value, out: &mut Vec<ClaudeEvent>) {
    let Some(subtype) = val.get("subtype").and_then(Value::as_str) else { return };
    match subtype {
        "init" => out.push(ClaudeEvent::SystemInit(SystemInitInfo {
            session_id: str_field(val, &["session_id"]).unwrap_or_default(),
            model: str_field(val, &["model"]),
            permission_mode: mode_field(val, &["permissionMode", "permission_mode"]),
            tools: str_vec(val, &["tools"]),
            slash_commands: str_vec(val, &["slash_commands"]),
            cwd: str_field(val, &["cwd"]),
            claude_code_version: str_field(val, &["claude_code_version"]),
            ultrathink_available: val.get("ultrathink_available").and_then(Value::as_bool).unwrap_or(false),
        })),
        "status" => out.push(ClaudeEvent::StatusUpdate {
            status: str_field(val, &["status"]),
            permission_mode: mode_field(val, &["permissionMode", "permission_mode"]),
        }),
        "compact_boundary" => {
            let meta = val.get("compact_metadata").unwrap_or(val);
            out.push(ClaudeEvent::CompactBoundary {
                trigger: str_field(meta, &["trigger"]).or_else(|| str_field(val, &["trigger"])),
                pre_tokens: u64_field(meta, &["pre_tokens", "preTokens"])
                    .or_else(|| u64_field(val, &["pre_tokens", "preTokens"])),
            });
        }
        "task_started" => out.push(ClaudeEvent::TaskStarted {
            task_id: str_field(val, &["task_id", "taskId"]).unwrap_or_default(),
            tool_use_id: str_field(val, &["tool_use_id", "toolUseId"]),
            description: str_field(val, &["description"]).unwrap_or_default(),
            task_type: str_field(val, &["task_type", "taskType"]),
        }),
        "task_progress" => {
            let u = val.get("usage");
            out.push(ClaudeEvent::TaskProgress(TaskProgress {
                task_id: str_field(val, &["task_id", "taskId"]).unwrap_or_default(),
                tool_use_id: str_field(val, &["tool_use_id", "toolUseId"]),
                description: str_field(val, &["description"]),
                last_tool_name: str_field(val, &["last_tool_name", "lastToolName"]),
                summary: str_field(val, &["summary"]),
                total_tokens: u.and_then(|v| u64_field(v, &["total_tokens", "totalTokens"])),
                tool_uses: u.and_then(|v| u64_field(v, &["tool_uses", "toolUses"])).map(|n| n as u32),
                duration_ms: u.and_then(|v| u64_field(v, &["duration_ms", "durationMs"])),
            }));
        }
        "task_notification" => out.push(ClaudeEvent::TaskNotification {
            task_id: str_field(val, &["task_id", "taskId"]).unwrap_or_default(),
            tool_use_id: str_field(val, &["tool_use_id", "toolUseId"]),
            status: str_field(val, &["status"]).unwrap_or_default(),
            summary: str_field(val, &["summary"]),
        }),
        _ => {}
    }
}

fn parse_result(val: &Value, state: &mut StreamParseState, out: &mut Vec<ClaudeEvent>) {
    state.reset_message();
    let mut model_usage = Vec::new();
    let mut context_window = None;
    if let Some(mu) = val.get("modelUsage").or_else(|| val.get("model_usage")).and_then(Value::as_object) {
        for (model, e) in mu {
            let cw = u64_field(e, &["contextWindow", "context_window"]).unwrap_or(0);
            if cw > 0 && context_window.is_none_or(|prev| cw > prev) {
                context_window = Some(cw);
            }
            let u = parse_usage(e);
            model_usage.push(ModelUsage {
                model: model.clone(),
                input_tokens: u.input_tokens,
                output_tokens: u.output_tokens,
                cache_creation_input_tokens: u.cache_creation_input_tokens,
                cache_read_input_tokens: u.cache_read_input_tokens,
                cost_usd: f64_field(e, &["costUSD", "cost_usd"]).unwrap_or(0.0),
                context_window: cw,
                max_output_tokens: u64_field(e, &["maxOutputTokens", "max_output_tokens"]).unwrap_or(0),
            });
        }
    }
    let errors = val
        .get("errors")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().or_else(|| v.get("message").and_then(Value::as_str)).map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    out.push(ClaudeEvent::TurnFinished(TurnResult {
        session_id: str_field(val, &["session_id"]),
        subtype: str_field(val, &["subtype"]).unwrap_or_else(|| "success".into()),
        total_cost_usd: f64_field(val, &["total_cost_usd"]).unwrap_or(0.0),
        duration_ms: u64_field(val, &["duration_ms"]).unwrap_or(0),
        duration_api_ms: u64_field(val, &["duration_api_ms"]).unwrap_or(0),
        num_turns: u64_field(val, &["num_turns"]).unwrap_or(0) as u32,
        usage: val.get("usage").map(parse_usage).unwrap_or_default(),
        model_usage,
        context_window,
        is_error: bool_field(val, &["is_error"]),
        result_text: str_field(val, &["result"]),
        errors,
    }));
}

fn parse_control_request(val: &Value, out: &mut Vec<ClaudeEvent>) {
    let request_id = str_field(val, &["request_id"]).unwrap_or_default();
    let Some(req) = val.get("request") else { return };
    if req.get("subtype").and_then(Value::as_str) != Some("can_use_tool") {
        return;
    }
    let tool_name = str_field(req, &["tool_name", "toolName"]).unwrap_or_default();
    let input = req.get("input").cloned().unwrap_or(Value::Object(Map::new()));
    let tool_use_id = str_field(req, &["tool_use_id", "toolUseID", "toolUseId"]);

    let kind = match tool_name.as_str() {
        "AskUserQuestion" => {
            PermissionKind::AskUserQuestion { questions: parse_ask_questions(&input), raw_input: input }
        }
        "ExitPlanMode" => PermissionKind::ExitPlanMode {
            plan_text: str_field(&input, &["plan", "planText", "plan_text"]),
            plan_path: str_field(&input, &["planFilePath", "plan_file_path", "plan_path", "file_path"]),
            allowed_prompts: val_vec(&input, &["allowedPrompts", "allowed_prompts"]),
            raw_input: input,
        },
        _ => PermissionKind::ToolApproval {
            tool_name,
            input,
            permission_suggestions: val_vec(req, &["permission_suggestions", "permissionSuggestions"]),
            decision_reason: str_field(req, &["decision_reason", "decisionReason"]),
            decision_reason_type: str_field(req, &["decision_reason_type", "decisionReasonType"]),
            decision_reason_rule_source: str_field(req, &["decision_reason_rule_source", "decisionReasonRuleSource"]),
            blocked_path: str_field(req, &["blocked_path", "blockedPath"]),
            description: str_field(req, &["description"]),
        },
    };
    out.push(ClaudeEvent::PermissionRequested(PermissionRequest { request_id, tool_use_id, kind }));
}

fn parse_ask_questions(input: &Value) -> Vec<AskQuestion> {
    let Some(arr) = input.get("questions").and_then(Value::as_array) else { return Vec::new() };
    arr.iter()
        .map(|q| AskQuestion {
            question: str_field(q, &["question"]).unwrap_or_default(),
            header: str_field(q, &["header"]).unwrap_or_default(),
            options: q
                .get("options")
                .and_then(Value::as_array)
                .map(|opts| {
                    opts.iter()
                        .map(|o| AskQuestionOption {
                            label: str_field(o, &["label"]).unwrap_or_default(),
                            description: str_field(o, &["description"]).unwrap_or_default(),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            multi_select: bool_field(q, &["multiSelect", "multi_select"]),
        })
        .collect()
}

fn parse_control_response(val: &Value, state: &mut StreamParseState, out: &mut Vec<ClaudeEvent>) {
    let Some(resp) = val.get("response") else { return };
    let request_id = str_field(resp, &["request_id"]).unwrap_or_default();
    if resp.get("subtype").and_then(Value::as_str) == Some("error") {
        let error = resp
            .get("error")
            .and_then(|e| e.as_str().or_else(|| e.get("message").and_then(Value::as_str)))
            .unwrap_or("Control request failed");
        state.pending_mcp_requests.remove(&request_id);
        out.push(ClaudeEvent::ControlError { request_id, error: error.into() });
        return;
    }

    let payload = resp.get("response").cloned().unwrap_or(Value::Null);
    if request_id == "init-1"
        || payload.get("models").is_some()
        || payload.get("commands").is_some()
        || payload.get("claude_code_version").is_some()
    {
        out.push(ClaudeEvent::Initialized(parse_init_info(&payload)));
    } else if state.pending_mcp_requests.remove(&request_id)
        || payload.get("mcp_servers").is_some()
        || payload.get("mcpServers").is_some()
        || payload.get("servers").is_some()
    {
        out.push(ClaudeEvent::McpStatus(parse_mcp_servers(&payload)));
    } else {
        out.push(ClaudeEvent::ControlAck { request_id, response: payload });
    }
}

fn parse_init_info(p: &Value) -> InitInfo {
    let models = val_vec(p, &["models"])
        .iter()
        .map(|m| {
            let value = str_field(m, &["value"]).unwrap_or_default();
            let display_name = str_field(m, &["displayName", "display_name"]).unwrap_or_else(|| value.clone());
            let supported_effort_levels =
                str_vec(m, &["supportedEffortLevels"]).iter().filter_map(|s| EffortLevel::from_wire(s)).collect();
            ModelInfo {
                value,
                resolved_model: str_field(m, &["resolvedModel", "resolved_model"]),
                display_name,
                description: str_field(m, &["description"]).unwrap_or_default(),
                supports_effort: bool_field(m, &["supportsEffort"]),
                supported_effort_levels,
                supports_adaptive_thinking: bool_field(m, &["supportsAdaptiveThinking"]),
                supports_fast_mode: bool_field(m, &["supportsFastMode"]),
                supports_auto_mode: bool_field(m, &["supportsAutoMode"]),
                supports_thinking_off: bool_field(m, &["supportsThinkingOff"]),
            }
        })
        .collect();

    let commands = val_vec(p, &["commands"])
        .iter()
        .filter_map(|c| {
            Some(SlashCommand {
                name: str_field(c, &["name"])?,
                description: str_field(c, &["description"]).unwrap_or_default(),
                argument_hint: str_field(c, &["argumentHint", "argument_hint"]),
                aliases: str_vec(c, &["aliases"]),
                builtin: bool_field(c, &["builtin"]),
            })
        })
        .collect();

    let agents = val_vec(p, &["agents"])
        .iter()
        .filter_map(|a| {
            Some(AgentDef {
                name: str_field(a, &["name"])?,
                description: str_field(a, &["description"]).unwrap_or_default(),
                model: str_field(a, &["model"]),
            })
        })
        .collect();

    let account = p.get("account").filter(|v| v.is_object()).map(|a| AccountInfo {
        email: str_field(a, &["email"]),
        organization: str_field(a, &["organization"]),
        subscription_type: str_field(a, &["subscriptionType", "subscription_type"]),
        api_provider: str_field(a, &["apiProvider", "api_provider"]),
    });

    let fast_mode_state = p.get("fast_mode_state").or_else(|| p.get("fastModeState")).and_then(|v| {
        v.as_str().map(str::to_string).or_else(|| v.as_bool().map(|b| if b { "on".into() } else { "off".into() }))
    });

    InitInfo {
        models,
        commands,
        agents,
        account,
        current_permission_mode: mode_field(p, &["current_permission_mode", "currentPermissionMode"]),
        output_style: str_field(p, &["output_style", "outputStyle"]),
        available_output_styles: str_vec(p, &["available_output_styles", "availableOutputStyles"]),
        fast_mode_state,
        claude_code_version: str_field(p, &["claude_code_version", "claudeCodeVersion"]),
    }
}

fn parse_mcp_servers(p: &Value) -> Vec<McpServerInfo> {
    let arr = p
        .get("mcp_servers")
        .or_else(|| p.get("mcpServers"))
        .or_else(|| p.get("servers"))
        .and_then(Value::as_array)
        .or_else(|| p.as_array());
    arr.into_iter()
        .flatten()
        .filter_map(|s| {
            Some(McpServerInfo {
                name: str_field(s, &["name"])?,
                status: str_field(s, &["status"]).unwrap_or_else(|| "unknown".into()),
                error: str_field(s, &["error"]),
            })
        })
        .collect()
}

fn parse_rate_limit(val: &Value, out: &mut Vec<ClaudeEvent>) {
    let info = val.get("rate_limit_info").or_else(|| val.get("rateLimitInfo")).unwrap_or(val);
    out.push(ClaudeEvent::RateLimit(RateLimitInfo {
        status: str_field(info, &["status"]).unwrap_or_else(|| "limited".into()),
        resets_at: str_field(info, &["resetsAt", "resets_at"])
            .or_else(|| u64_field(info, &["resetsAt", "resets_at"]).map(|n| n.to_string())),
        rate_limit_type: str_field(info, &["rateLimitType", "rate_limit_type"]),
        message: str_field(info, &["message"]).or_else(|| str_field(val, &["message"])),
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_initialize_and_control_frames() {
        let parse = |s: String| serde_json::from_str::<Value>(&s).unwrap();
        assert_eq!(parse(build_initialize("init-1"))["request"]["subtype"], "initialize");
        assert_eq!(
            parse(build_set_permission_mode("r-2", PermissionMode::AcceptEdits))["request"]["mode"],
            "acceptEdits"
        );
        assert_eq!(parse(build_set_model("r-3", "opus"))["request"]["model"], "opus");
        assert_eq!(parse(build_set_effort("r-4", EffortLevel::XHigh))["request"]["settings"]["effortLevel"], "xhigh");
        assert_eq!(parse(build_set_fast_mode("r-5", true))["request"]["settings"]["fastMode"], true);
        assert_eq!(parse(build_interrupt("r-6"))["request"]["subtype"], "interrupt");
        assert_eq!(parse(build_stop_task("r-7", "task-9"))["request"]["task_id"], "task-9");
        assert_eq!(parse(build_rewind_files("r-8", "u-123"))["request"]["user_message_id"], "u-123");
        assert_eq!(parse(build_mcp_status("r-9"))["request"]["subtype"], "mcp_status");
    }

    #[test]
    fn builds_user_message_with_text_image_and_file_context() {
        let msg = build_user_message(&[
            PromptBlock::Text("Look at this".into()),
            PromptBlock::Image { media_type: "image/png".into(), base64: "iVBORw==".into() },
            PromptBlock::FileContext { path: "src/main.rs".into(), content: "fn main() {}".into() },
        ]);
        let v: Value = serde_json::from_str(&msg).unwrap();
        let content = v["message"]["content"].as_array().unwrap();
        assert_eq!(content.len(), 3);
        assert_eq!(content[0]["text"], "Look at this");
        assert_eq!(content[1]["source"]["media_type"], "image/png");
        assert!(content[2]["text"].as_str().unwrap().contains("<file path=\"src/main.rs\">"));
    }

    #[test]
    fn builds_permission_responses() {
        let orig = json!({ "command": "cargo test" });
        let allow: Value =
            serde_json::from_str(&build_permission_response("p-1", &PermissionAnswer::AllowOnce, Some(&orig))).unwrap();
        assert_eq!(allow["response"]["response"]["behavior"], "allow");
        assert_eq!(allow["response"]["response"]["updatedInput"], orig);

        let sugg = vec![json!({ "type": "addRules", "behavior": "allow" })];
        let allow_sess: Value = serde_json::from_str(&build_permission_response(
            "p-2",
            &PermissionAnswer::AllowSession(sugg.clone()),
            Some(&orig),
        ))
        .unwrap();
        assert_eq!(allow_sess["response"]["response"]["updatedPermissions"], Value::Array(sugg));

        let deny: Value = serde_json::from_str(&build_permission_response(
            "p-3",
            &PermissionAnswer::Deny { message: Some("no thanks".into()), interrupt: true },
            None,
        ))
        .unwrap();
        assert_eq!(deny["response"]["response"]["behavior"], "deny");
        assert_eq!(deny["response"]["response"]["interrupt"], true);

        let q_orig = json!({ "questions": [{ "question": "Pick one?" }] });
        let mut answers = Map::new();
        answers.insert("Pick one?".into(), Value::String("Option A".into()));
        let q_resp: Value = serde_json::from_str(&build_permission_response(
            "p-4",
            &PermissionAnswer::Questions(answers),
            Some(&q_orig),
        ))
        .unwrap();
        assert_eq!(q_resp["response"]["response"]["updatedInput"]["questions"], q_orig["questions"]);
        assert_eq!(q_resp["response"]["response"]["updatedInput"]["answers"]["Pick one?"], "Option A");
    }

    #[test]
    fn parses_live_initialize_control_response() {
        let line = r#"{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"commands":[{"name":"compact","description":"Compact context","argumentHint":"[instructions]","aliases":["c"],"builtin":true}],"agents":[{"name":"Explore","description":"Fast codebase exploration","model":"haiku"}],"models":[{"value":"opus","resolvedModel":"claude-opus-5-5","displayName":"Opus 5.5","description":"For complex work","supportsEffort":true,"supportedEffortLevels":["low","medium","high","xhigh","max"],"supportsAdaptiveThinking":true,"supportsFastMode":true,"supportsAutoMode":true,"supportsThinkingOff":false}],"account":{"email":"a@b.com","organization":"Acme","subscriptionType":"Claude Team","apiProvider":"firstParty"},"output_style":"Concise","available_output_styles":["default","Concise"],"current_permission_mode":"default","fast_mode_state":"off","claude_code_version":"2.1.330"}}}"#;
        let mut state = StreamParseState::default();
        let evs = parse_line(line, &mut state);
        let ClaudeEvent::Initialized(info) = &evs[0] else { panic!("expected Initialized") };
        assert_eq!(info.claude_code_version.as_deref(), Some("2.1.330"));
        assert_eq!(info.current_permission_mode, Some(PermissionMode::Default));
        assert_eq!(info.models[0].supported_effort_levels, EffortLevel::ALL);
        assert_eq!(info.commands[0].aliases, ["c"]);
        assert_eq!(info.account.as_ref().unwrap().email.as_deref(), Some("a@b.com"));
    }

    #[test]
    fn parses_streaming_deltas_and_deduplicates_assistant_snapshot() {
        let mut state = StreamParseState::default();
        let lines = [
            r#"{"type":"stream_event","event":{"type":"message_start","message":{"model":"claude-opus-5-5","usage":{"input_tokens":100,"cache_read_input_tokens":50}}}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"hmm"}}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"Hello"}}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"tu_1","name":"Bash","input":{}}}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{\"command\":\"ls\"}"}}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_stop","index":2}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"hmm"},{"type":"text","text":"Hello"},{"type":"tool_use","id":"tu_1","name":"Bash","input":{"command":"ls"}}]}}"#,
        ];
        let all: Vec<ClaudeEvent> = lines.iter().flat_map(|l| parse_line(l, &mut state)).collect();
        assert_eq!(all.iter().filter(|e| matches!(e, ClaudeEvent::TextDelta(_))).count(), 1);
        assert_eq!(all.iter().filter(|e| matches!(e, ClaudeEvent::ThoughtDelta(_))).count(), 1);
        assert_eq!(all.iter().filter(|e| matches!(e, ClaudeEvent::ToolCallStart { .. })).count(), 1);
        assert!(all.iter().any(
            |e| matches!(e, ClaudeEvent::ToolCallUpdate { id, input, .. } if id == "tu_1" && input["command"] == "ls")
        ));
    }

    #[test]
    fn parses_subagent_and_tool_result_with_inline_image() {
        let mut state = StreamParseState::default();
        let sub = r#"{"type":"stream_event","parent_tool_use_id":"tu_parent","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"scanning..."}}}"#;
        assert_eq!(
            parse_line(sub, &mut state),
            vec![ClaudeEvent::SubagentEvent(SubagentEvent {
                parent_tool_use_id: "tu_parent".into(),
                message: SubagentMessage::Text("scanning...".into()),
            })]
        );

        let user_tr = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tu_1","is_error":false,"content":[{"type":"text","text":"ok"},{"type":"image","source":{"type":"base64","media_type":"image/png","data":"AAAA"}}]}]}}"#;
        assert_eq!(
            parse_line(user_tr, &mut state),
            vec![ClaudeEvent::ToolResult {
                tool_use_id: "tu_1".into(),
                content: "ok".into(),
                is_error: false,
                images: vec![ToolResultImage { media_type: "image/png".into(), base64: "AAAA".into() }],
            }]
        );

        let replayed =
            r#"{"type":"user","uuid":"u-42","message":{"role":"user","content":[{"type":"text","text":"hi"}]}}"#;
        assert_eq!(
            parse_line(replayed, &mut state),
            vec![ClaudeEvent::UserReplayed { uuid: "u-42".into(), text: "hi".into() }]
        );
    }

    #[test]
    fn classifies_permission_requests() {
        let mut state = StreamParseState::default();
        let bash = r#"{"type":"control_request","request_id":"req-1","request":{"subtype":"can_use_tool","tool_name":"Bash","tool_use_id":"tu-1","input":{"command":"rm -rf tmp"},"permission_suggestions":[{"type":"addRules"}],"decision_reason":"Needs approval"}}"#;
        assert!(matches!(
            parse_line(bash, &mut state).as_slice(),
            [ClaudeEvent::PermissionRequested(PermissionRequest {
                request_id,
                kind: PermissionKind::ToolApproval { tool_name, permission_suggestions, decision_reason, .. },
                ..
            })] if request_id == "req-1" && tool_name == "Bash" && permission_suggestions.len() == 1 && decision_reason.as_deref() == Some("Needs approval")
        ));

        let ask = r#"{"type":"control_request","request_id":"req-2","request":{"subtype":"can_use_tool","tool_name":"AskUserQuestion","input":{"questions":[{"question":"Which DB?","header":"DB","multiSelect":false,"options":[{"label":"Postgres","description":"Relational"}]}]}}}"#;
        assert!(matches!(
            parse_line(ask, &mut state).as_slice(),
            [ClaudeEvent::PermissionRequested(PermissionRequest {
                kind: PermissionKind::AskUserQuestion { questions, .. },
                ..
            })] if questions.len() == 1 && questions[0].options[0].label == "Postgres"
        ));

        let plan = r###"{"type":"control_request","request_id":"req-3","request":{"subtype":"can_use_tool","tool_name":"ExitPlanMode","input":{"plan":"## Plan\n1. Do it","planFilePath":"/tmp/plan.md"}}}"###;
        assert!(matches!(
            parse_line(plan, &mut state).as_slice(),
            [ClaudeEvent::PermissionRequested(PermissionRequest {
                kind: PermissionKind::ExitPlanMode { plan_text, plan_path, .. },
                ..
            })] if plan_text.as_deref() == Some("## Plan\n1. Do it") && plan_path.as_deref() == Some("/tmp/plan.md")
        ));
    }

    #[test]
    fn parses_system_result_suggestion_and_rate_limit() {
        let mut state = StreamParseState::default();
        let sys_init = r#"{"type":"system","subtype":"init","session_id":"s-1","model":"claude-opus-5-5","permissionMode":"default","tools":["Bash","Edit"]}"#;
        assert!(matches!(
            parse_line(sys_init, &mut state).as_slice(),
            [ClaudeEvent::SystemInit(SystemInitInfo { session_id, .. })] if session_id == "s-1"
        ));

        let compact = r#"{"type":"system","subtype":"compact_boundary","compact_metadata":{"trigger":"auto","pre_tokens":180000}}"#;
        assert_eq!(
            parse_line(compact, &mut state),
            vec![ClaudeEvent::CompactBoundary { trigger: Some("auto".into()), pre_tokens: Some(180000) }]
        );

        let result = r#"{"type":"result","subtype":"success","session_id":"s-1","total_cost_usd":0.12,"duration_ms":3200,"num_turns":2,"is_error":false,"usage":{"input_tokens":500,"output_tokens":200,"cache_read_input_tokens":1000},"modelUsage":{"claude-opus-5-5":{"inputTokens":500,"outputTokens":200,"contextWindow":200000,"costUSD":0.12}}}"#;
        let ClaudeEvent::TurnFinished(tr) = &parse_line(result, &mut state)[0] else { panic!("expected TurnFinished") };
        assert_eq!(tr.context_window, Some(200_000));
        assert!((tr.total_cost_usd - 0.12).abs() < 1e-9);

        let sugg = r#"{"type":"prompt_suggestion","suggestion":"Run the test suite"}"#;
        assert_eq!(parse_line(sugg, &mut state), vec![ClaudeEvent::PromptSuggestion("Run the test suite".into())]);

        let rl =
            r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed_warning","rateLimitType":"five_hour"}}"#;
        assert!(matches!(parse_line(rl, &mut state).as_slice(), [ClaudeEvent::RateLimit(_)]));
    }

    #[test]
    fn malformed_lines_never_panic() {
        let mut state = StreamParseState::default();
        for garbage in [
            "",
            "   ",
            "{",
            "not json",
            r#"{"type":123}"#,
            r#"{"type":"stream_event","event":null}"#,
            r#"{"type":"control_response","response":"bad"}"#,
            r#"{"type":"result","modelUsage":42}"#,
        ] {
            let _ = parse_line(garbage, &mut state);
        }
    }
}
