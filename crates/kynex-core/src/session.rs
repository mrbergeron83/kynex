//! Drives a `claude` CLI process over its stream-json protocol.
//!
//! The CLI owns auth (whatever account `claude` is logged into), the agent
//! loop and the tools. Kynex only renders events and answers permission
//! prompts, which arrive as `control_request` messages on stdout.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, SystemTime};

use serde_json::{Value, json};

use crate::commands::SlashCommand;
use crate::queue::Queue;
use crate::tasks::{self, Tasks};
use crate::{project, wsl};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Full Claude Code agent with tools, rooted in a project folder.
    Code,
    /// Plain conversation: same CLI and login, all tools disabled.
    Chat,
}

/// The UI's own handle to an image (Kynex puts an `Arc<gpui::Image>` here), kept
/// opaque so this crate needn't depend on the UI toolkit.
pub type Preview = Arc<dyn std::any::Any + Send + Sync>;

/// Makes a [`Preview`] from an image's media type and bytes. The app sets one
/// at startup, so images read back from logs and tool results show too.
pub type MakePreview = fn(&str, Vec<u8>) -> Option<Preview>;

static MAKE_PREVIEW: OnceLock<MakePreview> = OnceLock::new();

/// Sets how images found in messages become previews; the first call wins.
pub fn set_preview_maker(make: MakePreview) {
    let _ = MAKE_PREVIEW.set(make);
}

/// Previews of the images among `content`'s blocks, if the app makes them.
fn previews_in(content: &Value) -> Vec<Preview> {
    let Some(make) = MAKE_PREVIEW.get() else { return Vec::new() };
    crate::attach::images_in(content).into_iter().filter_map(|i| make(&i.media_type, i.bytes)).collect()
}

pub struct Attachment {
    pub media_type: String,
    pub base64: String,
    pub preview: Preview,
}

/// Called from the reader threads whenever the process has news, so the UI drains it.
pub type Wake = Arc<dyn Fn() + Send + Sync>;

pub enum Item {
    User {
        text: String,
        images: Vec<Preview>,
    },
    Assistant(String),
    Thinking,
    ToolUse {
        /// The call's `tool_use_id`, which its result names.
        id: String,
        name: String,
        summary: String,
        input: String,
    },
    ToolResult {
        /// The `tool_use_id` of the call this answers.
        id: String,
        content: String,
        /// Images it returned: a `Read` of a picture, a browser screenshot.
        images: Vec<Preview>,
        is_error: bool,
    },
    /// A `can_use_tool` request. `tool` is the tool's name (`Bash`, `mcp__x__y`),
    /// `description` the CLI's one-line summary of what it's for.
    Permission {
        request_id: String,
        tool: String,
        input: Value,
        suggestions: Value,
        description: String,
        state: PermState,
    },
    /// The end of a turn, with a recap of it: how many tools ran and which files changed.
    Result {
        cost: f64,
        duration_ms: u64,
        is_error: bool,
        tools: usize,
        files: Vec<String>,
    },
    /// A background task ended (or a Monitor saw something); Claude's turn about it follows.
    Task(tasks::Notice),
    Error(String),
}

/// Where a prompt stands; answered ones say how, e.g. "allowed for this session".
#[derive(Clone, PartialEq, Eq)]
pub enum PermState {
    Pending,
    Allowed(String),
    Denied(String),
}

#[derive(Clone, PartialEq)]
pub enum Answer {
    Once,
    /// Allow, and stop asking for the same thing until this process ends.
    Session,
    /// Allow, and save the rule in the project's Kynex settings.
    Project,
    /// Allow every tool of this MCP server until this process ends.
    Server,
    /// Approve a plan (`ExitPlanMode`) and build it in this mode.
    Plan(project::Mode),
    /// Answers to `AskUserQuestion`: question text → the chosen label(s), comma-separated.
    Questions(serde_json::Map<String, Value>),
    /// Say no. With feedback Claude reads it and carries on; without, the turn
    /// stops, like Esc in the CLI.
    Deny(String),
}

enum Event {
    Line(Value),
    Stderr(String),
    Exited,
}

/// How full a session's context window is, as of Claude's last reply.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ContextUse {
    /// Tokens the last request sent: input plus cache reads and writes.
    pub tokens: u64,
    /// The model's window, once the CLI has reported it (at the end of a turn).
    pub window: Option<u64>,
}

impl ContextUse {
    /// The share of the window in use, 0.0–1.0, when the window is known.
    pub fn fraction(&self) -> Option<f64> {
        self.window.filter(|&w| w > 0).map(|w| (self.tokens as f64 / w as f64).min(1.0))
    }
}

/// Context windows the CLI has reported this run, by model id, so a resumed
/// session (whose transcript has no `result` lines) knows its window too.
fn windows() -> &'static Mutex<HashMap<String, u64>> {
    static WINDOWS: OnceLock<Mutex<HashMap<String, u64>>> = OnceLock::new();
    WINDOWS.get_or_init(Default::default)
}

fn known_window(model: &str) -> Option<u64> {
    windows().lock().ok()?.get(model).copied()
}

pub struct Session {
    /// Unique among this run's sessions, so the UI can tell them apart before
    /// (and regardless of) the CLI assigning a `session_id`.
    pub uid: u64,
    pub kind: Kind,
    pub cwd: PathBuf,
    pub model: String,
    pub items: Vec<Item>,
    pub session_id: Option<String>,
    pub busy: bool,
    /// A turn ended and the user hasn't looked at it yet; the UI clears it.
    pub finished: bool,
    pub total_cost: f64,
    pub context: ContextUse,
    /// The live permission mode: starts at the project's default, follows the
    /// CLI's reports, and is switched from the composer without a restart.
    pub mode: project::Mode,
    /// Messages written while Claude works; the next goes when a turn ends.
    pub queue: Queue,
    /// Background tasks running in this session's process.
    pub tasks: Tasks,
    /// The MCP servers its process reported when it started, and how they're doing.
    pub mcp: Vec<crate::mcp::Server>,
    /// Set by the UI so it hears about new events; a no-op until then.
    pub wake: Wake,
    child: Option<Child>,
    stdin: Option<Arc<Mutex<ChildStdin>>>,
    rx: Option<Receiver<Event>>,
    streaming_text: bool,
    next_request: u64,
    /// The model that wrote the last reply, whose window `context` is measured against.
    reply_model: String,
    /// What the process reported when it started, until the app takes it.
    init: Option<Init>,
    /// The log entry a rewind went back to, which the next process continues from.
    resume_at: Option<String>,
}

/// What a session's process reports as it starts (the `initialize` handshake):
/// its folder's slash commands and the account's models.
pub struct Init {
    pub commands: Vec<SlashCommand>,
    pub models: Vec<ModelInfo>,
}

/// The request id of the `initialize` handshake each process starts with.
const INIT_ID: &str = "kynex-init";

impl Session {
    pub fn new(kind: Kind, cwd: PathBuf) -> Self {
        let mode = match kind {
            Kind::Code => project::ClaudeSettings::load(&cwd).mode,
            Kind::Chat => project::Mode::default(),
        };
        static NEXT_UID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self {
            uid: NEXT_UID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            kind,
            cwd,
            model: String::new(),
            items: Vec::new(),
            session_id: None,
            busy: false,
            finished: false,
            total_cost: 0.0,
            context: ContextUse::default(),
            mode,
            queue: Queue::default(),
            tasks: Tasks::default(),
            mcp: Vec::new(),
            wake: Arc::new(|| {}),
            child: None,
            stdin: None,
            rx: None,
            streaming_text: false,
            next_request: 0,
            reply_model: String::new(),
            init: None,
            resume_at: None,
        }
    }

    /// Reopen a previous conversation: replay its transcript from Claude Code's
    /// own session log, and `--resume` it on the next message.
    pub fn resume(kind: Kind, cwd: PathBuf, id: String) -> Self {
        let mut s = Self::new(kind, cwd);
        s.session_id = Some(id);
        if !s.replay(None) {
            s.items.push(Item::Error("previous transcript not found; continuing the session anyway".into()));
        }
        s
    }

    /// Fills the transcript from the session log's current branch, up to and
    /// including entry `until` if given. False if there's no log to read.
    fn replay(&mut self, until: Option<&str>) -> bool {
        let id = self.session_id.clone().unwrap_or_default();
        let Some(log) = find_transcript(&self.cwd, &id).and_then(|p| std::fs::read_to_string(p).ok()) else {
            return false;
        };
        for v in crate::rewind::branch(&log) {
            let last = until.is_some_and(|u| v["uuid"] == u);
            self.replay_entry(v);
            if last {
                break;
            }
        }
        // Replayed ids are this session's own; a slip shouldn't change it.
        self.session_id = Some(id);
        self.streaming_text = false;
        true
    }

    /// Goes back to just before `point`: its transcript ends there and the next
    /// message continues from there, leaving out what came after (it stays in
    /// the log, on its own branch). Back to the first message, it starts over
    /// as a new session. Stops the process; blocks on reading the log.
    pub fn rewind_to(&mut self, point: &crate::rewind::Point) {
        self.kill();
        self.items.clear();
        self.context = ContextUse::default();
        self.queue = Queue::default();
        self.finished = false;
        match &point.before {
            Some(before) => {
                self.replay(Some(before));
                self.resume_at = Some(before.clone());
            }
            None => {
                self.session_id = None;
                self.resume_at = None;
            }
        }
    }

    /// One entry of a session log, into the transcript.
    pub(crate) fn replay_entry(&mut self, v: Value) {
        if v["isMeta"] == true {
            return;
        }
        if v["type"] == "user" && !written_by_cli(&v) {
            self.replay_user(&v["message"]["content"]);
        } else if let Some(n) = v["message"]["content"].as_str().and_then(tasks::parse_notification) {
            self.items.push(Item::Task(n));
        }
        self.handle(v);
    }

    fn replay_user(&mut self, content: &Value) {
        let (text, sent) = match content {
            Value::String(t) => (t.clone(), 0),
            Value::Array(blocks) => (
                blocks.iter().filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("\n"),
                blocks.iter().filter(|b| b["type"] == "image").count(),
            ),
            _ => return,
        };
        if text.is_empty() && sent == 0 {
            return;
        }
        // A slash command is logged in `<command-name>` wrappers; show it as typed.
        let text = crate::commands::unwrap(&text).unwrap_or(text);
        let images = previews_in(content);
        // Images that can't be shown are at least counted.
        let missing = sent.saturating_sub(images.len());
        let text = if missing > 0 { format!("[{missing} image(s)] {text}") } else { text };
        self.items.push(Item::User { text, images });
    }

    pub fn is_running(&self) -> bool {
        self.child.is_some()
    }

    /// Whether a permission prompt or question is waiting on the user.
    pub fn waiting(&self) -> bool {
        self.items.iter().any(|i| matches!(i, Item::Permission { state: PermState::Pending, .. }))
    }

    /// Whether this session wants the user: a turn ended unseen, or a prompt waits.
    pub fn wants_you(&self) -> bool {
        self.finished || self.waiting()
    }

    fn spawn(&mut self) -> Result<(), String> {
        let mut cmd = claude_command(&self.cwd);
        // Checkpoints of the files Claude edits, so a rewind can restore them.
        cmd.env(crate::rewind::CHECKPOINTS_ENV, "1");
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
        match self.kind {
            Kind::Chat => {
                cmd.args(["--tools", ""]);
            }
            // Read fresh on every start, so edits in Claude settings apply to the next process.
            Kind::Code => {
                if let Some(settings) = project::Permissions::load(&self.cwd).as_cli_settings() {
                    cmd.args(["--settings", &settings]);
                }
                cmd.args(["--permission-mode", self.mode.cli()]);
                cmd.args(project::ClaudeSettings::load(&self.cwd).cli_args());
            }
        }
        if !self.model.is_empty() {
            cmd.args(["--model", &self.model]);
        }
        if let Some(id) = &self.session_id {
            cmd.args(["--resume", id]);
            // After a rewind: continue from there, branching off what came after.
            if let Some(at) = self.resume_at.take() {
                cmd.args(["--resume-session-at", &at]);
            }
        }
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());

        let mut child =
            crate::process::dies_with_parent(&mut cmd).spawn().map_err(|e| format!("failed to start `claude`: {e}"))?;
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        let stdin = child.stdin.take().unwrap();
        let (tx, rx) = channel();

        spawn_readers(stdout, stderr, tx, self.wake.clone());

        self.child = Some(child);
        self.stdin = Some(Arc::new(Mutex::new(stdin)));
        self.rx = Some(rx);
        // Ask for the commands and models first, as the Agent SDK does; the
        // answer comes before any reply, and nothing else about the session changes.
        self.write(json!({ "type": "control_request", "request_id": INIT_ID, "request": { "subtype": "initialize" } }));
        Ok(())
    }

    /// The commands and models the process reported when it started, once.
    pub fn take_init(&mut self) -> Option<Init> {
        self.init.take()
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

    pub fn send(&mut self, text: String, attachments: Vec<Attachment>) {
        if self.child.is_none()
            && let Err(e) = self.spawn()
        {
            self.items.push(Item::Error(e));
            return;
        }
        let mut content = Vec::new();
        let mut images = Vec::new();
        for a in attachments {
            content.push(json!({
                "type": "image",
                "source": { "type": "base64", "media_type": a.media_type, "data": a.base64 }
            }));
            images.push(a.preview);
        }
        if !text.is_empty() {
            content.push(json!({ "type": "text", "text": text }));
        }
        self.items.push(Item::User { text, images });
        self.busy = true;
        self.write(json!({ "type": "user", "message": { "role": "user", "content": content } }));
    }

    /// Stops the turn. Anything queued goes out as soon as it has ended.
    pub fn interrupt(&mut self) {
        self.control(json!({ "subtype": "interrupt" }));
    }

    /// Sends the queue now: right away when idle, otherwise it stops the turn
    /// and the queue goes as soon as the turn has ended.
    pub fn send_queue_now(&mut self) {
        if self.busy {
            self.interrupt();
        } else if let Some(m) = self.queue.take_all() {
            self.send(m.text, m.attachments);
        }
    }

    /// Switches the permission mode, live if the process is running (like
    /// Shift+Tab in the CLI), otherwise from the next start.
    pub fn set_mode(&mut self, mode: project::Mode) {
        self.mode = mode;
        if self.child.is_some() {
            self.control(json!({ "subtype": "set_permission_mode", "mode": mode.cli() }));
        }
    }

    fn control(&mut self, request: Value) {
        self.next_request += 1;
        let id = format!("kynex-{}", self.next_request);
        self.write(json!({ "type": "control_request", "request_id": id, "request": request }));
    }

    /// Answers a permission prompt. For [`Answer::Project`] it returns the rules
    /// to save in the project's settings; the live process gets them right away
    /// as session rules, and later processes get them from the saved settings.
    pub fn answer_permission(&mut self, index: usize, answer: Answer) -> Vec<String> {
        let Some(Item::Permission { request_id, tool, input, suggestions, state, .. }) = self.items.get_mut(index)
        else {
            return Vec::new();
        };
        if *state != PermState::Pending {
            return Vec::new();
        }
        let rules = if answer == Answer::Project { project::suggested_rules(suggestions) } else { Vec::new() };
        let mut plan_mode = None;
        let (response, new_state) = match answer {
            Answer::Deny(feedback) if feedback.trim().is_empty() => (
                json!({ "behavior": "deny", "message": "The user doesn't want to proceed with this tool use.", "interrupt": true }),
                PermState::Denied("denied".into()),
            ),
            Answer::Deny(feedback) => (
                json!({
                    "behavior": "deny",
                    "message": format!("The user doesn't want to proceed with this tool use. They said: {}", feedback.trim()),
                }),
                PermState::Denied(format!("denied: {}", feedback.trim())),
            ),
            Answer::Once => {
                (json!({ "behavior": "allow", "updatedInput": input }), PermState::Allowed("allowed".into()))
            }
            Answer::Session | Answer::Project => {
                // Kynex keeps project rules itself, so the CLI never writes them to its settings files.
                let mut updates = suggestions.clone();
                for u in updates.as_array_mut().into_iter().flatten() {
                    u["destination"] = json!("session");
                }
                let note = if rules.is_empty() { "allowed for this session" } else { "always allowed in this project" };
                (
                    json!({ "behavior": "allow", "updatedInput": input, "updatedPermissions": updates }),
                    PermState::Allowed(note.into()),
                )
            }
            Answer::Server => {
                // `mcp__<server>` with no tool covers all of that server's tools.
                let rule = crate::mcp::server_rule(tool).unwrap_or_else(|| tool.clone());
                let updates = json!([{
                    "type": "addRules",
                    "rules": [{ "toolName": rule }],
                    "behavior": "allow",
                    "destination": "session",
                }]);
                let label = crate::mcp::split(tool).map_or_else(|| tool.clone(), |(s, _)| crate::mcp::server_label(s));
                (
                    json!({ "behavior": "allow", "updatedInput": input, "updatedPermissions": updates }),
                    PermState::Allowed(format!("all {label} actions allowed this session")),
                )
            }
            Answer::Plan(mode) => {
                plan_mode = Some(mode);
                (
                    json!({ "behavior": "allow", "updatedInput": input }),
                    PermState::Allowed(format!("plan approved · {}", mode.label())),
                )
            }
            Answer::Questions(answers) => {
                let note = answers.values().filter_map(Value::as_str).collect::<Vec<_>>().join(" · ");
                let mut updated = input.clone();
                updated["answers"] = Value::Object(answers);
                (
                    json!({ "behavior": "allow", "updatedInput": updated }),
                    PermState::Allowed(format!("answered: {note}")),
                )
            }
        };
        let approves_plan =
            matches!(new_state, PermState::Allowed(_)) && tool.replace(' ', "").eq_ignore_ascii_case("ExitPlanMode");
        *state = new_state;
        let msg = json!({
            "type": "control_response",
            "response": { "subtype": "success", "request_id": request_id.clone(), "response": response }
        });
        self.write(msg);
        // An approved plan means "go build it": in the mode picked with it, else
        // leave plan mode for the project's default, or Auto if that's Plan itself.
        if let Some(mode) = plan_mode {
            self.set_mode(mode);
        } else if approves_plan && self.mode == project::Mode::Plan {
            let default = match self.kind {
                Kind::Code => project::ClaudeSettings::load(&self.cwd).mode,
                Kind::Chat => project::Mode::default(),
            };
            self.set_mode(if default == project::Mode::Plan { project::Mode::Auto } else { default });
        }
        rules
    }

    pub fn kill(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.stdin = None;
        self.rx = None;
        self.busy = false;
        self.tasks.clear();
        // Nobody is left to answer them.
        self.cancel_permissions(|_| true);
    }

    /// Marks the pending permission prompts `which` picks as cancelled.
    fn cancel_permissions(&mut self, which: impl Fn(&str) -> bool) {
        for item in &mut self.items {
            if let Item::Permission { request_id, state, .. } = item
                && *state == PermState::Pending
                && which(request_id)
            {
                *state = PermState::Denied("cancelled".into());
            }
        }
    }

    /// Whether background tasks are running, which closing the process would stop.
    pub fn has_tasks(&self) -> bool {
        !self.tasks.running.is_empty()
    }

    /// Drain pending events from the process. Call when woken.
    pub fn poll(&mut self) {
        let Some(rx) = &self.rx else { return };
        let events: Vec<Event> = rx.try_iter().collect();
        let mut exited = false;
        for ev in events {
            match ev {
                Event::Line(v) => self.handle(v),
                Event::Stderr(s) => {
                    if !s.trim().is_empty() {
                        self.items.push(Item::Error(s));
                    }
                }
                Event::Exited => exited = true,
            }
        }
        if exited {
            if self.busy {
                self.items.push(Item::Error("claude process exited".into()));
            }
            self.kill();
        }
    }

    pub(crate) fn handle(&mut self, v: Value) {
        if let Some(id) = v["session_id"].as_str() {
            self.session_id = Some(id.to_string());
        }
        // `system` init and status messages report the mode, e.g. after a plan is approved.
        if let Some(mode) = v["permissionMode"].as_str().and_then(project::Mode::from_cli)
            && self.child.is_some()
        {
            self.mode = mode;
        }
        match v["type"].as_str().unwrap_or("") {
            "stream_event" => self.handle_stream(&v["event"]),
            "control_response" if v["response"]["request_id"] == INIT_ID => {
                let r = &v["response"]["response"];
                self.init = Some(Init { commands: crate::commands::from_init(r), models: models_from_init(r) });
            }
            // `/compact` finished: the context is down to its summary.
            // Without a size after it, the meter waits for the next reply.
            "system" if v["subtype"] == "compact_boundary" => {
                self.context.tokens = v["compact_metadata"]["post_tokens"].as_u64().unwrap_or(0);
            }
            // Each process start reports its MCP servers and whether they connected.
            "system" if v["subtype"] == "init" => self.mcp = crate::mcp::servers(&v),
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
                            let id = block["id"].as_str().unwrap_or("").to_string();
                            self.items.push(Item::ToolUse { id, name, summary, input });
                        }
                        _ => {}
                    }
                }
                self.streaming_text = false;
            }
            "user" => {
                for block in v["message"]["content"].as_array().into_iter().flatten() {
                    if block["type"] == "tool_result" {
                        let id = block["tool_use_id"].as_str().unwrap_or("").to_string();
                        let content = tool_result_text(&block["content"]);
                        self.tasks.tool_result(&id, &content);
                        self.items.push(Item::ToolResult {
                            id,
                            content,
                            images: previews_in(&block["content"]),
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
                    state: PermState::Pending,
                });
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
                self.total_cost = cost.max(self.total_cost);
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
                if let Some(next) = self.queue.take_all() {
                    self.send(next.text, next.attachments);
                }
            }
            _ => {}
        }
    }

    /// Tool calls since the last user message, and the file names they wrote to.
    fn turn_recap(&self) -> (usize, Vec<String>) {
        let start = self.items.iter().rposition(|i| matches!(i, Item::User { .. })).map_or(0, |i| i + 1);
        let mut tools = 0;
        let mut files: Vec<String> = Vec::new();
        for item in &self.items[start..] {
            let Item::ToolUse { name, input, .. } = item else { continue };
            tools += 1;
            if !matches!(name.as_str(), "Edit" | "MultiEdit" | "Write" | "NotebookEdit") {
                continue;
            }
            let v: Value = serde_json::from_str(input).unwrap_or_default();
            let path = v["file_path"].as_str().or(v["notebook_path"].as_str()).unwrap_or("");
            let file = Path::new(path).file_name().map(|f| f.to_string_lossy().into_owned());
            if let Some(f) = file.filter(|f| !files.contains(f)) {
                files.push(f);
            }
        }
        (tools, files)
    }

    fn handle_stream(&mut self, ev: &Value) {
        match ev["type"].as_str() {
            Some("content_block_start") if ev["content_block"]["type"] == "text" => {
                self.items.push(Item::Assistant(String::new()));
                self.streaming_text = true;
            }
            Some("content_block_delta") => {
                if let Some(t) = ev["delta"]["text"].as_str()
                    && let Some(Item::Assistant(s)) = self.items.last_mut()
                {
                    s.push_str(t);
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

/// Reads the process's stdout and stderr on two threads. `Exited` goes out once
/// stdout closes and stderr has had its say, so the reason a process died
/// (a bad `--model`, a failed `--resume`) arrives before it.
fn spawn_readers(
    stdout: impl std::io::Read + Send + 'static,
    stderr: impl std::io::Read + Send + 'static,
    tx: Sender<Event>,
    wake: Wake,
) {
    // Dropped when the stderr thread ends.
    let (stderr_open, stderr_done) = channel::<()>();
    let (err_tx, err_wake) = (tx.clone(), wake.clone());
    thread::spawn(move || {
        let _open = stderr_open;
        for line in BufReader::new(stderr).lines() {
            let Ok(line) = line else { break };
            if err_tx.send(Event::Stderr(line)).is_err() {
                return;
            }
            err_wake();
        }
    });
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            let Ok(v) = serde_json::from_str(&line) else { continue };
            if tx.send(Event::Line(v)).is_err() {
                return;
            }
            wake();
        }
        // A grandchild that inherited stderr can hold it open; don't wait on it for long.
        let _ = stderr_done.recv_timeout(STDERR_GRACE);
        let _ = tx.send(Event::Exited);
        wake();
    });
}

/// How long `Exited` waits for stderr to close after stdout has.
const STDERR_GRACE: Duration = Duration::from_millis(500);

/// A past Claude Code conversation in some directory.
#[derive(Clone)]
pub struct SessionInfo {
    pub id: String,
    /// Its name: a rename (`custom-title`), else Claude's own (`ai-title`), else
    /// the one Kynex made, else the first thing the user said.
    pub title: String,
    /// The first thing the user said.
    pub first: String,
    /// `title` is a real name, not just the first message.
    pub named: bool,
    pub modified: SystemTime,
}

/// `~/.claude/projects`, under the WSL user's home for projects that live in WSL.
fn projects_dir(cwd: &Path) -> Option<PathBuf> {
    let home = match wsl::parse(cwd) {
        Some(w) => w.home()?,
        None => PathBuf::from(std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?),
    };
    Some(home.join(".claude").join("projects"))
}

/// Claude Code keeps a directory's sessions in `~/.claude/projects/<cwd>`, with
/// every character that isn't a letter or digit replaced by `-`.
fn transcripts_dir(cwd: &Path) -> Option<PathBuf> {
    let path = match wsl::parse(cwd) {
        Some(w) => w.path,
        None => cwd.to_string_lossy().into_owned(),
    };
    let name: String = path.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    // The drive letter's case depends on who launched Claude (`C:` vs `c:`).
    std::fs::read_dir(projects_dir(cwd)?)
        .ok()?
        .filter_map(Result::ok)
        .map(|d| d.path())
        .find(|p| p.is_dir() && p.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(&name)))
}

/// Each session is `<transcripts dir>/<id>.jsonl`.
pub(crate) fn find_transcript(cwd: &Path, id: &str) -> Option<PathBuf> {
    let file = format!("{id}.jsonl");
    if let Some(p) = transcripts_dir(cwd).map(|d| d.join(&file)).filter(|p| p.is_file()) {
        return Some(p);
    }
    // Fall back to every project, in case the folder was renamed or moved.
    std::fs::read_dir(projects_dir(cwd)?)
        .ok()?
        .filter_map(Result::ok)
        .map(|d| d.path().join(&file))
        .find(|p| p.is_file())
}

/// A user-role log entry the CLI wrote itself, like the `<task-notification>`
/// it sends Claude when a background command ends: Claude reads it, but it
/// isn't something the user said.
fn written_by_cli(v: &Value) -> bool {
    let text = v["message"]["content"].as_str().unwrap_or("");
    v["promptSource"] == "system" || v["origin"]["kind"].is_string() || text.starts_with("<task-notification>")
}

/// `claude`, to be run in `cwd` (inside WSL for a WSL folder), without a console window.
pub(crate) fn claude_command(cwd: &Path) -> Command {
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut cmd = match wsl::parse(cwd) {
        Some(w) => w.command("claude"),
        None => {
            let mut c = Command::new(crate::cli::program());
            c.current_dir(cwd);
            c
        }
    };
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// A model the logged-in account can use, as the CLI reports it.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ModelInfo {
    /// What to pass to `--model`: an alias (`opus`), a full name, or `default`.
    pub value: String,
    pub name: String,
    pub description: String,
    /// `--effort` levels it takes; empty if it has no effort setting.
    pub efforts: Vec<String>,
}

/// What the CLI reports in the stream-json `initialize` handshake when started
/// in `cwd`: models, slash commands, agents and so on. Sends no message, so it
/// costs nothing. Blocks for a second or two; call it off the UI thread.
pub(crate) fn initialize(cwd: &Path) -> Result<Value, String> {
    let mut cmd = claude_command(cwd);
    cmd.args(["-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child =
        crate::process::dies_with_parent(&mut cmd).spawn().map_err(|e| format!("failed to start `claude`: {e}"))?;
    let request = json!({ "type": "control_request", "request_id": INIT_ID, "request": { "subtype": "initialize" } });
    let sent = child.stdin.as_mut().is_some_and(|s| writeln!(s, "{request}").and_then(|_| s.flush()).is_ok());
    let mut response = None;
    if sent {
        for line in BufReader::new(child.stdout.take().unwrap()).lines() {
            let Ok(line) = line else { break };
            let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
            if v["type"] == "control_response" {
                response = Some(v);
                break;
            }
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    let mut v = response.ok_or("claude didn't answer the initialize request")?;
    Ok(v["response"]["response"].take())
}

/// Asks the CLI which models this account can use. Blocks; see [`initialize`].
/// Only for when there's no saved list yet: sessions report it when they start.
pub fn list_models() -> Result<Vec<ModelInfo>, String> {
    let v = initialize(&std::env::temp_dir())?;
    let models = models_from_init(&v);
    if models.is_empty() {
        return Err("no models in the initialize response".into());
    }
    Ok(models)
}

/// The models a session last reported, saved so the picker has them at once
/// after a restart. Empty if there's none. Blocks on a small file read.
pub fn saved_models() -> Vec<ModelInfo> {
    let path = crate::paths::data_dir().join("models.json");
    std::fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

/// Saves the models for [`saved_models`]. Blocks on a small file write.
pub fn save_models(models: &[ModelInfo]) -> Result<(), String> {
    let dir = crate::paths::data_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("Couldn't create {}: {e}", dir.display()))?;
    project::write_json(&dir.join("models.json"), &models)
}

/// The models in an `initialize` response.
fn models_from_init(v: &Value) -> Vec<ModelInfo> {
    v["models"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| {
            Some(ModelInfo {
                value: m["value"].as_str()?.to_string(),
                name: m["displayName"].as_str().unwrap_or("").to_string(),
                description: m["description"].as_str().unwrap_or("").to_string(),
                efforts: m["supportedEffortLevels"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|e| e.as_str().map(str::to_string))
                    .collect(),
            })
        })
        .collect()
}

/// Past sessions in `cwd`, newest first.
pub fn list_sessions(cwd: &Path) -> Vec<SessionInfo> {
    let Some(entries) = transcripts_dir(cwd).and_then(|d| std::fs::read_dir(d).ok()) else { return Vec::new() };
    let mut sessions: Vec<SessionInfo> = entries
        .filter_map(Result::ok)
        .filter_map(|e| {
            let path = e.path();
            if path.extension()? != "jsonl" {
                return None;
            }
            let meta = e.metadata().ok();
            let modified = meta.as_ref().and_then(|m| m.modified().ok()).unwrap_or(SystemTime::UNIX_EPOCH);
            let id = path.file_stem()?.to_string_lossy().into_owned();
            let names = transcript_names(&path, modified, meta.map_or(0, |m| m.len()))?;
            let name = names.title.or_else(|| crate::titles::stored(&id));
            Some(SessionInfo {
                title: name.clone().unwrap_or_else(|| names.first.clone()),
                named: name.is_some(),
                first: names.first,
                id,
                modified,
            })
        })
        .collect();
    sessions.sort_by_key(|s| std::cmp::Reverse(s.modified));
    sessions
}

#[derive(Clone)]
struct Names {
    /// The latest `custom-title`, else the latest `ai-title`.
    title: Option<String>,
    first: String,
}

/// A transcript's names, or `None` for one with no conversation in it. Titles
/// can be anywhere in the file, so it's read whole, once per change.
fn transcript_names(path: &Path, modified: SystemTime, len: u64) -> Option<Names> {
    type Cache = HashMap<PathBuf, (SystemTime, u64, Option<Names>)>;
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some((m, l, names)) = cache.lock().ok()?.get(path)
        && *m == modified
        && *l == len
    {
        return names.clone();
    }
    let names = read_names(path);
    cache.lock().ok()?.insert(path.to_path_buf(), (modified, len, names.clone()));
    names
}

fn read_names(path: &Path) -> Option<Names> {
    let file = std::fs::File::open(path).ok()?;
    let (mut first, mut custom, mut ai) = (None, None, None);
    for line in BufReader::new(file).lines() {
        let Ok(line) = line else { break };
        if line.contains(r#""type":"custom-title""#) {
            let v: Value = serde_json::from_str(&line).unwrap_or_default();
            custom = v["customTitle"].as_str().map(str::to_string).filter(|t| !t.trim().is_empty()).or(custom);
        } else if line.contains(r#""type":"ai-title""#) {
            let v: Value = serde_json::from_str(&line).unwrap_or_default();
            ai = v["aiTitle"].as_str().map(str::to_string).filter(|t| !t.trim().is_empty()).or(ai);
        } else if first.is_none() && line.contains(r#""type":"user""#) {
            first = first_message(&line);
        }
    }
    Some(Names { title: custom.or(ai), first: first? })
}

/// The text of a transcript line if it's a real user message (not a tool
/// result, a meta line or a slash-command wrapper).
pub(crate) fn first_message(line: &str) -> Option<String> {
    {
        let Ok(v) = serde_json::from_str::<Value>(line) else { return None };
        if v["type"] != "user" || v["isMeta"] == true || v["isSidechain"] == true {
            return None;
        }
        let text = match &v["message"]["content"] {
            Value::String(t) => t.clone(),
            Value::Array(blocks) => blocks.iter().filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join(" "),
            _ => return None,
        };
        let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
        // Skip slash-command and system wrappers like `<command-name>`.
        if text.is_empty() || text.starts_with('<') {
            return None;
        }
        Some(text)
    }
}

/// Deletes a session's transcript, and the folder Claude Code keeps beside it if there is one.
pub fn delete_session(cwd: &Path, id: &str) -> Result<(), String> {
    let path = find_transcript(cwd, id).ok_or("transcript not found")?;
    std::fs::remove_file(&path).map_err(|e| format!("couldn't delete {}: {e}", path.display()))?;
    let _ = std::fs::remove_dir_all(path.with_extension(""));
    Ok(())
}

/// One line for a tool call: Claude's own `description` of it when there is one
/// (Bash, PowerShell, Agent…), else the most telling argument.
fn tool_summary(name: &str, input: &Value) -> String {
    let field = match name {
        _ if input["description"].as_str().is_some_and(|d| !d.trim().is_empty()) => "description",
        "Bash" | "PowerShell" => "command",
        "Read" | "Write" | "Edit" | "NotebookEdit" => "file_path",
        "Glob" | "Grep" => "pattern",
        "WebFetch" => "url",
        "WebSearch" => "query",
        "Task" | "Agent" => "description",
        // MCP calls read as what they do: "Click “Sign in”".
        _ => return crate::mcp::describe(name, input).map(|(_, target)| target).unwrap_or_default(),
    };
    let s = input[field].as_str().unwrap_or("");
    let first = s.lines().next().unwrap_or("");
    if first.chars().count() > 120 {
        format!("{}…", first.chars().take(120).collect::<String>())
    } else {
        first.into()
    }
}

fn tool_result_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts.iter().filter_map(|p| p["text"].as_str()).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn result_recaps_the_turn() {
        let mut s = Session::new(Kind::Chat, std::env::temp_dir());
        s.items.push(Item::User { text: "old".into(), images: Vec::new() });
        s.handle(json!({ "type": "assistant", "message": { "content": [
            { "type": "tool_use", "name": "Edit", "input": { "file_path": "C:/x/old.rs" } }
        ] } }));
        s.items.push(Item::User { text: "go".into(), images: Vec::new() });
        s.handle(json!({ "type": "assistant", "message": { "content": [
            { "type": "tool_use", "name": "Read", "input": { "file_path": "C:/x/a.rs" } },
            { "type": "tool_use", "name": "Edit", "input": { "file_path": "C:/x/a.rs" } },
            { "type": "tool_use", "name": "Write", "input": { "file_path": "/y/b.md" } },
            { "type": "tool_use", "name": "Edit", "input": { "file_path": "C:/x/a.rs" } }
        ] } }));
        s.busy = true;
        s.handle(json!({ "type": "result", "duration_ms": 5000, "total_cost_usd": 0.1 }));
        assert!(!s.busy && s.finished);
        let Some(Item::Result { tools, files, .. }) = s.items.last() else { panic!("no result") };
        assert_eq!(*tools, 4);
        assert_eq!(files, &["a.rs", "b.md"]);
    }

    #[test]
    fn wants_you_after_a_turn_or_on_a_prompt() {
        let mut s = Session::new(Kind::Chat, std::env::temp_dir());
        assert!(!s.wants_you());
        s.busy = true;
        s.handle(json!({ "type": "result", "duration_ms": 10, "total_cost_usd": 0.0 }));
        assert!(s.wants_you());
        s.finished = false;
        s.items.push(Item::Permission {
            request_id: "r".into(),
            tool: "Bash".into(),
            input: Value::Null,
            suggestions: Value::Null,
            description: String::new(),
            state: PermState::Pending,
        });
        assert!(s.wants_you());
    }

    #[test]
    fn prompts_are_cancelled_when_their_request_or_process_goes() {
        let mut s = Session::new(Kind::Chat, std::env::temp_dir());
        let ask = |id: &str| {
            json!({ "type": "control_request", "request_id": id,
                "request": { "subtype": "can_use_tool", "tool_name": "Bash", "input": {} } })
        };
        s.handle(ask("a"));
        s.handle(ask("b"));
        s.handle(json!({ "type": "control_cancel_request", "request_id": "a" }));
        let states = |s: &Session| -> Vec<PermState> {
            s.items
                .iter()
                .filter_map(|i| match i {
                    Item::Permission { state, .. } => Some(state.clone()),
                    _ => None,
                })
                .collect()
        };
        assert!(states(&s) == [PermState::Denied("cancelled".into()), PermState::Pending]);
        assert!(s.waiting());
        s.kill();
        assert!(!s.waiting());
        // An answer after the fact changes nothing.
        assert!(s.answer_permission(1, Answer::Once).is_empty());
        assert!(states(&s)[1] == PermState::Denied("cancelled".into()));
    }

    #[test]
    fn mcp_servers_and_server_wide_allow() {
        let mut s = Session::new(Kind::Chat, std::env::temp_dir());
        s.handle(json!({ "type": "system", "subtype": "init",
            "mcp_servers": [{ "name": "playwright-edge", "status": "failed" }] }));
        assert_eq!(s.mcp[0].status, crate::mcp::Status::Failed);
        s.handle(json!({ "type": "assistant", "message": { "content": [{ "type": "tool_use", "id": "t",
            "name": "mcp__playwright-edge__browser_click", "input": { "element": "Sign in" } }] } }));
        let Some(Item::ToolUse { summary, .. }) = s.items.last() else { panic!("no call") };
        assert_eq!(summary, "\u{201c}Sign in\u{201d}");
        s.handle(json!({ "type": "control_request", "request_id": "r", "request": {
            "subtype": "can_use_tool", "tool_name": "mcp__playwright-edge__browser_click", "input": {} } }));
        s.answer_permission(1, Answer::Server);
        let Some(Item::Permission { state, .. }) = s.items.last() else { panic!("no prompt") };
        assert!(*state == PermState::Allowed("all Edge actions allowed this session".into()));
    }

    #[test]
    fn keeps_what_the_process_reports_at_start() {
        let mut s = Session::new(Kind::Chat, std::env::temp_dir());
        // Other control responses (an interrupt's, say) aren't the handshake.
        s.handle(json!({ "type": "control_response", "response": { "request_id": "kynex-1", "response": {} } }));
        assert!(s.take_init().is_none());
        s.handle(json!({ "type": "control_response", "response": { "subtype": "success", "request_id": INIT_ID,
            "response": {
                "commands": [{ "name": "compact", "description": "Free up context" }],
                "models": [{ "value": "opus", "displayName": "Opus", "supportedEffortLevels": ["high"] }],
            } } }));
        let init = s.take_init().unwrap();
        assert_eq!(init.commands[0].name, "compact");
        assert_eq!(init.models[0].value, "opus");
        assert_eq!(init.models[0].efforts, ["high"]);
        assert!(s.take_init().is_none(), "taken once");
        assert!(s.items.is_empty(), "nothing in the transcript");
    }

    #[test]
    fn context_use_follows_main_thread_replies() {
        let mut s = Session::new(Kind::Chat, std::env::temp_dir());
        let reply = |parent: Value, input: u64| {
            json!({ "type": "assistant", "parent_tool_use_id": parent, "message": {
                "model": "test-model-ctx", "content": [],
                "usage": { "input_tokens": input, "cache_creation_input_tokens": 1000, "cache_read_input_tokens": 9000 }
            } })
        };
        s.handle(reply(Value::Null, 0));
        assert_eq!(s.context, ContextUse { tokens: 10_000, window: None });
        // A subagent's reply doesn't move the meter.
        s.handle(reply(json!("toolu_1"), 500_000));
        assert_eq!(s.context.tokens, 10_000);
        s.handle(json!({ "type": "result", "modelUsage": { "test-model-ctx": { "contextWindow": 200_000 } } }));
        assert_eq!(s.context.window, Some(200_000));
        assert_eq!(s.context.fraction(), Some(0.05));
        // Another session on the same model knows the window from its first reply.
        let mut other = Session::new(Kind::Chat, std::env::temp_dir());
        other.handle(reply(Value::Null, 10_000));
        assert_eq!(other.context, ContextUse { tokens: 20_000, window: Some(200_000) });
        other.handle(json!({ "type": "system", "subtype": "compact_boundary",
            "compact_metadata": { "pre_tokens": 20_000, "post_tokens": 3_000 } }));
        assert_eq!(other.context.tokens, 3_000);
        // A synthetic reply leaves the meter alone; a compaction without a size blanks it.
        other.handle(json!({ "type": "assistant", "parent_tool_use_id": null, "message": {
            "model": "<synthetic>", "content": [], "usage": { "input_tokens": 0 } } }));
        assert_eq!(other.context, ContextUse { tokens: 3_000, window: Some(200_000) });
        other.handle(json!({ "type": "system", "subtype": "compact_boundary" }));
        assert_eq!(other.context.tokens, 0);
        // An unknown reply model takes the biggest window the turn reported.
        let mut third = Session::new(Kind::Chat, std::env::temp_dir());
        third.handle(json!({ "type": "result", "modelUsage": {
            "side-model-ctx": { "contextWindow": 100_000 }, "main-model-ctx[1m]": { "contextWindow": 1_000_000 } } }));
        assert_eq!(third.context.window, Some(1_000_000));
    }

    #[test]
    fn replay_leaves_out_what_the_cli_wrote() {
        let mut s = Session::new(Kind::Chat, std::env::temp_dir());
        let note = "<task-notification>\n<task-id>b1</task-id>\n<status>completed</status>\n</task-notification>";
        s.replay_entry(json!({ "type": "user", "message": { "role": "user", "content": "check CI" } }));
        s.replay_entry(json!({ "type": "user", "promptSource": "system",
            "origin": { "kind": "task-notification" }, "message": { "role": "user", "content": note } }));
        // Older logs have no `promptSource` or `origin`.
        s.replay_entry(json!({ "type": "user", "message": { "role": "user", "content": note } }));
        let said: Vec<_> = s
            .items
            .iter()
            .filter_map(|i| if let Item::User { text, .. } = i { Some(text.as_str()) } else { None })
            .collect();
        assert_eq!(said, ["check CI"]);
        // Each notification is a row of its own instead.
        assert_eq!(s.items.iter().filter(|i| matches!(i, Item::Task(n) if n.task_id == "b1")).count(), 2);
    }

    #[test]
    fn a_background_task_ending_wants_you() {
        let mut s = Session::new(Kind::Code, std::env::temp_dir());
        s.handle(json!({ "type": "system", "subtype": "task_started", "task_id": "b1", "tool_use_id": "toolu_1",
            "description": "Watch CI", "task_type": "local_bash" }));
        s.handle(json!({ "type": "user", "message": { "content": [{ "type": "tool_result", "tool_use_id": "toolu_1",
            "content": "Command running in background with ID: b1. Output is being written to: /tmp/b1.output. You will be notified when it completes." }] } }));
        assert!(s.has_tasks() && !s.wants_you());
        assert_eq!(s.tasks.running[0].output_file, Some(PathBuf::from("/tmp/b1.output")));
        s.handle(json!({ "type": "system", "subtype": "task_notification", "task_id": "b1", "tool_use_id": "toolu_1",
            "status": "completed", "output_file": "/tmp/b1.output", "summary": "Background command \"Watch CI\" completed (exit code 0)" }));
        assert!(!s.has_tasks() && s.wants_you());
        assert!(matches!(s.items.last(), Some(Item::Task(n)) if n.status == tasks::Status::Completed));
    }

    #[test]
    fn images_in_logs_and_tool_results_get_previews() {
        // Previews here say what they were made from.
        set_preview_maker(|mt, bytes| Some(Arc::new(format!("{mt} {}", bytes.len()))));
        let said = |p: &Preview| p.clone().downcast::<String>().unwrap().to_string();
        let image = |data: &str| json!({ "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": data } });
        let mut s = Session::new(Kind::Chat, std::env::temp_dir());
        // "AAAA" is three bytes; "!" isn't base64, so that one is only counted.
        s.replay_user(&json!([image("AAAA"), image("!"), { "type": "text", "text": "see" }]));
        let Some(Item::User { text, images }) = s.items.last() else { panic!() };
        assert_eq!(text, "[1 image(s)] see");
        assert_eq!(images.iter().map(said).collect::<Vec<_>>(), ["image/png 3"]);

        s.handle(json!({ "type": "user", "message": { "content": [{ "type": "tool_result", "tool_use_id": "t",
            "content": [{ "type": "text", "text": "shot" }, image("AAAAAAAA")] }] } }));
        let Some(Item::ToolResult { content, images, .. }) = s.items.last() else { panic!() };
        assert_eq!(content, "shot");
        assert_eq!(images.iter().map(said).collect::<Vec<_>>(), ["image/png 6"]);
    }

    #[test]
    fn title_is_first_real_user_message() {
        let path = std::env::temp_dir().join(format!("kynex-title-{}.jsonl", std::process::id()));
        let lines = [
            r#"{"type":"queue-operation","operation":"enqueue"}"#,
            r#"{"type":"user","isMeta":true,"message":{"role":"user","content":"<local-command-caveat>x</local-command-caveat>"}}"#,
            r#"{"type":"user","message":{"role":"user","content":"<command-name>/clear</command-name>"}}"#,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"image","source":{}},{"type":"text","text":"fix the\n  build"}]}}"#,
            r#"{"type":"user","message":{"role":"user","content":"second message"}}"#,
        ];
        std::fs::write(&path, lines.join("\n")).unwrap();
        let names = read_names(&path).unwrap();
        assert_eq!(names.first, "fix the build");
        assert_eq!(names.title, None);
        std::fs::write(&path, lines[..3].join("\n")).unwrap();
        assert!(read_names(&path).is_none());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn titles_from_claude_and_renames() {
        let path = std::env::temp_dir().join(format!("kynex-names-{}.jsonl", std::process::id()));
        let mut lines = vec![
            r#"{"type":"user","message":{"role":"user","content":"clean up the worktrees please"}}"#,
            r#"{"type":"ai-title","aiTitle":"Git worktree cleanup","sessionId":"x"}"#,
        ];
        std::fs::write(&path, lines.join("\n")).unwrap();
        let names = read_names(&path).unwrap();
        assert_eq!(names.title.as_deref(), Some("Git worktree cleanup"));
        assert_eq!(names.first, "clean up the worktrees please");
        // A rename wins over Claude's own title, wherever it sits.
        lines.insert(1, r#"{"type":"custom-title","customTitle":"Worktrees","sessionId":"x"}"#);
        std::fs::write(&path, lines.join("\n")).unwrap();
        assert_eq!(read_names(&path).unwrap().title.as_deref(), Some("Worktrees"));
        let _ = std::fs::remove_file(path);
    }

    /// Lists the sessions Claude Code has for this repo (or `KYNEX_SESSIONS_DIR`).
    /// Run with `cargo test -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn lists_real_sessions() {
        let dir =
            std::env::var_os("KYNEX_SESSIONS_DIR").map_or_else(|| std::env::current_dir().unwrap(), PathBuf::from);
        let sessions = list_sessions(&dir);
        for s in &sessions {
            println!("{}  {:?}  {}", s.id, s.modified, s.title.chars().take(60).collect::<String>());
        }
        assert!(!sessions.is_empty());
    }

    /// Talks to the real `claude` CLI, so it uses your login and a little quota.
    /// Run with `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn chat_round_trip() {
        let mut s = Session::new(Kind::Chat, std::env::temp_dir());
        s.model = "haiku".into();
        s.send("Reply with exactly: pong".into(), Vec::new());
        let start = Instant::now();
        while s.busy && start.elapsed() < Duration::from_secs(90) {
            s.poll();
            std::thread::sleep(Duration::from_millis(50));
        }
        let text: String =
            s.items.iter().filter_map(|i| if let Item::Assistant(t) = i { Some(t.as_str()) } else { None }).collect();
        assert!(text.to_lowercase().contains("pong"), "got: {text:?}");
        assert!(s.items.iter().any(|i| matches!(i, Item::Result { is_error: false, .. })));
        assert!(s.session_id.is_some());
        // The process answered the `initialize` it starts with, before the reply.
        let init = s.take_init().expect("no initialize response");
        assert!(init.commands.iter().any(|c| c.name == "compact"));
        assert!(!init.models.is_empty());
    }

    /// Two turns, the second writing a file; rewinding to it restores the file
    /// and the conversation, and the next message carries on from the first
    /// turn. Run with `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn rewind_live() {
        let root = std::env::temp_dir().join(format!("kynex-rewind-{}", std::process::id()));
        let project = root.join("project");
        std::fs::create_dir_all(&project).unwrap();
        unsafe { std::env::set_var("KYNEX_DATA_DIR", root.join("data")) };
        let mut s = Session::new(Kind::Code, project.clone());
        s.model = "haiku".into();
        s.mode = project::Mode::AcceptEdits;
        let turn = |s: &mut Session, text: &str| {
            s.send(text.into(), Vec::new());
            let start = Instant::now();
            while s.busy && start.elapsed() < Duration::from_secs(120) {
                s.poll();
                std::thread::sleep(Duration::from_millis(50));
            }
            s.items
                .iter()
                .rev()
                .find_map(|i| if let Item::Assistant(t) = i { Some(t.clone()) } else { None })
                .unwrap_or_default()
        };
        turn(&mut s, "Remember the word APPLE. Reply only OK.");
        turn(&mut s, "Also remember BANANA, and write BANANA to fruit.txt with the Write tool. Reply only OK.");
        assert!(project.join("fruit.txt").is_file(), "the second turn wrote no file");

        let id = s.session_id.clone().unwrap();
        let points = crate::rewind::points(&project, &id).unwrap();
        assert_eq!(points.len(), 2);
        let second = &points[0];
        assert!(second.checkpoint && second.changed == ["fruit.txt"], "unexpected rewind checkpoint metadata");
        s.kill();
        crate::rewind::restore_files(&project, &id, second).unwrap();
        assert!(!project.join("fruit.txt").exists(), "the file is still there");
        s.rewind_to(second);
        assert_eq!(s.items.iter().filter(|i| matches!(i, Item::User { .. })).count(), 1);

        let reply = turn(&mut s, "List every word I asked you to remember, nothing else.");
        assert!(reply.contains("APPLE") && !reply.contains("BANANA"), "got: {reply:?}");
        assert_eq!(s.session_id.as_deref(), Some(id.as_str()));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A project session starts in Auto (no prompt for a harmless mkdir), then a
    /// live switch to Manual makes the next one ask. Run with `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn permission_modes_live() {
        let root = std::env::temp_dir().join(format!("kynex-modes-{}", std::process::id()));
        let project = root.join("project");
        std::fs::create_dir_all(&project).unwrap();
        // Keep the project's Kynex settings out of the real data dir.
        unsafe { std::env::set_var("KYNEX_DATA_DIR", root.join("data")) };

        let mut s = Session::new(Kind::Code, project.clone());
        assert_eq!(s.mode, project::Mode::Auto);
        s.model = "haiku".into();
        let run = |s: &mut Session, dir: &str| {
            s.send(format!("Run this exact command with the Bash tool: mkdir {dir} . Then reply OK."), Vec::new());
            let start = Instant::now();
            while s.busy && start.elapsed() < Duration::from_secs(120) {
                s.poll();
                if let Some(i) =
                    s.items.iter().position(|i| matches!(i, Item::Permission { state: PermState::Pending, .. }))
                {
                    s.answer_permission(i, Answer::Deny(String::new()));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        };
        let asked = |s: &Session| s.items.iter().filter(|i| matches!(i, Item::Permission { .. })).count();

        run(&mut s, "made1");
        assert!(project.join("made1").is_dir(), "auto mode should run mkdir");
        assert_eq!(asked(&s), 0, "auto mode shouldn't ask");

        s.set_mode(project::Mode::Manual);
        run(&mut s, "made2");
        assert_eq!(s.mode, project::Mode::Manual);
        assert!(asked(&s) > 0, "manual mode should ask");
        assert!(!project.join("made2").exists(), "denied mkdir shouldn't run");

        s.kill();
        let _ = std::fs::remove_dir_all(&root);
    }

    /// An `AskUserQuestion` answered from Kynex reaches Claude. Run with `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn answers_questions() {
        let mut s = Session::new(Kind::Code, std::env::temp_dir());
        s.model = "haiku".into();
        s.send(
            "Use the AskUserQuestion tool to ask me which color I like, with options Red and Blue. \
             Then reply with exactly the color I picked, in capitals."
                .into(),
            Vec::new(),
        );
        let start = Instant::now();
        while s.busy && start.elapsed() < Duration::from_secs(120) {
            s.poll();
            let pending = s.items.iter().enumerate().find_map(|(i, it)| match it {
                Item::Permission { tool, input, state: PermState::Pending, .. } if tool == "AskUserQuestion" => {
                    Some((i, input["questions"][0]["question"].as_str().unwrap_or("").to_string()))
                }
                _ => None,
            });
            if let Some((i, question)) = pending {
                let mut answers = serde_json::Map::new();
                answers.insert(question, json!("Blue"));
                s.answer_permission(i, Answer::Questions(answers));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let text: String =
            s.items.iter().filter_map(|i| if let Item::Assistant(t) = i { Some(t.as_str()) } else { None }).collect();
        assert!(
            s.items.iter().any(|i| matches!(i, Item::Permission { state: PermState::Allowed(_), .. })),
            "never asked"
        );
        assert!(text.contains("BLUE"), "got: {text:?}");
        s.kill();
    }
}
