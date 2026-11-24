//! Drives a `claude` CLI process over its stream-json protocol.
//!
//! The CLI owns auth (whatever account `claude` is logged into), the agent
//! loop and the tools. Kynex only renders events and answers permission
//! prompts, which arrive as `control_request` messages on stdout.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Child;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::claude::commands::SlashCommand;
use crate::claude::queue::Queue;
use crate::claude::session_log::{LoggedImage, Span};
use crate::claude::tasks::{self, Tasks};
use crate::project;

mod answer;
mod composer;
mod handle;
mod launch;
mod models;
mod process;
mod replay;
mod state;
#[cfg(test)]
mod tests;
mod turns;

pub(crate) use handle::written_by_cli;
pub(crate) use models::initialize;
use models::models_from_init;
pub use models::{ModelInfo, list_models, save_models, saved_models};
pub(crate) use process::claude_command;
use process::spawn_readers;
use replay::Loader;

/// How long after a failed start warming up leaves it alone; a message still tries.
const START_BACKOFF: Duration = Duration::from_secs(30);

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
    crate::claude::attach::images_in(content).into_iter().filter_map(|i| make(&i.media_type, i.bytes)).collect()
}

/// The images among `content`'s blocks (at JSON pointer `at` in a line of a
/// log) as [`LoggedImage`]s: where they are, and their size, but not their bytes.
fn logged_in(content: &Value, (path, span): &(Arc<PathBuf>, Span), at: &str) -> Vec<Preview> {
    let blocks = content.as_array().into_iter().flatten().enumerate();
    blocks
        .filter(|(_, b)| b["type"] == "image" && b["source"]["type"] == "base64")
        .filter_map(|(i, b)| {
            let data = b["source"]["data"].as_str()?;
            let image = LoggedImage {
                path: path.clone(),
                span: *span,
                pointer: format!("{at}/{i}"),
                media_type: b["source"]["media_type"].as_str()?.to_string(),
                size: crate::claude::attach::base64_dimensions(data),
            };
            Some(Arc::new(image) as Preview)
        })
        .collect()
}

pub struct Attachment {
    pub media_type: String,
    pub base64: String,
    pub preview: Preview,
}

/// A thinking block: its text when the CLI shares it, and how long it took
/// when it was watched live. One read back from a log has no timing.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Thought {
    pub text: String,
    /// When it started streaming in.
    pub started: Option<Instant>,
    /// How long it took, once done.
    pub took: Option<Duration>,
}

impl Thought {
    /// One starting now, from the stream.
    pub fn live() -> Self {
        Self { started: Some(Instant::now()), ..Self::default() }
    }

    /// One read whole, from a finished message or a log.
    pub fn logged(text: &str) -> Self {
        Self { text: text.to_string(), ..Self::default() }
    }

    /// Still streaming in.
    pub fn is_live(&self) -> bool {
        self.started.is_some() && self.took.is_none()
    }

    /// Done: keeps how long it took, and `text` if none streamed in. A second
    /// block straight after joins this one.
    fn finish(&mut self, text: &str) {
        if self.text.is_empty() {
            self.text = text.to_string();
        } else if !text.is_empty() && self.text != text {
            self.text.push_str("\n\n");
            self.text.push_str(text);
        }
        if let (Some(started), None) = (self.started, self.took) {
            self.took = Some(started.elapsed());
        }
    }

    /// "Thinking… 4s" while live, "Thought for 6s" after, or just "Thought"
    /// when its timing isn't known.
    pub fn label(&self) -> String {
        match (self.started, self.took) {
            (Some(started), None) => format!("Thinking… {}s", started.elapsed().as_secs()),
            (_, Some(took)) => format!("Thought for {}", crate::fmt::duration(took.as_millis() as u64)),
            (None, None) => "Thought".into(),
        }
    }
}

/// When a tool call started and how long it took, for one watched live; one
/// read back from a log has neither.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Timing {
    pub started: Option<Instant>,
    pub took: Option<Duration>,
}

impl Timing {
    /// Started and not done yet.
    pub fn is_running(&self) -> bool {
        self.started.is_some() && self.took.is_none()
    }

    /// How long it has run so far, or ran in all.
    pub fn elapsed(&self) -> Option<Duration> {
        self.took.or_else(|| self.started.map(|s| s.elapsed()))
    }

    fn finish(&mut self) {
        if let (Some(started), None) = (self.started, self.took) {
            self.took = Some(started.elapsed());
        }
    }
}

/// How much of a streaming tool call's input is read for its target.
const PARTIAL_INPUT_MAX: usize = 16 * 1024;

/// A tool call's time limit in ms (Bash's `timeout`), from its pretty-printed input.
pub fn timeout_ms(input: &str) -> Option<u64> {
    serde_json::from_str::<Value>(input).ok()?["timeout"].as_u64().filter(|&ms| ms > 0)
}

/// Called from the reader threads whenever the process has news, so the UI drains it.
pub type Wake = Arc<dyn Fn() + Send + Sync>;

/// Claude's todo list as [`Session::todos`] hands it out, shared rather than copied.
pub type TodoList = Arc<Vec<crate::claude::todos::Todo>>;

pub enum Item {
    User {
        text: String,
        images: Vec<Preview>,
    },
    Assistant(String),
    Thinking(Thought),
    ToolUse {
        /// The call's `tool_use_id`, which its result names.
        id: String,
        name: String,
        summary: String,
        input: String,
        /// How long it ran, when watched live.
        timing: Timing,
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
        /// Why the CLI asked, in words (empty if it didn't say); see [`crate::claude::why`].
        why: String,
        state: PermState,
    },
    /// The end of a turn, with a recap of it: how many tools ran and which files changed.
    Result {
        cost: f64,
        duration_ms: u64,
        is_error: bool,
        tools: usize,
        /// The files it edited, as the tools were given them (usually absolute).
        files: Vec<String>,
    },
    /// A background task ended (or a Monitor saw something); Claude's turn about it follows.
    Task(tasks::Notice),
    /// A `!` command the user ran, and its output; Claude sees it with the next message.
    Shell(crate::claude::input::shell::Run),
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
    /// Writing to its stdin failed.
    Lost,
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
    /// The folder whose Kynex settings (permission rules, mode, effort, extra folders…)
    /// it runs under, when that isn't `cwd`: a session in a worktree or another folder of
    /// a project runs under the project's, not under settings of its own folder.
    pub settings_dir: Option<PathBuf>,
    /// The model its process starts on (`--model`); empty is the CLI's default.
    pub model: String,
    /// A model pinned to this session in the picker, over its project's.
    pub own_model: Option<String>,
    /// Who answers it; see [`Self::set_provider`].
    provider: crate::provider::Launch,
    pub items: Vec<Item>,
    pub session_id: Option<String>,
    pub busy: bool,
    /// A turn ended and the user hasn't looked at it yet; the UI clears it.
    pub finished: bool,
    /// What the session has cost: [`Self::carried_cost`] plus this process's own counter.
    pub total_cost: f64,
    /// Cost of the processes before the current one. A resumed or restarted
    /// `claude` starts its `total_cost_usd` over from 0, so the legs add up.
    carried_cost: f64,
    /// The current process's `total_cost_usd`, as of its last result.
    run_cost: f64,
    /// Tokens and cost of every turn this session object has seen, subagents included.
    pub spent: crate::claude::spent::Spent,
    pub context: ContextUse,
    /// The live permission mode: starts at the project's default, follows the
    /// CLI's reports, and is switched from the composer without a restart.
    pub mode: project::Mode,
    /// Messages written while Claude works; the next goes when a turn ends.
    pub queue: Queue,
    /// The queue stays put when a turn ends: the session is about to move to another
    /// folder, where it goes instead.
    pub hold_queue: bool,
    /// Reopened from a past conversation ([`Self::resume`]), not started here.
    reopened: bool,
    /// `session_id` names a conversation the CLI has a log of: reopened, or
    /// sent a message. A process warmed up early reports an id before that,
    /// which a restart mustn't `--resume` ("No conversation found").
    conversation: bool,
    /// Background tasks running in this session's process.
    pub tasks: Tasks,
    /// The subagents of the turn in progress, each call's input parsed once.
    pub agents: crate::claude::subagents::Agents,
    /// The MCP servers its process reported when it started, and how they're doing.
    pub mcp: Vec<crate::claude::mcp::Server>,
    /// The tools each of those servers offers, by server name.
    pub mcp_tools: std::collections::BTreeMap<String, Vec<String>>,
    /// Its running process has driven a browser, so its browser server holds a
    /// pairing (and tabs) that stopping the process would lose.
    pub browser: bool,
    /// Its process runs on project settings it couldn't take live; it starts
    /// again once idle ([`Self::apply_settings`]).
    pub stale: bool,
    /// Thinking is switched off for it ([`Self::set_thinking`]).
    pub thinking_off: bool,
    /// What its process said about the `ultrathink` keyword.
    pub ultrathink: crate::claude::input::thinking::Ultrathink,
    /// Its process was started allowed to bypass permissions.
    bypass: bool,
    /// `set_permission_mode` requests awaiting an answer, with the mode to go back to.
    mode_requests: Vec<(String, project::Mode)>,
    /// Set by the UI so it hears about new events; a no-op until then.
    pub wake: Wake,
    /// Variables its process gets on top (the app's pane, session and control address);
    /// when set, they stand in for the session's own `knx` variables.
    pub extra_env: Vec<(String, String)>,
    child: Option<Child>,
    /// Lines for its stdin, which a thread writes ([`launch`]): a big message
    /// to a process that isn't reading yet would block the UI otherwise.
    stdin: Option<Sender<String>>,
    rx: Option<Receiver<Event>>,
    /// A start running on a thread ([`launch::begin`]), and the lines written
    /// to the process until it's up.
    starting: Option<Receiver<launch::Started>>,
    outbox: Vec<String>,
    /// When a start last failed, so warming up doesn't retry it on every keystroke.
    failed_at: Option<Instant>,
    streaming_text: bool,
    /// When the process started, whether it has said anything yet, and whether
    /// it has been sent a message (or was only warmed up).
    spawned: Option<Instant>,
    ready: bool,
    talked: bool,
    /// A content block (text, thinking, a tool call's input) is streaming in.
    in_block: bool,
    /// Something of Claude's reply came since the last message went.
    answered: bool,
    /// When the transcript last stopped showing live progress, for [`Self::pulse`].
    quiet_since: Instant,
    /// When it last did or was asked anything, for [`Self::sleep_if_idle`].
    active_at: Instant,
    /// The input of the tool call streaming in, until its summary can be read from it.
    tool_input: String,
    /// Goes up when an item changes in a way that changes the transcript's layout.
    revision: u64,
    /// Goes up when a permission prompt changes state; with the item count it keys `waiting_cache`.
    perm_gen: u64,
    /// `waiting()` as last scanned: (item count, `perm_gen`, answer). It is asked every frame.
    waiting_cache: std::cell::Cell<Option<(usize, u64, bool)>>,
    /// `counts()` as last scanned: (item count, revision, turns, subagents). Asked every frame.
    counts_cache: std::cell::Cell<Option<(usize, u64, usize, usize)>>,
    /// `wants_browser()` as last worked out: (item count, revision, answer). Asked every frame.
    browser_cache: std::cell::Cell<Option<(usize, u64, bool)>>,
    /// `todos()` as last read: (item count, revision, the list). Asked every frame.
    todos_cache: std::cell::RefCell<Option<(usize, u64, TodoList)>>,
    next_request: u64,
    /// `mcp_reconnect` and `mcp_status` requests awaiting an answer.
    mcp_requests: Vec<String>,
    /// `stop_task` requests awaiting an answer: request id → task id.
    stopping: HashMap<String, String>,
    /// The project settings the running process got, and the `apply_flag_settings`
    /// requests awaiting an answer.
    launched: Option<project::Launched>,
    applying: Vec<String>,
    /// The model that wrote the last reply, whose window `context` is measured against.
    reply_model: String,
    /// An OpenRouter model that turned out to take no tool calls (its request
    /// failed for it), until the app takes it.
    no_tools: Option<String>,
    /// What the process reported when it started, until the app takes it.
    init: Option<Init>,
    /// The account's usage limits from the last `rate_limit_event`, until the app takes them.
    limits: Option<crate::claude::usage::Limits>,
    /// The log entry a rewind went back to, which the next process continues from.
    resume_at: Option<String>,
    /// A fork's original ([`Self::fork`]): its process continues that one as a new
    /// session, until the first message is sent and the new one has its own log.
    fork_from: Option<String>,
    /// A message was written into the turn in progress ([`Self::steer`]).
    steered: bool,
    /// The last turn ended with a steer written into it, which the CLI may not have
    /// read in time: if it starts a turn of its own for it, that turn is busy too.
    late_steer: bool,
    /// Counts processes started, so [`Self::poll`] drops what an old one sent
    /// once a new one has taken its place.
    runs: u64,
    /// The log being read back, on [`Self::resume`] or a rewind.
    loader: Option<Loader>,
    /// While replaying: the log and line the entry came from, so its images stay there.
    origin: Option<(Arc<PathBuf>, Span)>,
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
        let settings = match kind {
            Kind::Code => project::ClaudeSettings::load(&cwd),
            Kind::Chat => project::ClaudeSettings::default(),
        };
        let mode = settings.mode;
        static NEXT_UID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self {
            uid: NEXT_UID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            kind,
            cwd,
            settings_dir: None,
            model: String::new(),
            own_model: None,
            provider: Default::default(),
            items: Vec::new(),
            session_id: None,
            busy: false,
            finished: false,
            total_cost: 0.0,
            carried_cost: 0.0,
            run_cost: 0.0,
            spent: Default::default(),
            context: ContextUse::default(),
            mode,
            queue: Queue::default(),
            hold_queue: false,
            reopened: false,
            conversation: false,
            tasks: Tasks::default(),
            agents: Default::default(),
            mcp: Vec::new(),
            mcp_tools: Default::default(),
            browser: false,
            stale: false,
            thinking_off: settings.thinking_off,
            ultrathink: Default::default(),
            bypass: false,
            mode_requests: Vec::new(),
            wake: Arc::new(|| {}),
            extra_env: Vec::new(),
            child: None,
            stdin: None,
            rx: None,
            starting: None,
            outbox: Vec::new(),
            failed_at: None,
            streaming_text: false,
            spawned: None,
            ready: false,
            talked: false,
            in_block: false,
            answered: false,
            quiet_since: Instant::now(),
            active_at: Instant::now(),
            tool_input: String::new(),
            revision: 0,
            perm_gen: 0,
            waiting_cache: std::cell::Cell::new(None),
            counts_cache: std::cell::Cell::new(None),
            browser_cache: std::cell::Cell::new(None),
            todos_cache: Default::default(),
            next_request: 0,
            stopping: HashMap::new(),
            mcp_requests: Vec::new(),
            launched: None,
            applying: Vec::new(),
            reply_model: String::new(),
            no_tools: None,
            init: None,
            limits: None,
            resume_at: None,
            fork_from: None,
            steered: false,
            late_steer: false,
            runs: 0,
            loader: None,
            origin: None,
        }
    }
}
