//! Drives a `claude` CLI process over its stream-json protocol.
//!
//! The CLI owns auth (whatever account `claude` is logged into), the agent
//! loop and the tools. Kynex only renders events and answers permission
//! prompts, which arrive as `control_request` messages on stdout.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread;

use eframe::egui;
use serde_json::{Value, json};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Full Claude Code agent with tools, rooted in a project folder.
    Code,
    /// Plain conversation: same CLI and login, all tools disabled.
    Chat,
}

pub struct Attachment {
    pub media_type: String,
    pub base64: String,
    pub texture: egui::TextureHandle,
}

pub enum Item {
    User { text: String, images: Vec<egui::TextureHandle> },
    Assistant(String),
    Thinking,
    ToolUse { name: String, summary: String, input: String },
    ToolResult { content: String, is_error: bool },
    Permission { request_id: String, tool: String, input: Value, suggestions: Value, state: PermState },
    Result { cost: f64, duration_ms: u64, is_error: bool },
    Error(String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PermState {
    Pending,
    Allowed,
    Denied,
}

enum Event {
    Line(Value),
    Stderr(String),
    Exited,
}

pub struct Session {
    pub kind: Kind,
    pub cwd: PathBuf,
    pub model: String,
    pub items: Vec<Item>,
    pub session_id: Option<String>,
    pub busy: bool,
    pub total_cost: f64,
    child: Option<Child>,
    stdin: Option<Arc<Mutex<ChildStdin>>>,
    rx: Option<Receiver<Event>>,
    streaming_text: bool,
    next_request: u64,
}

impl Session {
    pub fn new(kind: Kind, cwd: PathBuf) -> Self {
        Self {
            kind,
            cwd,
            model: String::new(),
            items: Vec::new(),
            session_id: None,
            busy: false,
            total_cost: 0.0,
            child: None,
            stdin: None,
            rx: None,
            streaming_text: false,
            next_request: 0,
        }
    }

    /// Reopen a previous conversation: replay its transcript from Claude Code's
    /// own session log, and `--resume` it on the next message.
    pub fn resume(kind: Kind, cwd: PathBuf, id: String) -> Self {
        let mut s = Self::new(kind, cwd);
        match find_transcript(&id).and_then(|p| std::fs::read_to_string(p).ok()) {
            Some(log) => {
                for line in log.lines() {
                    let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
                    if v["isSidechain"] == true || v["isMeta"] == true {
                        continue;
                    }
                    if v["type"] == "user" {
                        s.replay_user(&v["message"]["content"]);
                    }
                    s.handle(v);
                }
                s.streaming_text = false;
            }
            None => s.items.push(Item::Error("previous transcript not found; continuing the session anyway".into())),
        }
        s.session_id = Some(id);
        s
    }

    fn replay_user(&mut self, content: &Value) {
        let (text, images) = match content {
            Value::String(t) => (t.clone(), 0),
            Value::Array(blocks) => (
                blocks.iter().filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("\n"),
                blocks.iter().filter(|b| b["type"] == "image").count(),
            ),
            _ => return,
        };
        if text.is_empty() && images == 0 {
            return;
        }
        let text = if images > 0 { format!("[{images} image(s)] {text}") } else { text };
        self.items.push(Item::User { text, images: Vec::new() });
    }

    pub fn is_running(&self) -> bool {
        self.child.is_some()
    }

    fn spawn(&mut self, ctx: &egui::Context) -> Result<(), String> {
        let mut cmd = Command::new("claude");
        cmd.args([
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--include-partial-messages",
            "--permission-prompt-tool",
            "stdio",
        ]);
        if self.kind == Kind::Chat {
            cmd.args(["--tools", ""]);
        }
        if !self.model.is_empty() {
            cmd.args(["--model", &self.model]);
        }
        if let Some(id) = &self.session_id {
            cmd.args(["--resume", id]);
        }
        cmd.current_dir(&self.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        let mut child = cmd.spawn().map_err(|e| format!("failed to start `claude`: {e}"))?;
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        let stdin = child.stdin.take().unwrap();
        let (tx, rx) = channel();

        spawn_reader(stdout, tx.clone(), ctx.clone(), true);
        spawn_reader(stderr, tx, ctx.clone(), false);

        self.child = Some(child);
        self.stdin = Some(Arc::new(Mutex::new(stdin)));
        self.rx = Some(rx);
        Ok(())
    }

    fn write(&mut self, msg: Value) {
        let Some(stdin) = &self.stdin else { return };
        let mut line = msg.to_string();
        line.push('\n');
        let ok = stdin.lock().is_ok_and(|mut s| s.write_all(line.as_bytes()).and_then(|_| s.flush()).is_ok());
        if !ok {
            self.items.push(Item::Error("lost connection to claude process".into()));
            self.kill();
        }
    }

    pub fn send(&mut self, ctx: &egui::Context, text: String, attachments: Vec<Attachment>) {
        if self.child.is_none() {
            if let Err(e) = self.spawn(ctx) {
                self.items.push(Item::Error(e));
                return;
            }
        }
        let mut content = Vec::new();
        let mut images = Vec::new();
        for a in attachments {
            content.push(json!({
                "type": "image",
                "source": { "type": "base64", "media_type": a.media_type, "data": a.base64 }
            }));
            images.push(a.texture);
        }
        if !text.is_empty() {
            content.push(json!({ "type": "text", "text": text }));
        }
        self.items.push(Item::User { text, images });
        self.busy = true;
        self.write(json!({ "type": "user", "message": { "role": "user", "content": content } }));
    }

    pub fn interrupt(&mut self) {
        self.next_request += 1;
        let id = format!("kynex-{}", self.next_request);
        self.write(json!({ "type": "control_request", "request_id": id, "request": { "subtype": "interrupt" } }));
    }

    pub fn answer_permission(&mut self, index: usize, allow: bool, remember: bool) {
        let Some(Item::Permission { request_id, input, suggestions, state, .. }) = self.items.get_mut(index) else {
            return;
        };
        *state = if allow { PermState::Allowed } else { PermState::Denied };
        let response = if allow {
            let mut r = json!({ "behavior": "allow", "updatedInput": input });
            if remember && suggestions.is_array() {
                r["updatedPermissions"] = suggestions.clone();
            }
            r
        } else {
            json!({ "behavior": "deny", "message": "The user denied this action in Kynex." })
        };
        let msg = json!({
            "type": "control_response",
            "response": { "subtype": "success", "request_id": request_id.clone(), "response": response }
        });
        self.write(msg);
    }

    pub fn kill(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.stdin = None;
        self.rx = None;
        self.busy = false;
    }

    /// Drain pending events from the process. Call once per frame.
    pub fn poll(&mut self) {
        let Some(rx) = &self.rx else { return };
        let events: Vec<Event> = rx.try_iter().collect();
        for ev in events {
            match ev {
                Event::Line(v) => self.handle(v),
                Event::Stderr(s) => {
                    if !s.trim().is_empty() {
                        self.items.push(Item::Error(s));
                    }
                }
                Event::Exited => {
                    if self.busy {
                        self.items.push(Item::Error("claude process exited".into()));
                    }
                    self.kill();
                    return;
                }
            }
        }
    }

    fn handle(&mut self, v: Value) {
        if let Some(id) = v["session_id"].as_str() {
            self.session_id = Some(id.to_string());
        }
        match v["type"].as_str().unwrap_or("") {
            "stream_event" => self.handle_stream(&v["event"]),
            "assistant" => {
                for block in v["message"]["content"].as_array().into_iter().flatten() {
                    match block["type"].as_str() {
                        // Already rendered live from the stream deltas.
                        Some("text") if self.streaming_text => {}
                        Some("text") => self.items.push(Item::Assistant(block["text"].as_str().unwrap_or("").into())),
                        Some("thinking") => {
                            if !matches!(self.items.last(), Some(Item::Thinking)) {
                                self.items.push(Item::Thinking);
                            }
                        }
                        Some("tool_use") => {
                            let name = block["name"].as_str().unwrap_or("tool").to_string();
                            let summary = tool_summary(&name, &block["input"]);
                            let input = serde_json::to_string_pretty(&block["input"]).unwrap_or_default();
                            self.items.push(Item::ToolUse { name, summary, input });
                        }
                        _ => {}
                    }
                }
                self.streaming_text = false;
            }
            "user" => {
                for block in v["message"]["content"].as_array().into_iter().flatten() {
                    if block["type"] == "tool_result" {
                        self.items.push(Item::ToolResult {
                            content: tool_result_text(&block["content"]),
                            is_error: block["is_error"].as_bool().unwrap_or(false),
                        });
                    }
                }
            }
            "control_request" if v["request"]["subtype"] == "can_use_tool" => {
                self.items.push(Item::Permission {
                    request_id: v["request_id"].as_str().unwrap_or("").into(),
                    tool: v["request"]["display_name"]
                        .as_str()
                        .or(v["request"]["tool_name"].as_str())
                        .unwrap_or("tool")
                        .into(),
                    input: v["request"]["input"].clone(),
                    suggestions: v["request"]["permission_suggestions"].clone(),
                    state: PermState::Pending,
                });
            }
            "result" => {
                let cost = v["total_cost_usd"].as_f64().unwrap_or(0.0);
                self.total_cost = cost.max(self.total_cost);
                self.items.push(Item::Result {
                    cost,
                    duration_ms: v["duration_ms"].as_u64().unwrap_or(0),
                    is_error: v["is_error"].as_bool().unwrap_or(false),
                });
                self.busy = false;
            }
            _ => {}
        }
    }

    fn handle_stream(&mut self, ev: &Value) {
        match ev["type"].as_str() {
            Some("content_block_start") if ev["content_block"]["type"] == "text" => {
                self.items.push(Item::Assistant(String::new()));
                self.streaming_text = true;
            }
            Some("content_block_delta") => {
                if let Some(t) = ev["delta"]["text"].as_str() {
                    if let Some(Item::Assistant(s)) = self.items.last_mut() {
                        s.push_str(t);
                    }
                }
            }
            _ => {}
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.kill();
    }
}

fn spawn_reader(stream: impl std::io::Read + Send + 'static, tx: Sender<Event>, ctx: egui::Context, stdout: bool) {
    thread::spawn(move || {
        for line in BufReader::new(stream).lines() {
            let Ok(line) = line else { break };
            let ev = if stdout {
                match serde_json::from_str(&line) {
                    Ok(v) => Event::Line(v),
                    Err(_) => continue,
                }
            } else {
                Event::Stderr(line)
            };
            if tx.send(ev).is_err() {
                return;
            }
            ctx.request_repaint();
        }
        if stdout {
            let _ = tx.send(Event::Exited);
            ctx.request_repaint();
        }
    });
}

/// Claude Code stores each session as `~/.claude/projects/<encoded cwd>/<id>.jsonl`.
fn find_transcript(id: &str) -> Option<PathBuf> {
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?;
    let projects = PathBuf::from(home).join(".claude").join("projects");
    std::fs::read_dir(projects)
        .ok()?
        .filter_map(Result::ok)
        .map(|d| d.path().join(format!("{id}.jsonl")))
        .find(|p| p.is_file())
}

fn tool_summary(name: &str, input: &Value) -> String {
    let field = match name {
        "Bash" | "PowerShell" => "command",
        "Read" | "Write" | "Edit" | "NotebookEdit" => "file_path",
        "Glob" | "Grep" => "pattern",
        "WebFetch" => "url",
        "WebSearch" => "query",
        "Task" | "Agent" => "description",
        _ => "",
    };
    let s = input[field].as_str().unwrap_or("");
    let first = s.lines().next().unwrap_or("");
    if first.chars().count() > 120 { format!("{}…", first.chars().take(120).collect::<String>()) } else { first.into() }
}

fn tool_result_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|p| p["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// Talks to the real `claude` CLI, so it uses your login and a little quota.
    /// Run with `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn chat_round_trip() {
        let ctx = egui::Context::default();
        let mut s = Session::new(Kind::Chat, std::env::temp_dir());
        s.model = "haiku".into();
        s.send(&ctx, "Reply with exactly: pong".into(), Vec::new());
        let start = Instant::now();
        while s.busy && start.elapsed() < Duration::from_secs(90) {
            s.poll();
            std::thread::sleep(Duration::from_millis(50));
        }
        let text: String = s.items.iter().filter_map(|i| if let Item::Assistant(t) = i { Some(t.as_str()) } else { None }).collect();
        assert!(text.to_lowercase().contains("pong"), "got: {text:?}");
        assert!(s.items.iter().any(|i| matches!(i, Item::Result { is_error: false, .. })));
        assert!(s.session_id.is_some());
    }
}
