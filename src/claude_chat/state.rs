//! View-model structs, configuration, events, and small formatting helpers for
//! the native Claude Code chat & composer pane (`ClaudeChatView`).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use gpui::RenderImage;
use kynex_core::agent_panes::AgentState;
use kynex_core::claude::composer::{CommandMatch, FileMatch};
use kynex_core::claude::diff::{EditDiff, diff_from_tool_input};
use kynex_core::claude::history::{HistoryItem, LoggedImageRef};
use kynex_core::claude::markdown::{self, Block};
use kynex_core::claude::{AskQuestion, PromptBlock, SubagentMessage};
use kynex_core::provider::Launch;
use serde_json::Value;

/// Configuration required to instantiate a [`super::ClaudeChatView`].
#[derive(Clone, Debug)]
pub struct ClaudeChatConfig {
    pub workspace: PathBuf,
    pub provider: String,
    pub model: Option<String>,
    pub resume_session_id: Option<String>,
    pub fork_from: Option<String>,
    pub launch: Launch,
    pub openrouter_models: Vec<String>,
    pub extra_env: Vec<(String, String)>,
}

/// Events emitted by [`super::ClaudeChatView`] to `KynexApp` (`src/app/claude_chat.rs`).
#[derive(Clone, Debug)]
pub enum ClaudeChatEvent {
    StatusChanged { state: AgentState, detail: String },
    SessionId(String),
    TitleChanged(String),
    TurnFinished,
    ForkRequested { from_session_id: String },
    RunInTerminal(String),
    OpenFile { path: PathBuf, line: Option<u32> },
}

/// Reference key for an image rendered inside a transcript row.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ImageKey {
    /// Eagerly decoded inline image (pasted by user or returned by a tool).
    Inline(u64),
    /// Lazily decoded image stored inside a `.jsonl` transcript file on disk.
    Logged(LoggedKey),
}

/// Hashable key for a [`LoggedImageRef`] inside [`super::media::ImageCache`].
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct LoggedKey {
    pub path: PathBuf,
    pub line_offset: u64,
    pub block_index: usize,
}

impl From<&LoggedImageRef> for LoggedKey {
    fn from(r: &LoggedImageRef) -> Self {
        Self { path: r.path.clone(), line_offset: r.line_offset, block_index: r.block_index }
    }
}

/// Image thumbnail attached to a transcript row (either inline or lazy `.jsonl` ref).
#[derive(Clone, Debug)]
pub struct RowImage {
    pub key: ImageKey,
    pub logged_ref: Option<LoggedImageRef>,
    pub label: String,
}

/// One pending image attachment queued in the composer before submit.
#[derive(Clone)]
pub struct ComposerAttachment {
    pub id: u64,
    pub media_type: String,
    pub base64: String,
    pub size: (u32, u32),
    pub thumb: Arc<RenderImage>,
    pub full: Option<Arc<RenderImage>>,
    pub label: String,
}

/// Kind of read/search call grouped into a [`LogRow::ReadSearchRun`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadKind {
    Read,
    Search,
    Fetch,
}

/// One entry inside a collapsed/expanded [`LogRow::ReadSearchRun`].
#[derive(Clone, Debug)]
pub struct ReadSearchEntry {
    pub tool_use_id: String,
    pub kind: ReadKind,
    pub tool_name: String,
    pub label: String,
    pub path: Option<String>,
    pub done: bool,
    pub is_error: bool,
}

/// One virtualized row inside the transcript `gpui::list`.
#[derive(Clone, Debug)]
pub enum LogRow {
    UserPrompt {
        id: u64,
        uuid: Option<String>,
        text: String,
        images: Vec<RowImage>,
        file_chips: Vec<String>,
        paste_labels: Vec<String>,
    },
    Thought {
        id: u64,
        text: String,
        started_at: Option<Instant>,
        duration_ms: Option<u64>,
        expanded: bool,
        streaming: bool,
    },
    ReadSearchRun {
        id: u64,
        entries: Vec<ReadSearchEntry>,
        expanded: bool,
    },
    EditRow {
        id: u64,
        tool_use_id: String,
        tool_name: String,
        path: String,
        diff: EditDiff,
        expanded: bool,
        done: bool,
        is_error: bool,
        error_text: Option<String>,
    },
    ToolCall {
        id: u64,
        tool_use_id: String,
        name: String,
        summary: String,
        command: Option<String>,
        input_preview: String,
        output: String,
        images: Vec<RowImage>,
        started_at: Option<Instant>,
        duration_ms: Option<u64>,
        done: bool,
        is_error: bool,
        expanded: bool,
    },
    Prose {
        id: u64,
        raw: String,
        blocks: Vec<Block>,
        plain: String,
        streaming: bool,
    },
    CompactionDivider {
        detail: String,
    },
    Notice {
        text: String,
        is_error: bool,
    },
}

impl LogRow {
    /// Searchable plain text for `Ctrl+F` transcript find.
    pub fn searchable_text(&self) -> String {
        match self {
            Self::UserPrompt { text, file_chips, .. } => {
                if file_chips.is_empty() {
                    text.clone()
                } else {
                    format!("{text} {}", file_chips.join(" "))
                }
            }
            Self::Thought { text, .. } => text.clone(),
            Self::ReadSearchRun { entries, .. } => {
                entries.iter().map(|e| e.label.as_str()).collect::<Vec<_>>().join(" ")
            }
            Self::EditRow { path, diff, .. } => {
                let mut out = path.clone();
                for h in &diff.hunks {
                    for l in &h.lines {
                        out.push('\n');
                        out.push_str(&l.text);
                    }
                }
                out
            }
            Self::ToolCall { name, summary, output, .. } => format!("{name} {summary}\n{output}"),
            Self::Prose { plain, .. } => plain.clone(),
            Self::CompactionDivider { detail, .. } => detail.clone(),
            Self::Notice { text, .. } => text.clone(),
        }
    }

    pub fn is_user_turn(&self) -> bool {
        matches!(self, Self::UserPrompt { .. })
    }
}

/// One checklist item shown in the pinned Plan / Todo strip above the composer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanStep {
    pub content: String,
    pub status: String,
    pub active_form: Option<String>,
}

/// One forwarded row inside a subagent's live peek transcript.
#[derive(Clone, Debug)]
pub enum SubagentRow {
    Thought(String),
    /// Raw Markdown plus its parse, kept in step so the peek modal never re-parses per frame.
    Prose {
        text: String,
        blocks: Vec<Block>,
    },
    Tool {
        id: String,
        name: String,
        summary: String,
        output: Option<String>,
        is_error: bool,
    },
}

/// Live state for one active or recent subagent (`Agent` / `Task` tool call).
#[derive(Clone, Debug)]
pub struct SubagentTrack {
    pub tool_use_id: String,
    pub task_id: Option<String>,
    pub label: String,
    pub last_tool: Option<String>,
    pub summary: Option<String>,
    pub running: bool,
    pub status: String,
    pub rows: Vec<SubagentRow>,
}

/// Interactive docked amber card awaiting user input above the composer.
#[derive(Clone, Debug)]
pub enum PendingCard {
    Permission {
        req_id: String,
        tool_name: String,
        input: Value,
        suggestions: Vec<Value>,
        why: Option<String>,
        blocked_path: Option<String>,
        description: Option<String>,
        diff_preview: Option<EditDiff>,
        show_comment: bool,
    },
    AskUserQuestion {
        req_id: String,
        questions: Vec<AskQuestion>,
        active_idx: usize,
        selected: Vec<Vec<usize>>,
        other_active: Vec<bool>,
        other_text: Vec<String>,
    },
    ExitPlanMode {
        req_id: String,
        plan_blocks: Vec<Block>,
        plan_path: Option<String>,
        show_feedback: bool,
    },
}

impl PendingCard {
    pub fn req_id(&self) -> &str {
        match self {
            Self::Permission { req_id, .. }
            | Self::AskUserQuestion { req_id, .. }
            | Self::ExitPlanMode { req_id, .. } => req_id,
        }
    }
}

/// A prompt queued while Claude is busy with an active turn.
#[derive(Clone, Debug)]
pub struct QueuedMessage {
    pub id: u64,
    pub display_text: String,
    pub blocks: Vec<PromptBlock>,
    pub file_chips: Vec<String>,
    pub paste_labels: Vec<String>,
    pub row_images: Vec<RowImage>,
}

/// State of the `Ctrl+F` transcript find bar.
#[derive(Clone, Debug, Default)]
pub struct FindState {
    pub open: bool,
    pub query: String,
    pub matches: Vec<usize>,
    pub current: usize,
}

/// Which composer toolbar popover is currently open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenPopover {
    Model,
    Effort,
    Mode,
}

/// Active `/` command or `@` file completion popup above the composer.
#[derive(Clone, Debug)]
pub enum CompletionPopup {
    Slash { start: usize, end: usize, items: Vec<CommandMatch>, selected: usize },
    AtFile { start: usize, end: usize, items: Vec<FileMatch>, selected: usize },
}

/// Full-resolution image currently displayed in the lightbox modal.
#[derive(Clone)]
pub struct LightboxImage {
    pub image: Arc<RenderImage>,
    pub size: (u32, u32),
    pub label: String,
    /// When `true`, this texture isn't tracked in `ImageCache` and must be dropped on close.
    pub owned: bool,
}

// ---------------------------------------------------------------------------
// Classification & formatting helpers
// ---------------------------------------------------------------------------

pub fn is_read_search_tool(name: &str) -> Option<ReadKind> {
    match name {
        "Read" | "LS" | "ListDir" | "NotebookRead" => Some(ReadKind::Read),
        "Glob" | "Grep" | "Search" | "WebSearch" | "ToolSearch" => Some(ReadKind::Search),
        "WebFetch" => Some(ReadKind::Fetch),
        _ => None,
    }
}

pub fn is_edit_tool(name: &str) -> bool {
    matches!(name, "Edit" | "MultiEdit" | "Write" | "NotebookEdit")
}

pub fn is_todo_tool(name: &str) -> bool {
    matches!(name, "TodoWrite" | "TaskCreate" | "TaskUpdate")
}

pub fn is_subagent_tool(name: &str) -> bool {
    matches!(name, "Agent" | "Task")
}

pub fn is_shell_tool(name: &str) -> bool {
    matches!(name, "Bash" | "PowerShell" | "Shell" | "Terminal")
}

pub fn extract_file_path(input: &Value) -> Option<String> {
    for key in ["file_path", "path", "notebook_path", "target_file"] {
        if let Some(s) = input.get(key).and_then(Value::as_str).filter(|s| !s.is_empty()) {
            return Some(s.to_string());
        }
    }
    None
}

pub fn extract_command(input: &Value) -> Option<String> {
    input.get("command").or_else(|| input.get("cmd")).and_then(Value::as_str).map(str::to_string)
}

/// Strips `workspace` prefix when `path` sits inside it, normalizing slashes for compact display.
pub fn short_path(workspace: &std::path::Path, raw: &str) -> String {
    let norm_raw = raw.replace('\\', "/");
    let norm_ws = workspace.to_string_lossy().replace('\\', "/");
    let ws = norm_ws.trim_end_matches('/');
    if !ws.is_empty()
        && let Some(rest) = norm_raw.strip_prefix(ws)
    {
        let trimmed = rest.trim_start_matches('/');
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    // Case-insensitive strip on Windows where drive casing can differ.
    #[cfg(windows)]
    if !ws.is_empty() && norm_raw.len() > ws.len() {
        let head = &norm_raw[..ws.len()];
        if head.eq_ignore_ascii_case(ws) && norm_raw.as_bytes().get(ws.len()) == Some(&b'/') {
            return norm_raw[ws.len() + 1..].to_string();
        }
    }
    norm_raw
}

/// Produces a concise 1-line summary of a tool call for collapsed rows and subagent tracks.
pub fn tool_summary(workspace: &std::path::Path, name: &str, input: &Value) -> String {
    if let Some(cmd) = extract_command(input) {
        let first = cmd.lines().next().unwrap_or("").trim();
        return truncate_chars(first, 96);
    }
    if let Some(p) = extract_file_path(input) {
        return short_path(workspace, &p);
    }
    if let Some(pat) = input.get("pattern").and_then(Value::as_str) {
        if let Some(dir) = input.get("path").and_then(Value::as_str) {
            return format!("{pat} in {}", short_path(workspace, dir));
        }
        return truncate_chars(pat, 80);
    }
    if let Some(q) = input.get("query").or_else(|| input.get("url")).and_then(Value::as_str) {
        return truncate_chars(q, 80);
    }
    if let Some(desc) = input.get("description").or_else(|| input.get("prompt")).and_then(Value::as_str) {
        let first = desc.lines().next().unwrap_or("").trim();
        return truncate_chars(first, 80);
    }
    if let Some(obj) = input.as_object()
        && let Some((k, v)) = obj.iter().next()
    {
        let val = v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string());
        return truncate_chars(&format!("{k}: {val}"), 80);
    }
    name.to_string()
}

pub fn format_input_preview(input: &Value) -> String {
    if input.is_null() {
        return String::new();
    }
    let pretty = serde_json::to_string_pretty(input).unwrap_or_else(|_| input.to_string());
    truncate_chars(&pretty, 1200)
}

pub fn truncate_chars(s: &str, max: usize) -> String {
    let mut chars = s.chars();
    let taken: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() { format!("{taken}…") } else { taken }
}

/// Extracts `@path` mentions from a user prompt for chip rendering.
pub fn extract_at_chips(text: &str) -> Vec<String> {
    let mut chips = Vec::new();
    for token in text.split_whitespace() {
        let trimmed = token.trim_matches(|c: char| matches!(c, '(' | ')' | '[' | ']' | ',' | '.' | ';' | '"' | '\''));
        if let Some(path) = trimmed.strip_prefix('@')
            && !path.is_empty()
            && (path.contains('/') || path.contains('\\') || path.contains('.'))
            && !chips.iter().any(|c| c == path)
        {
            chips.push(path.to_string());
        }
    }
    chips
}

pub fn format_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

pub fn format_cost(usd: f64) -> String {
    if !usd.is_finite() || usd <= 0.0 {
        "$0.00".into()
    } else if usd < 0.01 {
        format!("${usd:.4}")
    } else {
        format!("${usd:.2}")
    }
}

pub fn format_duration(ms: u64) -> String {
    if ms < 1_000 {
        format!("{ms}ms")
    } else if ms < 60_000 {
        format!("{:.1}s", ms as f64 / 1_000.0)
    } else {
        let secs = ms / 1_000;
        format!("{}m {}s", secs / 60, secs % 60)
    }
}

/// Applies a `TodoWrite` / `TaskCreate` / `TaskUpdate` tool input onto the live plan checklist.
pub fn apply_todo_input(steps: &mut Vec<PlanStep>, tool_name: &str, input: &Value) {
    match tool_name {
        "TodoWrite" => {
            let Some(arr) = input.get("todos").and_then(Value::as_array) else { return };
            steps.clear();
            for item in arr {
                let Some(content) = item.get("content").and_then(Value::as_str) else { continue };
                let status = item.get("status").and_then(Value::as_str).unwrap_or("pending").to_string();
                let active_form = item.get("activeForm").and_then(Value::as_str).map(str::to_string);
                steps.push(PlanStep { content: content.to_string(), status, active_form });
            }
        }
        "TaskCreate" => {
            let Some(subject) = input.get("subject").or_else(|| input.get("content")).and_then(Value::as_str) else {
                return;
            };
            let active_form = input.get("activeForm").and_then(Value::as_str).map(str::to_string);
            steps.push(PlanStep { content: subject.to_string(), status: "pending".into(), active_form });
        }
        "TaskUpdate" => {
            let idx = input
                .get("taskId")
                .and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
                .and_then(|id| id.checked_sub(1))
                .map(|i| i as usize);
            if let Some(i) = idx
                && let Some(step) = steps.get_mut(i)
            {
                if let Some(st) = input.get("status").and_then(Value::as_str) {
                    step.status = st.to_string();
                }
                if let Some(subj) = input.get("subject").and_then(Value::as_str) {
                    step.content = subj.to_string();
                }
                if let Some(af) = input.get("activeForm").and_then(Value::as_str) {
                    step.active_form = Some(af.to_string());
                }
            }
        }
        _ => {}
    }
}

/// Appends one forwarded subagent message into `subagents`.
pub fn apply_subagent_msg(
    workspace: &std::path::Path,
    subagents: &mut Vec<SubagentTrack>,
    parent_tool_use_id: &str,
    msg: SubagentMessage,
) {
    let pos = match subagents.iter().position(|s| s.tool_use_id == parent_tool_use_id) {
        Some(i) => i,
        None => {
            subagents.push(SubagentTrack {
                tool_use_id: parent_tool_use_id.to_string(),
                task_id: None,
                label: "Subagent".into(),
                last_tool: None,
                summary: None,
                running: true,
                status: "Running".into(),
                rows: Vec::new(),
            });
            subagents.len() - 1
        }
    };
    let track = &mut subagents[pos];
    match msg {
        SubagentMessage::Thought(t) => {
            if let Some(SubagentRow::Thought(prev)) = track.rows.last_mut() {
                prev.push_str(&t);
            } else {
                track.rows.push(SubagentRow::Thought(t));
            }
        }
        SubagentMessage::Text(t) => {
            if let Some(SubagentRow::Prose { text, blocks }) = track.rows.last_mut() {
                text.push_str(&t);
                *blocks = markdown::parse(text);
            } else {
                let blocks = markdown::parse(&t);
                track.rows.push(SubagentRow::Prose { text: t, blocks });
            }
        }
        SubagentMessage::ToolUse { id, name, input } => {
            track.last_tool = Some(name.clone());
            let summary = tool_summary(workspace, &name, &input);
            track.rows.push(SubagentRow::Tool { id, name, summary, output: None, is_error: false });
        }
        SubagentMessage::ToolResult { tool_use_id, content, is_error, .. } => {
            if let Some(SubagentRow::Tool { output, is_error: err, .. }) =
                track.rows.iter_mut().rev().find(|r| matches!(r, SubagentRow::Tool { id, .. } if id == &tool_use_id))
            {
                *output = Some(content);
                *err = is_error;
            }
        }
    }
}

/// Converts replayed `.jsonl` [`HistoryItem`]s into transcript [`LogRow`]s and [`SubagentTrack`]s.
pub fn history_items_to_rows(
    workspace: &std::path::Path,
    items: Vec<HistoryItem>,
    out: &mut Vec<LogRow>,
    subagents: &mut Vec<SubagentTrack>,
    next_id: &mut impl FnMut() -> u64,
) {
    for item in items {
        match item {
            HistoryItem::User { uuid, text, images } => {
                let row_imgs = images
                    .into_iter()
                    .enumerate()
                    .map(|(i, r)| RowImage {
                        key: ImageKey::Logged(LoggedKey::from(&r)),
                        logged_ref: Some(r),
                        label: format!("Image #{}", i + 1),
                    })
                    .collect();
                let file_chips = extract_at_chips(&text);
                out.push(LogRow::UserPrompt {
                    id: next_id(),
                    uuid,
                    text,
                    images: row_imgs,
                    file_chips,
                    paste_labels: Vec::new(),
                });
            }
            HistoryItem::Thought(text) => {
                out.push(LogRow::Thought {
                    id: next_id(),
                    text,
                    started_at: None,
                    duration_ms: None,
                    expanded: false,
                    streaming: false,
                });
            }
            HistoryItem::Prose(raw) => {
                let blocks = markdown::parse(&raw);
                let plain = markdown::extract_plain_text(&blocks);
                out.push(LogRow::Prose { id: next_id(), raw, blocks, plain, streaming: false });
            }
            HistoryItem::ToolUse { id: tid, name, input, result, is_error, images } => {
                if is_todo_tool(&name) {
                    continue;
                }
                if let Some(kind) = is_read_search_tool(&name) {
                    let entry = ReadSearchEntry {
                        tool_use_id: tid,
                        kind,
                        tool_name: name.clone(),
                        label: tool_summary(workspace, &name, &input),
                        path: extract_file_path(&input),
                        done: true,
                        is_error,
                    };
                    if let Some(LogRow::ReadSearchRun { entries, .. }) = out.last_mut() {
                        entries.push(entry);
                    } else {
                        out.push(LogRow::ReadSearchRun { id: next_id(), entries: vec![entry], expanded: false });
                    }
                } else if is_edit_tool(&name) {
                    let path = extract_file_path(&input).unwrap_or_else(|| name.clone());
                    let diff = diff_from_tool_input(&name, &input).unwrap_or_default();
                    out.push(LogRow::EditRow {
                        id: next_id(),
                        tool_use_id: tid,
                        tool_name: name,
                        path,
                        diff,
                        expanded: false,
                        done: true,
                        is_error,
                        error_text: is_error.then(|| result.unwrap_or_default()).filter(|s| !s.is_empty()),
                    });
                } else {
                    let row_imgs = images
                        .into_iter()
                        .enumerate()
                        .map(|(i, r)| RowImage {
                            key: ImageKey::Logged(LoggedKey::from(&r)),
                            logged_ref: Some(r),
                            label: format!("Tool image #{}", i + 1),
                        })
                        .collect();
                    out.push(LogRow::ToolCall {
                        id: next_id(),
                        tool_use_id: tid,
                        name: name.clone(),
                        summary: tool_summary(workspace, &name, &input),
                        command: extract_command(&input),
                        input_preview: format_input_preview(&input),
                        output: result.unwrap_or_default(),
                        images: row_imgs,
                        started_at: None,
                        duration_ms: None,
                        done: true,
                        is_error,
                        expanded: false,
                    });
                }
            }
            HistoryItem::Subagent { parent_tool_use_id, prompt, items: sub_items } => {
                let label = prompt
                    .as_deref()
                    .map(|p| truncate_chars(p.lines().next().unwrap_or(p).trim(), 64))
                    .unwrap_or_else(|| "Subagent".into());
                let mut rows = Vec::new();
                for si in sub_items {
                    match si {
                        HistoryItem::Thought(t) => rows.push(SubagentRow::Thought(t)),
                        HistoryItem::Prose(p) => {
                            let blocks = markdown::parse(&p);
                            rows.push(SubagentRow::Prose { text: p, blocks });
                        }
                        HistoryItem::ToolUse { id, name, input, result, is_error, .. } => {
                            rows.push(SubagentRow::Tool {
                                id,
                                summary: tool_summary(workspace, &name, &input),
                                name,
                                output: result,
                                is_error,
                            });
                        }
                        _ => {}
                    }
                }
                if !subagents.iter().any(|s| s.tool_use_id == parent_tool_use_id) {
                    subagents.push(SubagentTrack {
                        tool_use_id: parent_tool_use_id,
                        task_id: None,
                        label,
                        last_tool: None,
                        summary: None,
                        running: false,
                        status: "Done".into(),
                        rows,
                    });
                }
            }
            HistoryItem::CompactBoundary => {
                out.push(LogRow::CompactionDivider { detail: "Conversation compacted".into() });
            }
        }
    }
}
