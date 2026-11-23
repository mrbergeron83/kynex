//! Fast, branch-aware `.jsonl` transcript replayer for `~/.claude/projects/<slug>/<id>.jsonl`.
//!
//! - Follows the active leaf's `parentUuid` chain so rewound/abandoned branches are excluded.
//! - Extracts lightweight [`LoggedImageRef`] byte spans instead of keeping megabytes of base64 in RAM.
//! - Supports progressive two-phase loading ([`load_progressive`]) so reopened sessions render the
//!   most recent turns immediately and stream older turns afterwards.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::Sender;

use base64::Engine as _;
use serde_json::Value;

/// Number of recent user turns emitted first by [`load_progressive`].
pub const RECENT_TURNS: usize = 30;

/// Maximum single `.jsonl` line size read from disk (matches the 32 MiB stream reader cap).
const MAX_LINE_BYTES: usize = 32 * 1024 * 1024;

/// A lazy reference to a base64 image block inside a `.jsonl` transcript file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoggedImageRef {
    pub path: PathBuf,
    /// Byte offset of the JSONL line from the start of `path`.
    pub line_offset: u64,
    /// Byte length of the JSONL line (excluding trailing `\r`/`\n`).
    pub line_len: usize,
    /// 0-based index of this image among all base64 image blocks within that JSONL line.
    pub block_index: usize,
    pub media_type: String,
}

/// One checklist item reconstructed from `TodoWrite` / `TaskCreate` / `TaskUpdate` tool calls.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TodoItem {
    pub content: String,
    pub status: String,
    pub active_form: Option<String>,
}

/// Session-level metadata reconstructed alongside the transcript items.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HistoryMeta {
    pub context_tokens: u64,
    pub model: Option<String>,
    pub latest_todos: Vec<TodoItem>,
}

/// One reconstructed item in a replayed Claude Code transcript.
#[derive(Clone, Debug, PartialEq)]
pub enum HistoryItem {
    User {
        uuid: Option<String>,
        text: String,
        images: Vec<LoggedImageRef>,
    },
    Thought(String),
    Prose(String),
    ToolUse {
        id: String,
        name: String,
        input: Value,
        result: Option<String>,
        is_error: bool,
        images: Vec<LoggedImageRef>,
    },
    Subagent {
        parent_tool_use_id: String,
        prompt: Option<String>,
        items: Vec<HistoryItem>,
    },
    CompactBoundary,
}

/// Progressive load notifications sent by [`load_progressive`].
#[derive(Clone, Debug)]
pub enum LoadedChunk {
    /// Most recent ~[`RECENT_TURNS`] turns, sent first for instant rendering.
    Recent { items: Vec<HistoryItem>, meta: HistoryMeta, has_older: bool },
    /// Earlier turns to prepend before the recent chunk.
    Older { items: Vec<HistoryItem> },
    /// Loading finished (even if the session had no transcript file yet).
    Done,
}

/// Lazily reads and decodes a single base64 image referenced by [`LoggedImageRef`].
/// Returns `(media_type, decoded_bytes)` on success.
pub fn load_logged_image(img: &LoggedImageRef) -> Option<(String, Vec<u8>)> {
    if img.line_len == 0 || img.line_len > MAX_LINE_BYTES {
        return None;
    }
    let mut file = File::open(&img.path).ok()?;
    file.seek(SeekFrom::Start(img.line_offset)).ok()?;
    let mut buf = vec![0u8; img.line_len];
    file.read_exact(&mut buf).ok()?;
    let value: Value = serde_json::from_slice(&buf).ok()?;
    let mut sources = Vec::new();
    collect_image_sources(&value, &mut sources);
    let (media_type, data) = *sources.get(img.block_index)?;
    let compact: String = data.chars().filter(|c| !c.is_whitespace()).collect();
    let bytes = base64::engine::general_purpose::STANDARD.decode(compact.as_bytes()).ok()?;
    let mt = if media_type.is_empty() { img.media_type.clone() } else { media_type.to_string() };
    Some((mt, bytes))
}

/// Parses an entire `.jsonl` transcript from an in-memory string. Never panics on untrusted input.
pub fn parse_str(content: &str, source_path: Option<&Path>) -> (Vec<HistoryItem>, HistoryMeta) {
    let mut nodes = Vec::new();
    let mut offset = 0u64;
    for raw_line in content.split_inclusive('\n') {
        let line_len_with_nl = raw_line.len();
        let trimmed = raw_line.trim_end_matches(['\r', '\n']);
        if !trimmed.is_empty()
            && let Some(node) = parse_line_node(trimmed.as_bytes(), offset, trimmed.len(), source_path)
        {
            nodes.push(node);
        }
        offset += line_len_with_nl as u64;
    }
    assemble_history(nodes)
}

/// Loads and parses a `.jsonl` transcript file from disk.
pub fn load_file(path: &Path) -> std::io::Result<(Vec<HistoryItem>, HistoryMeta)> {
    let file = File::open(path)?;
    let mut reader = BufReader::with_capacity(64 * 1024, file);
    let mut nodes = Vec::new();
    let mut buf = Vec::new();
    let mut offset = 0u64;

    loop {
        buf.clear();
        let n = reader.read_until(b'\n', &mut buf)?;
        if n == 0 {
            break;
        }
        let mut line_len = n;
        if line_len > 0 && buf[line_len - 1] == b'\n' {
            line_len -= 1;
        }
        if line_len > 0 && buf[line_len - 1] == b'\r' {
            line_len -= 1;
        }
        if line_len > 0
            && line_len <= MAX_LINE_BYTES
            && let Some(node) = parse_line_node(&buf[..line_len], offset, line_len, Some(path))
        {
            nodes.push(node);
        }
        offset += n as u64;
    }

    Ok(assemble_history(nodes))
}

/// Loads a session's transcript for `(cwd, session_id)` if it exists on disk.
pub fn load_session(cwd: &Path, session_id: &str) -> Option<(Vec<HistoryItem>, HistoryMeta)> {
    let path = crate::sessions::find_claude_transcript(cwd, session_id)?;
    load_file(&path).ok()
}

/// Splits `items` into `(older, recent)` keeping the last `max_turns` `User` turns in `recent`.
pub fn split_recent_turns(items: Vec<HistoryItem>, max_turns: usize) -> (Vec<HistoryItem>, Vec<HistoryItem>) {
    if max_turns == 0 || items.is_empty() {
        return (items, Vec::new());
    }
    let mut user_seen = 0usize;
    let mut split_at = 0usize;
    for (idx, item) in items.iter().enumerate().rev() {
        if matches!(item, HistoryItem::User { .. }) {
            user_seen += 1;
            if user_seen >= max_turns {
                split_at = idx;
                break;
            }
        }
    }
    if split_at == 0 {
        return (Vec::new(), items);
    }
    let mut older = items;
    let recent = older.split_off(split_at);
    (older, recent)
}

/// Loads a Claude session transcript on a background thread, sending the most recent ~30 turns
/// first ([`LoadedChunk::Recent`]), then older turns ([`LoadedChunk::Older`]), then [`LoadedChunk::Done`],
/// invoking `wake()` after each send.
pub fn load_progressive(cwd: PathBuf, session_id: String, tx: Sender<LoadedChunk>, wake: Arc<dyn Fn() + Send + Sync>) {
    let _ = std::thread::Builder::new().name("claude-history".into()).spawn(move || {
        if let Some((items, meta)) = load_session(&cwd, &session_id) {
            let (older, recent) = split_recent_turns(items, RECENT_TURNS);
            let has_older = !older.is_empty();
            if tx.send(LoadedChunk::Recent { items: recent, meta, has_older }).is_ok() {
                wake();
            } else {
                return;
            }
            if has_older {
                if tx.send(LoadedChunk::Older { items: older }).is_ok() {
                    wake();
                } else {
                    return;
                }
            }
        }
        if tx.send(LoadedChunk::Done).is_ok() {
            wake();
        }
    });
}

// ---------------------------------------------------------------------------
// Internal lightweight node representation (never retains base64 payloads)
// ---------------------------------------------------------------------------

struct RawNode {
    uuid: Option<String>,
    parent_uuid: Option<String>,
    parent_tool_use_id: Option<String>,
    is_sidechain: bool,
    payload: RawPayload,
}

enum RawPayload {
    User {
        is_meta: bool,
        text: String,
        images: Vec<LoggedImageRef>,
        tool_results: Vec<RawToolResult>,
    },
    Assistant {
        model: Option<String>,
        context_tokens: Option<u64>,
        blocks: Vec<RawAssistantBlock>,
    },
    CompactBoundary,
    /// Non-renderable chain node (kept only so `parentUuid` links stay unbroken).
    ChainOnly,
}

struct RawToolResult {
    tool_use_id: String,
    content: String,
    is_error: bool,
    images: Vec<LoggedImageRef>,
}

enum RawAssistantBlock {
    Thought(String),
    Prose(String),
    ToolUse { id: String, name: String, input: Value },
}

fn parse_line_node(line: &[u8], offset: u64, line_len: usize, source_path: Option<&Path>) -> Option<RawNode> {
    let v: Value = serde_json::from_slice(line).ok()?;
    let kind = v.get("type").and_then(Value::as_str)?;
    let uuid = v.get("uuid").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string);
    let parent_uuid = v.get("parentUuid").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string);
    let parent_tool_use_id = v
        .get("parentToolUseId")
        .or_else(|| v.get("parent_tool_use_id"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let is_sidechain = v.get("isSidechain").and_then(Value::as_bool).unwrap_or(false);

    let payload = match kind {
        "user" => parse_user_payload(&v, offset, line_len, source_path),
        "assistant" => parse_assistant_payload(&v),
        "system" => {
            if v.get("subtype").and_then(Value::as_str) == Some("compact_boundary") {
                RawPayload::CompactBoundary
            } else {
                RawPayload::ChainOnly
            }
        }
        _ => {
            if uuid.is_some() {
                RawPayload::ChainOnly
            } else {
                return None;
            }
        }
    };

    Some(RawNode { uuid, parent_uuid, parent_tool_use_id, is_sidechain, payload })
}

fn parse_user_payload(v: &Value, offset: u64, line_len: usize, source_path: Option<&Path>) -> RawPayload {
    let is_meta = v.get("isMeta").and_then(Value::as_bool).unwrap_or(false);
    let content = v.pointer("/message/content");
    let mut text_parts = Vec::new();
    let mut images = Vec::new();
    let mut tool_results = Vec::new();
    let mut img_ordinal = 0usize;

    match content {
        Some(Value::String(s)) => {
            if !s.is_empty() {
                text_parts.push(s.clone());
            }
        }
        Some(Value::Array(blocks)) => {
            for block in blocks {
                match block.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        if let Some(t) = block.get("text").and_then(Value::as_str)
                            && !t.is_empty()
                        {
                            text_parts.push(t.to_string());
                        }
                    }
                    Some("image") => {
                        if let Some(mt) = image_media_type(block) {
                            if let Some(path) = source_path {
                                images.push(LoggedImageRef {
                                    path: path.to_path_buf(),
                                    line_offset: offset,
                                    line_len,
                                    block_index: img_ordinal,
                                    media_type: mt,
                                });
                            }
                            img_ordinal += 1;
                        }
                    }
                    Some("tool_result") => {
                        let id = block.get("tool_use_id").and_then(Value::as_str).unwrap_or_default().to_string();
                        let is_error = block.get("is_error").and_then(Value::as_bool).unwrap_or(false);
                        let mut result_images = Vec::new();
                        let result_text = extract_tool_result_content(
                            block.get("content"),
                            offset,
                            line_len,
                            source_path,
                            &mut img_ordinal,
                            &mut result_images,
                        );
                        if !id.is_empty() {
                            tool_results.push(RawToolResult {
                                tool_use_id: id,
                                content: result_text,
                                is_error,
                                images: result_images,
                            });
                        }
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }

    RawPayload::User { is_meta, text: text_parts.join("\n"), images, tool_results }
}

fn extract_tool_result_content(
    content: Option<&Value>,
    offset: u64,
    line_len: usize,
    source_path: Option<&Path>,
    img_ordinal: &mut usize,
    out_images: &mut Vec<LoggedImageRef>,
) -> String {
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(arr)) => {
            let mut parts = Vec::new();
            for item in arr {
                match item.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        if let Some(t) = item.get("text").and_then(Value::as_str) {
                            parts.push(t.to_string());
                        }
                    }
                    Some("image") => {
                        if let Some(mt) = image_media_type(item) {
                            if let Some(path) = source_path {
                                out_images.push(LoggedImageRef {
                                    path: path.to_path_buf(),
                                    line_offset: offset,
                                    line_len,
                                    block_index: *img_ordinal,
                                    media_type: mt,
                                });
                            }
                            *img_ordinal += 1;
                        }
                    }
                    _ => {}
                }
            }
            parts.join("\n")
        }
        Some(other) if !other.is_null() => other.to_string(),
        _ => String::new(),
    }
}

fn parse_assistant_payload(v: &Value) -> RawPayload {
    let msg = v.get("message");
    let model = msg.and_then(|m| m.get("model")).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string);

    let context_tokens = msg.and_then(|m| m.get("usage")).and_then(|u| {
        let input = u.get("input_tokens").and_then(Value::as_u64).unwrap_or(0);
        let cache_create = u.get("cache_creation_input_tokens").and_then(Value::as_u64).unwrap_or(0);
        let cache_read = u.get("cache_read_input_tokens").and_then(Value::as_u64).unwrap_or(0);
        let total = input.saturating_add(cache_create).saturating_add(cache_read);
        (total > 0).then_some(total)
    });

    let mut blocks = Vec::new();
    if let Some(Value::Array(arr)) = msg.and_then(|m| m.get("content")) {
        for block in arr {
            match block.get("type").and_then(Value::as_str) {
                Some("thinking") => {
                    if let Some(t) = block.get("thinking").and_then(Value::as_str)
                        && !t.is_empty()
                    {
                        blocks.push(RawAssistantBlock::Thought(t.to_string()));
                    }
                }
                Some("text") => {
                    if let Some(t) = block.get("text").and_then(Value::as_str)
                        && !t.is_empty()
                    {
                        blocks.push(RawAssistantBlock::Prose(t.to_string()));
                    }
                }
                Some("tool_use") => {
                    let id = block.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
                    let name = block.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
                    let input = block.get("input").cloned().unwrap_or(Value::Null);
                    if !name.is_empty() {
                        blocks.push(RawAssistantBlock::ToolUse { id, name, input });
                    }
                }
                _ => {}
            }
        }
    }

    RawPayload::Assistant { model, context_tokens, blocks }
}

fn image_media_type(block: &Value) -> Option<String> {
    let source = block.get("source")?;
    if source.get("type").and_then(Value::as_str) != Some("base64") {
        return None;
    }
    source.get("data").and_then(Value::as_str)?;
    Some(source.get("media_type").and_then(Value::as_str).unwrap_or("image/png").to_string())
}

fn collect_image_sources<'a>(v: &'a Value, out: &mut Vec<(&'a str, &'a str)>) {
    let Some(Value::Array(blocks)) = v.pointer("/message/content") else { return };
    for block in blocks {
        match block.get("type").and_then(Value::as_str) {
            Some("image") => push_image_source(block, out),
            Some("tool_result") => {
                if let Some(Value::Array(inner)) = block.get("content") {
                    for sub in inner {
                        if sub.get("type").and_then(Value::as_str) == Some("image") {
                            push_image_source(sub, out);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

fn push_image_source<'a>(block: &'a Value, out: &mut Vec<(&'a str, &'a str)>) {
    let Some(source) = block.get("source") else { return };
    if source.get("type").and_then(Value::as_str) != Some("base64") {
        return;
    }
    let Some(data) = source.get("data").and_then(Value::as_str) else { return };
    let mt = source.get("media_type").and_then(Value::as_str).unwrap_or("image/png");
    out.push((mt, data));
}

// ---------------------------------------------------------------------------
// Active branch resolution & item assembly
// ---------------------------------------------------------------------------

fn assemble_history(nodes: Vec<RawNode>) -> (Vec<HistoryItem>, HistoryMeta) {
    if nodes.is_empty() {
        return (Vec::new(), HistoryMeta::default());
    }

    let mut subagent_indices: HashMap<String, Vec<usize>> = HashMap::new();
    let mut uuid_to_idx: HashMap<String, usize> = HashMap::new();
    let mut main_indices: Vec<usize> = Vec::new();

    for (idx, node) in nodes.iter().enumerate() {
        if let Some(parent_tool) = &node.parent_tool_use_id {
            subagent_indices.entry(parent_tool.clone()).or_default().push(idx);
            continue;
        }
        if node.is_sidechain {
            continue;
        }
        if let Some(uuid) = &node.uuid {
            uuid_to_idx.insert(uuid.clone(), idx);
        }
        main_indices.push(idx);
    }

    let active_indices = select_active_branch(&nodes, &main_indices, &uuid_to_idx);
    let mut meta = HistoryMeta::default();
    let (mut items, tool_positions) = materialize_nodes(&nodes, &active_indices, Some(&mut meta));

    // Attach any subagent conversations whose parent tool call appears on the active branch.
    let mut subagent_entries: Vec<(usize, HistoryItem)> = Vec::new();
    for (parent_tool_id, sub_idxs) in subagent_indices {
        let Some(&insert_after) = tool_positions.get(&parent_tool_id) else { continue };
        let (sub_items, _) = materialize_nodes(&nodes, &sub_idxs, None);
        if sub_items.is_empty() {
            continue;
        }
        let prompt = sub_items.iter().find_map(|item| match item {
            HistoryItem::User { text, .. } if !text.is_empty() => Some(text.clone()),
            _ => None,
        });
        subagent_entries.push((
            insert_after,
            HistoryItem::Subagent { parent_tool_use_id: parent_tool_id, prompt, items: sub_items },
        ));
    }

    // Insert subagent blocks right after their parent ToolUse (reverse order keeps indices stable).
    subagent_entries.sort_by_key(|(pos, _)| std::cmp::Reverse(*pos));
    for (pos, subagent) in subagent_entries {
        let insert_at = (pos + 1).min(items.len());
        items.insert(insert_at, subagent);
    }

    (items, meta)
}

fn select_active_branch(nodes: &[RawNode], main_indices: &[usize], uuid_to_idx: &HashMap<String, usize>) -> Vec<usize> {
    // Find the last main-thread node with a uuid. If the log has no uuids at all, keep file order.
    let Some(&leaf_idx) = main_indices.iter().rev().find(|&&i| nodes[i].uuid.is_some()) else {
        return main_indices.to_vec();
    };

    let mut chain = Vec::new();
    let mut visited = HashSet::new();
    let mut cur = Some(leaf_idx);
    while let Some(idx) = cur {
        if !visited.insert(idx) {
            break;
        }
        chain.push(idx);
        cur = nodes[idx].parent_uuid.as_deref().and_then(|p| uuid_to_idx.get(p).copied());
    }
    chain.reverse();
    chain
}

fn materialize_nodes(
    nodes: &[RawNode],
    indices: &[usize],
    mut meta: Option<&mut HistoryMeta>,
) -> (Vec<HistoryItem>, HashMap<String, usize>) {
    let mut items: Vec<HistoryItem> = Vec::new();
    let mut tool_positions: HashMap<String, usize> = HashMap::new();

    for &idx in indices {
        let node = &nodes[idx];
        match &node.payload {
            RawPayload::User { is_meta, text, images, tool_results } => {
                for tr in tool_results {
                    if let Some(&pos) = tool_positions.get(&tr.tool_use_id)
                        && let HistoryItem::ToolUse { result, is_error, images: tool_imgs, .. } = &mut items[pos]
                    {
                        *result = Some(tr.content.clone());
                        *is_error = tr.is_error;
                        tool_imgs.extend(tr.images.iter().cloned());
                    }
                }
                if !*is_meta && (!is_hidden_user_text(text) || !images.is_empty()) {
                    let clean_text = if is_hidden_user_text(text) { String::new() } else { text.clone() };
                    items.push(HistoryItem::User { uuid: node.uuid.clone(), text: clean_text, images: images.clone() });
                }
            }
            RawPayload::Assistant { model, context_tokens, blocks } => {
                if let Some(m) = meta.as_deref_mut() {
                    if let Some(model_name) = model {
                        m.model = Some(model_name.clone());
                    }
                    if let Some(tokens) = context_tokens {
                        m.context_tokens = *tokens;
                    }
                }
                for block in blocks {
                    match block {
                        RawAssistantBlock::Thought(t) => items.push(HistoryItem::Thought(t.clone())),
                        RawAssistantBlock::Prose(p) => items.push(HistoryItem::Prose(p.clone())),
                        RawAssistantBlock::ToolUse { id, name, input } => {
                            if let Some(m) = meta.as_deref_mut() {
                                update_todos(&mut m.latest_todos, name, input);
                            }
                            let pos = items.len();
                            if !id.is_empty() {
                                tool_positions.insert(id.clone(), pos);
                            }
                            items.push(HistoryItem::ToolUse {
                                id: id.clone(),
                                name: name.clone(),
                                input: input.clone(),
                                result: None,
                                is_error: false,
                                images: Vec::new(),
                            });
                        }
                    }
                }
            }
            RawPayload::CompactBoundary => items.push(HistoryItem::CompactBoundary),
            RawPayload::ChainOnly => {}
        }
    }

    (items, tool_positions)
}

fn is_hidden_user_text(text: &str) -> bool {
    let trimmed = text.trim();
    trimmed.is_empty() || trimmed.starts_with("<local-command") || trimmed.starts_with("<command-name>")
}

fn update_todos(todos: &mut Vec<TodoItem>, tool_name: &str, input: &Value) {
    match tool_name {
        "TodoWrite" => {
            let Some(arr) = input.get("todos").and_then(Value::as_array) else { return };
            todos.clear();
            for entry in arr {
                let Some(content) = entry.get("content").and_then(Value::as_str) else { continue };
                let status = entry.get("status").and_then(Value::as_str).unwrap_or("pending").to_string();
                let active_form = entry.get("activeForm").and_then(Value::as_str).map(str::to_string);
                todos.push(TodoItem { content: content.to_string(), status, active_form });
            }
        }
        "TaskCreate" => {
            let Some(subject) = input.get("subject").or_else(|| input.get("content")).and_then(Value::as_str) else {
                return;
            };
            let active_form = input.get("activeForm").and_then(Value::as_str).map(str::to_string);
            todos.push(TodoItem { content: subject.to_string(), status: "pending".into(), active_form });
        }
        "TaskUpdate" => {
            let idx = input
                .get("taskId")
                .and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
                .and_then(|id| id.checked_sub(1))
                .map(|i| i as usize);
            if let Some(i) = idx
                && let Some(item) = todos.get_mut(i)
            {
                if let Some(status) = input.get("status").and_then(Value::as_str) {
                    item.status = status.to_string();
                }
                if let Some(subject) = input.get("subject").and_then(Value::as_str) {
                    item.content = subject.to_string();
                }
                if let Some(af) = input.get("activeForm").and_then(Value::as_str) {
                    item.active_form = Some(af.to_string());
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follows_active_leaf_branch_and_ignores_rewound_branch() {
        let log = concat!(
            r#"{"type":"user","uuid":"u1","parentUuid":null,"message":{"content":"Start"}}"#,
            "\n",
            r#"{"type":"assistant","uuid":"a1","parentUuid":"u1","message":{"model":"claude-sonnet-4-6","usage":{"input_tokens":100,"cache_read_input_tokens":50},"content":[{"type":"thinking","thinking":"hmm"},{"type":"text","text":"Old answer"}]}}"#,
            "\n",
            r#"{"type":"user","uuid":"u2-old","parentUuid":"a1","message":{"content":"Abandoned turn"}}"#,
            "\n",
            r#"{"type":"assistant","uuid":"a2-new","parentUuid":"u1","message":{"model":"claude-opus-4-6","usage":{"input_tokens":200,"cache_creation_input_tokens":300},"content":[{"type":"text","text":"Rewound answer"}]}}"#,
            "\n",
        );

        let (items, meta) = parse_str(log, None);
        assert_eq!(items.len(), 2);
        assert!(matches!(&items[0], HistoryItem::User { text, .. } if text == "Start"));
        assert!(matches!(&items[1], HistoryItem::Prose(p) if p == "Rewound answer"));
        assert_eq!(meta.model.as_deref(), Some("claude-opus-4-6"));
        assert_eq!(meta.context_tokens, 500);
    }

    #[test]
    fn attaches_tool_results_todos_compact_boundary_and_subagents() {
        let log = concat!(
            r#"{"type":"user","uuid":"u1","parentUuid":null,"message":{"content":"Fix it"}}"#,
            "\n",
            r#"{"type":"assistant","uuid":"a1","parentUuid":"u1","message":{"content":[{"type":"tool_use","id":"t1","name":"TodoWrite","input":{"todos":[{"content":"Step 1","status":"in_progress","activeForm":"Doing step 1"}]}},{"type":"tool_use","id":"t2","name":"Agent","input":{"prompt":"explore"}}]}}"#,
            "\n",
            r#"{"type":"user","parentToolUseId":"t2","message":{"content":"Explore repo"}}"#,
            "\n",
            r#"{"type":"assistant","parentToolUseId":"t2","message":{"content":[{"type":"text","text":"Found files"}]}}"#,
            "\n",
            r#"{"type":"user","uuid":"u2","parentUuid":"a1","message":{"content":[{"type":"tool_result","tool_use_id":"t2","content":"Done exploring","is_error":false}]}}"#,
            "\n",
            r#"{"type":"system","subtype":"compact_boundary","uuid":"s1","parentUuid":"u2"}"#,
            "\n",
        );

        let (items, meta) = parse_str(log, None);
        assert_eq!(meta.latest_todos.len(), 1);
        assert_eq!(meta.latest_todos[0].content, "Step 1");
        assert!(items.iter().any(|i| matches!(
            i,
            HistoryItem::ToolUse { id, result: Some(r), is_error: false, .. } if id == "t2" && r == "Done exploring"
        )));
        assert!(items.iter().any(|i| matches!(
            i,
            HistoryItem::Subagent { parent_tool_use_id, prompt: Some(p), items: sub }
                if parent_tool_use_id == "t2" && p == "Explore repo" && sub.len() == 2
        )));
        assert!(matches!(items.last(), Some(HistoryItem::CompactBoundary)));
    }

    #[test]
    fn decodes_logged_images_lazily_from_disk() {
        let dir = std::env::temp_dir().join(format!("kynex-history-img-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("session.jsonl");
        let b64 = base64::engine::general_purpose::STANDARD.encode(b"PNGDATA");
        let line = format!(
            r#"{{"type":"user","uuid":"u1","parentUuid":null,"message":{{"content":[{{"type":"text","text":"See image"}},{{"type":"image","source":{{"type":"base64","media_type":"image/png","data":"{b64}"}}}}]}}}}"#
        );
        std::fs::write(&file, format!("{line}\n")).unwrap();

        let (items, _) = load_file(&file).unwrap();
        let img_ref = match &items[0] {
            HistoryItem::User { images, .. } => &images[0],
            other => panic!("expected user item, got {other:?}"),
        };
        let (mt, bytes) = load_logged_image(img_ref).unwrap();
        assert_eq!(mt, "image/png");
        assert_eq!(bytes, b"PNGDATA");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn splits_recent_turns_and_handles_cycles_safely() {
        let mut items = Vec::new();
        for i in 0..5 {
            items.push(HistoryItem::User { uuid: Some(format!("u{i}")), text: format!("q{i}"), images: vec![] });
            items.push(HistoryItem::Prose(format!("a{i}")));
        }
        let (older, recent) = split_recent_turns(items, 2);
        assert_eq!(older.len(), 6);
        assert_eq!(recent.len(), 4);

        let cyclic = concat!(
            r#"{"type":"user","uuid":"a","parentUuid":"b","message":{"content":"loop"}}"#,
            "\n",
            r#"{"type":"assistant","uuid":"b","parentUuid":"a","message":{"content":[{"type":"text","text":"loop"}]}}"#,
            "\n",
        );
        let (parsed, _) = parse_str(cyclic, None);
        assert_eq!(parsed.len(), 2);
    }
}
