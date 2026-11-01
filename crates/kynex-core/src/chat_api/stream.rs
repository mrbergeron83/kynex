//! OpenRouter's chat completions API: a conversation as its request, and the
//! server-sent events of its streamed reply.

use serde_json::{Value, json};

/// Where chat completions go.
pub const URL: &str = "https://openrouter.ai/api/v1/chat/completions";

/// What curl writes after the body (`-w`), so the status can be told from it.
pub const STATUS_MARK: &str = "kynex-http-status:";

/// The request for `messages` (each `{ role, content }`) on `model`, streamed,
/// with usage (and its cost) in the last event.
pub fn body(model: &str, max_tokens: u32, messages: &[Value]) -> Value {
    json!({
        "model": model,
        "messages": messages,
        "stream": true,
        "max_tokens": max_tokens,
        "usage": { "include": true },
    })
}

/// A user message's content as the API takes it, from the content Kynex sends
/// and logs (a string, or Claude's blocks): text, and images as data URLs.
/// Tool results and the like are left out. `None` if nothing is left.
pub fn user_content(content: &Value) -> Option<Value> {
    let blocks = match content {
        Value::String(t) => return (!t.trim().is_empty()).then(|| Value::String(t.clone())),
        Value::Array(blocks) => blocks,
        _ => return None,
    };
    let mut parts = Vec::new();
    for b in blocks {
        match b["type"].as_str() {
            Some("text") => {
                if let Some(t) = b["text"].as_str().filter(|t| !t.is_empty()) {
                    parts.push(json!({ "type": "text", "text": t }));
                }
            }
            Some("image") if b["source"]["type"] == "base64" => {
                let (Some(media), Some(data)) = (b["source"]["media_type"].as_str(), b["source"]["data"].as_str())
                else {
                    continue;
                };
                parts.push(
                    json!({ "type": "image_url", "image_url": { "url": format!("data:{media};base64,{data}") } }),
                );
            }
            _ => {}
        }
    }
    match parts.as_slice() {
        [] => None,
        // Text alone goes as a plain string, which every model takes.
        [only] if only["type"] == "text" => Some(only["text"].clone()),
        _ => Some(Value::Array(parts)),
    }
}

/// The text of an assistant message's content: a string, or its text blocks.
pub fn assistant_text(content: &Value) -> String {
    match content {
        Value::String(t) => t.clone(),
        Value::Array(blocks) => blocks.iter().filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join(""),
        _ => String::new(),
    }
}

/// What a turn used, from the last event.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    /// What OpenRouter charged, in US dollars.
    pub cost: f64,
}

/// One streamed event's news.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Delta {
    pub id: Option<String>,
    /// The model that answered (a router may pick one).
    pub model: Option<String>,
    pub text: String,
    /// Thinking, from models that show it.
    pub reasoning: String,
    pub finish: Option<String>,
    pub usage: Option<Usage>,
    /// A failure mid-stream, after the 200 was sent.
    pub error: Option<String>,
}

/// One line of the reply.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Data(Delta),
    /// `data: [DONE]`: nothing more comes.
    Done,
    /// The HTTP status curl writes after the body.
    Status(u16),
    /// A comment (`: OPENROUTER PROCESSING`) or a blank line.
    Nothing,
    /// Anything else: an error body that isn't a stream.
    Other(String),
}

/// Reads one line of the reply.
pub fn event(line: &str) -> Event {
    let line = line.trim_end_matches(['\r', '\n']);
    if let Some(code) = line.strip_prefix(STATUS_MARK) {
        return code.trim().parse().map_or(Event::Other(line.to_string()), Event::Status);
    }
    if line.trim().is_empty() || line.starts_with(':') {
        return Event::Nothing;
    }
    let Some(data) = line.strip_prefix("data:") else { return Event::Other(line.to_string()) };
    let data = data.trim();
    if data == "[DONE]" {
        return Event::Done;
    }
    match serde_json::from_str::<Value>(data) {
        Ok(v) => Event::Data(delta(&v)),
        Err(_) => Event::Nothing,
    }
}

fn delta(v: &Value) -> Delta {
    let text = |v: &Value| v.as_str().unwrap_or("").to_string();
    let choice = &v["choices"][0];
    let d = &choice["delta"];
    let usage = v["usage"].is_object().then(|| {
        let n = |k: &str| v["usage"][k].as_u64().unwrap_or(0);
        Usage {
            input: n("prompt_tokens"),
            output: n("completion_tokens"),
            cost: v["usage"]["cost"].as_f64().unwrap_or(0.0),
        }
    });
    let error = v["error"].is_object().then(|| error_message(&v["error"]));
    Delta {
        id: v["id"].as_str().map(str::to_string),
        model: v["model"].as_str().filter(|m| !m.is_empty()).map(str::to_string),
        text: text(&d["content"]),
        reasoning: if d["reasoning"].is_string() { text(&d["reasoning"]) } else { text(&d["reasoning_content"]) },
        finish: choice["finish_reason"].as_str().map(str::to_string),
        usage,
        error,
    }
}

/// An API error object in words: its message, and the upstream provider's
/// own when OpenRouter passes it on.
fn error_message(e: &Value) -> String {
    let message = e["message"].as_str().unwrap_or("unknown error").trim().to_string();
    let raw = e["metadata"]["raw"].as_str().map(str::trim).filter(|r| !r.is_empty() && !message.contains(r));
    match raw {
        Some(raw) => format!("{message} ({})", truncate(raw, 300)),
        None => message,
    }
}

/// A failed request in words, as the CLI words them: `API Error: 404 …`.
pub fn failure(status: Option<u16>, body: &str) -> String {
    let said = serde_json::from_str::<Value>(body.trim())
        .ok()
        .filter(|v| v["error"].is_object())
        .map(|v| error_message(&v["error"]))
        .unwrap_or_else(|| truncate(body.trim(), 300));
    match (status, said.is_empty()) {
        (Some(s), false) => format!("API Error: {s} {said}"),
        (Some(s), true) => format!("API Error: {s}"),
        (None, false) => format!("API Error: {said}"),
        (None, true) => "API Error: no answer from OpenRouter".into(),
    }
}

/// At most `max` characters of `s`, with an ellipsis if cut.
fn truncate(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_and_images_become_the_apis_content() {
        assert_eq!(user_content(&json!("hi")), Some(json!("hi")));
        assert_eq!(user_content(&json!("  ")), None);
        assert_eq!(user_content(&json!([{ "type": "text", "text": "hi" }])), Some(json!("hi")));
        let with_image = json!([
            { "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": "AAA" } },
            { "type": "text", "text": "what's this" },
        ]);
        assert_eq!(
            user_content(&with_image),
            Some(json!([
                { "type": "image_url", "image_url": { "url": "data:image/png;base64,AAA" } },
                { "type": "text", "text": "what's this" },
            ]))
        );
        // A tool result alone (from a CLI session's log) is nothing to send.
        assert_eq!(user_content(&json!([{ "type": "tool_result", "content": "x" }])), None);
        assert_eq!(
            assistant_text(&json!([{ "type": "thinking", "thinking": "hm" }, { "type": "text", "text": "yes" }])),
            "yes"
        );
    }

    #[test]
    fn the_stream_reads_line_by_line() {
        let first = r#"data: {"id":"gen-1","model":"m/x","choices":[{"index":0,"delta":{"role":"assistant","content":"Hel"},"finish_reason":null}]}"#;
        let Event::Data(d) = event(first) else { panic!() };
        assert_eq!((d.id.as_deref(), d.model.as_deref(), d.text.as_str()), (Some("gen-1"), Some("m/x"), "Hel"));
        let thinking = r#"data: {"choices":[{"delta":{"content":null,"reasoning":"hmm"}}]}"#;
        let Event::Data(d) = event(thinking) else { panic!() };
        assert_eq!((d.text.as_str(), d.reasoning.as_str()), ("", "hmm"));
        let last = r#"data: {"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":12,"completion_tokens":3,"cost":0.0004}}"#;
        let Event::Data(d) = event(last) else { panic!() };
        assert_eq!(d.finish.as_deref(), Some("stop"));
        assert_eq!(d.usage, Some(Usage { input: 12, output: 3, cost: 0.0004 }));
        assert_eq!(event("data: [DONE]\r"), Event::Done);
        assert_eq!(event(": OPENROUTER PROCESSING"), Event::Nothing);
        assert_eq!(event(""), Event::Nothing);
        assert_eq!(event("kynex-http-status:402"), Event::Status(402));
        assert_eq!(event("{\"error\":{}}"), Event::Other("{\"error\":{}}".into()));
    }

    #[test]
    fn errors_are_put_in_words() {
        let mid = r#"data: {"error":{"code":502,"message":"Provider returned error","metadata":{"raw":"overloaded"}},"choices":[{"delta":{"content":""},"finish_reason":"error"}]}"#;
        let Event::Data(d) = event(mid) else { panic!() };
        assert_eq!(d.error.as_deref(), Some("Provider returned error (overloaded)"));
        let body = r#"{"error":{"code":402,"message":"Insufficient credits"}}"#;
        assert_eq!(failure(Some(402), body), "API Error: 402 Insufficient credits");
        assert_eq!(failure(Some(500), ""), "API Error: 500");
        assert_eq!(failure(None, "curl: (6) Could not resolve host"), "API Error: curl: (6) Could not resolve host");
    }

    #[test]
    fn the_request_streams_with_usage() {
        let b = body("m/x", 100, &[json!({ "role": "user", "content": "hi" })]);
        assert_eq!(b["stream"], true);
        assert_eq!(b["usage"]["include"], true);
        assert_eq!(b["max_tokens"], 100);
        assert_eq!(b["messages"][0]["content"], "hi");
    }
}
