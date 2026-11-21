//! Reading past sessions from each CLI's native on-disk storage for the Workspaces & Sessions
//! tree pane:
//!
//! - **Claude Code**: `~/.claude/projects/<slug>/<id>.jsonl` (fast head/tail metadata scan).
//! - **Grok Build**: `~/.grok/sessions/<percent-encoded-cwd>/<id>/summary.json`.
//! - **Antigravity (`agy`)**: recorded sessions whose `~/.gemini/antigravity-cli/conversations/<id>.db` exists.
//! - **Other CLIs** (`codex`, `copilot`, `cursor`, `opencode`, …): recorded sessions in `sessions.json`.
//!
//! Everything here is plain file I/O with no CLI wrapper or transcript replay.

use std::collections::HashSet;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::Value;

use crate::agent_cli;
use crate::groups::SessionGroups;

/// One past session shown in the Sessions tree.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionInfo {
    pub id: String,
    pub title: String,
    pub first: String,
    pub named: bool,
    pub modified: SystemTime,
    pub branch: Option<String>,
    pub ran_in: Option<PathBuf>,
    /// Canonical provider ID (`claude`, `claude-openrouter`, `grok`, `agy`, `agy-openrouter`, `codex`…).
    pub provider: String,
}

impl SessionInfo {
    /// If this session ran on a different git branch or folder than `cwd`, returns a short label.
    pub fn ran_elsewhere(&self, cwd: &Path, default_branch: Option<&str>) -> Option<&str> {
        if let Some(b) = self.branch.as_deref()
            && default_branch != Some(b)
            && !matches!(b, "HEAD" | "main" | "master")
        {
            return Some(b);
        }
        if let Some(dir) = self.ran_in.as_deref()
            && !crate::workspace::same_folder(dir, cwd)
        {
            return dir.file_name().and_then(|n| n.to_str());
        }
        None
    }
}

/// Reads a Claude Code `.jsonl` log's title (custom title, AI title, or first user message).
pub fn title_of(path: &Path) -> Option<String> {
    let names = read_claude_names(path)?;
    names.title.or_else(|| (!names.first.is_empty()).then_some(names.first))
}

/// Reads the working directory recorded in a Claude Code `.jsonl` log.
pub fn cwd_of(path: &Path) -> Option<PathBuf> {
    read_claude_names(path)?.ran_in
}

/// Lists every past session found for workspace folder `cwd`, newest first. Blocks on disk I/O.
pub fn list_sessions(cwd: &Path) -> Vec<SessionInfo> {
    let groups = SessionGroups::load(cwd);
    let mut sessions: Vec<SessionInfo> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    // 1. Claude Code sessions (`~/.claude/projects/<slug>/*.jsonl`)
    if let Some(entries) = transcripts_dir(cwd).and_then(|d| std::fs::read_dir(d).ok()) {
        for e in entries.filter_map(Result::ok) {
            let path = e.path();
            if path.extension().is_none_or(|ext| ext != "jsonl") {
                continue;
            }
            let Some(id) = path.file_stem().map(|s| s.to_string_lossy().into_owned()) else {
                continue;
            };
            let Some(names) = read_claude_names(&path) else { continue };
            let modified = names
                .last
                .unwrap_or_else(|| e.metadata().ok().and_then(|m| m.modified().ok()).unwrap_or(SystemTime::UNIX_EPOCH));
            let title = names.title.clone().unwrap_or_else(|| names.first.clone());
            let provider = groups.provider_of(&id);
            seen.insert(id.clone());
            sessions.push(SessionInfo {
                id,
                title,
                first: names.first,
                named: names.title.is_some(),
                modified,
                branch: names.branch,
                ran_in: names.ran_in,
                provider,
            });
        }
    }

    // 2. Grok Build sessions (`~/.grok/sessions/<encoded-cwd>/*` + recorded ones)
    for info in crate::grok::log::list(cwd) {
        if seen.insert(info.id.clone()) {
            sessions.push(info);
        }
    }
    for (id, provider) in &groups.providers {
        if seen.contains(id) {
            continue;
        }
        let norm = agent_cli::normalize_provider_id(provider);
        match norm.as_str() {
            agent_cli::GROK => {
                if let Some(info) = crate::grok::log::info(cwd, id) {
                    seen.insert(id.clone());
                    sessions.push(info);
                }
            }
            agent_cli::AGY | agent_cli::AGY_OPENROUTER => {
                if let Some(db) = crate::antigravity::conversation_file(id) {
                    let modified = std::fs::metadata(&db).and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
                    let given = groups.names.get(id).cloned();
                    let title = given.clone().unwrap_or_else(|| "Antigravity session".into());
                    seen.insert(id.clone());
                    sessions.push(SessionInfo {
                        id: id.clone(),
                        title,
                        first: String::new(),
                        named: given.is_some(),
                        modified,
                        branch: None,
                        ran_in: Some(cwd.to_path_buf()),
                        provider: norm,
                    });
                }
            }
            agent_cli::CLAUDE | agent_cli::CLAUDE_OPENROUTER => {}
            other => {
                // Other agent_rules CLIs (codex, copilot, cursor, opencode…) remembered in sessions.json.
                let given = groups.names.get(id).cloned();
                let title = given.clone().unwrap_or_else(|| format!("{other} session"));
                seen.insert(id.clone());
                sessions.push(SessionInfo {
                    id: id.clone(),
                    title,
                    first: String::new(),
                    named: given.is_some(),
                    modified: SystemTime::UNIX_EPOCH,
                    branch: None,
                    ran_in: Some(cwd.to_path_buf()),
                    provider: norm,
                });
            }
        }
    }

    sessions.sort_by_key(|s| std::cmp::Reverse(s.modified));
    sessions
}

/// Which session IDs saved in `groups` are gone on disk. `None` when Claude's folder isn't
/// readable (a moved project / read error) so we don't drop entries on transient failure.
pub fn gone_sessions(cwd: &Path, groups: &SessionGroups) -> Option<Vec<String>> {
    let mut transcripts = HashSet::new();
    if let Some(dir) = transcripts_dir(cwd) {
        let entries = std::fs::read_dir(dir).ok()?;
        for e in entries.flatten() {
            let path = e.path();
            if path.extension().is_some_and(|x| x == "jsonl")
                && let Some(id) = path.file_stem()
            {
                transcripts.insert(id.to_string_lossy().into_owned());
            }
        }
    }
    let grok_readable = crate::grok::log::readable();
    let agy_readable = crate::antigravity::readable();
    let exists = |id: &str| {
        transcripts.contains(id)
            || match groups.provider_of(id).as_str() {
                agent_cli::GROK => !grok_readable || crate::grok::log::session_dir(cwd, id).is_some(),
                agent_cli::AGY | agent_cli::AGY_OPENROUTER => {
                    !agy_readable || crate::antigravity::conversation_file(id).is_some()
                }
                agent_cli::CLAUDE | agent_cli::CLAUDE_OPENROUTER => false,
                // Other CLIs don't have a guaranteed disk layout we inspect; keep recorded entries.
                _ => true,
            }
    };
    Some(groups.saved_ids().into_iter().filter(|id| !exists(id)).map(str::to_string).collect())
}

/// Whether session `id` exists on disk for `cwd`.
pub fn session_exists(cwd: &Path, id: &str, provider: Option<&str>) -> bool {
    if find_claude_transcript(cwd, id).is_some() {
        return true;
    }
    match provider.map(agent_cli::normalize_provider_id).as_deref() {
        Some(agent_cli::GROK) => crate::grok::log::session_dir(cwd, id).is_some(),
        Some(agent_cli::AGY | agent_cli::AGY_OPENROUTER) => crate::antigravity::conversation_file(id).is_some(),
        Some(agent_cli::CLAUDE | agent_cli::CLAUDE_OPENROUTER) => false,
        Some(_) => true,
        None => crate::grok::log::session_dir(cwd, id).is_some() || crate::antigravity::conversation_file(id).is_some(),
    }
}

/// Deletes session `id`'s native files on disk. Blocks on file system.
pub fn delete_session(cwd: &Path, id: &str) -> Result<(), String> {
    if let Some(path) = find_claude_transcript(cwd, id) {
        std::fs::remove_file(&path).map_err(|e| format!("couldn't delete {}: {e}", path.display()))?;
        let _ = std::fs::remove_dir_all(path.with_extension(""));
        return Ok(());
    }
    if crate::grok::log::delete(cwd, id)? || crate::antigravity::delete(id)? {
        return Ok(());
    }
    Ok(())
}

/// `~/.claude/projects`.
pub fn projects_dir() -> Option<PathBuf> {
    Some(dirs::home_dir()?.join(".claude").join("projects"))
}

/// Where Claude Code stores session `.jsonl` files for `cwd`.
pub fn transcripts_dir(cwd: &Path) -> Option<PathBuf> {
    let projects = projects_dir()?;
    let slug = crate::storage::slug(cwd);
    let exact = projects.join(&slug);
    if exact.is_dir() {
        return Some(exact);
    }
    // Windows drive-letter case (`C-` vs `c-`).
    std::fs::read_dir(&projects)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.is_dir() && p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.eq_ignore_ascii_case(&slug)))
}

pub fn find_claude_transcript(cwd: &Path, id: &str) -> Option<PathBuf> {
    if id.is_empty() || id.contains(['/', '\\', '.']) {
        return None;
    }
    let path = transcripts_dir(cwd)?.join(format!("{id}.jsonl"));
    path.is_file().then_some(path)
}

#[derive(Clone, Debug, Default)]
struct ClaudeNames {
    title: Option<String>,
    first: String,
    last: Option<SystemTime>,
    branch: Option<String>,
    ran_in: Option<PathBuf>,
}

/// Reads the head & tail of a Claude Code `.jsonl` session log (capped so 100 MB logs take <1ms).
fn read_claude_names(path: &Path) -> Option<ClaudeNames> {
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    if len == 0 {
        return None;
    }
    let mut out = ClaudeNames::default();

    // Head: first 256 KiB for the first user message & ran_in cwd.
    let head_len = len.min(256 * 1024) as usize;
    let mut head = vec![0u8; head_len];
    file.read_exact(&mut head).ok()?;
    for line in head.split(|b| *b == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.is_empty() {
            continue;
        }
        if out.first.is_empty()
            && let Some(msg) = first_user_message(line)
        {
            out.first = msg;
        }
        if out.ran_in.is_none()
            && let Ok(v) = serde_json::from_slice::<Value>(line)
            && let Some(cwd) = v["cwd"].as_str().filter(|s| !s.is_empty())
        {
            out.ran_in = Some(PathBuf::from(cwd));
        }
        if !out.first.is_empty() && out.ran_in.is_some() {
            break;
        }
    }

    // Tail: up to last 512 KiB for latest title, branch, and timestamp.
    let _ = lines_back(&mut file, len, 64 * 1024, 512 * 1024, |line| {
        if out.title.is_none()
            && (contains(line, br#""custom-title""#) || contains(line, br#""ai-title""#))
            && let Ok(v) = serde_json::from_slice::<Value>(line)
        {
            let t =
                v["customTitle"].as_str().or_else(|| v["aiTitle"].as_str()).map(str::trim).filter(|s| !s.is_empty());
            if let Some(t) = t {
                out.title = Some(t.to_string());
            }
        }
        if out.branch.is_none()
            && contains(line, br#""gitBranch""#)
            && let Ok(v) = serde_json::from_slice::<Value>(line)
            && let Some(b) = v["gitBranch"].as_str().map(str::trim).filter(|s| !s.is_empty())
        {
            out.branch = Some(b.to_string());
        }
        if out.last.is_none()
            && contains(line, br#""timestamp""#)
            && let Ok(v) = serde_json::from_slice::<Value>(line)
            && let Some(ts) = v["timestamp"].as_str().and_then(parse_iso_time)
        {
            out.last = Some(ts);
        }
        !(out.title.is_some() && out.branch.is_some() && out.last.is_some())
    });

    if out.first.is_empty() && out.title.is_none() {
        return None;
    }
    Some(out)
}

/// Parses an ISO-8601 / RFC-3339 timestamp string into `SystemTime`.
pub fn parse_iso_time(s: &str) -> Option<SystemTime> {
    let ts: jiff::Timestamp = s.trim().parse().ok()?;
    Some(SystemTime::from(ts))
}

/// Extracts the first non-empty user message text from a Claude Code `.jsonl` line.
pub fn first_user_message(line: &[u8]) -> Option<String> {
    if !contains(line, br#""type":"user""#) && !contains(line, br#""type": "user""#) {
        return None;
    }
    let v: Value = serde_json::from_slice(line).ok()?;
    if v["type"].as_str() != Some("user") || v["isMeta"] == Value::Bool(true) {
        return None;
    }
    let text = match &v["message"]["content"] {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter(|b| b["type"].as_str() == Some("text"))
            .filter_map(|b| b["text"].as_str())
            .collect::<Vec<_>>()
            .join(" "),
        _ => return None,
    };
    let clean = strip_command_tags(&text);
    let compact: String = clean.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.is_empty() {
        return None;
    }
    const MAX: usize = 120;
    if compact.chars().count() > MAX {
        Some(format!("{}…", compact.chars().take(MAX).collect::<String>()))
    } else {
        Some(compact)
    }
}

fn strip_command_tags(s: &str) -> &str {
    let trimmed = s.trim();
    if trimmed.starts_with("<local-command") || trimmed.starts_with("<command-name>") { "" } else { trimmed }
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && hay.windows(needle.len()).any(|w| w == needle)
}

/// Reads lines from the end of `r` backwards, up to `cap` bytes. Stops when `f` returns false.
pub(crate) fn lines_back<R: Read + Seek>(
    r: &mut R,
    len: u64,
    chunk: u64,
    cap: u64,
    mut f: impl FnMut(&[u8]) -> bool,
) -> io::Result<()> {
    let chunk = chunk.max(1);
    let floor = len.saturating_sub(cap);
    let mut pos = len;
    let mut carry: Vec<u8> = Vec::new();
    while pos > floor {
        let start = pos.saturating_sub(chunk).max(floor);
        let lead = usize::from(start > 0);
        let base = start - lead as u64;
        let mut data = vec![0; (pos - base) as usize];
        r.seek(SeekFrom::Start(base))?;
        r.read_exact(&mut data)?;
        data.extend_from_slice(&carry);
        pos = start;
        let cut = if lead == 0 {
            0
        } else {
            match data.iter().position(|b| *b == b'\n') {
                Some(i) => i + 1,
                None => {
                    data.remove(0);
                    carry = data;
                    continue;
                }
            }
        };
        let mut end = data.len();
        loop {
            let newline = data[cut..end].iter().rposition(|b| *b == b'\n').map(|i| cut + i);
            let from = newline.map_or(cut, |i| i + 1);
            let line = data[from..end].strip_suffix(b"\r").unwrap_or(&data[from..end]);
            if !line.is_empty() && !f(line) {
                return Ok(());
            }
            match newline {
                Some(i) => end = i,
                None => break,
            }
        }
        carry = data[lead.min(cut)..cut].to_vec();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_first_user_message() {
        let line = br#"{"type":"user","message":{"content":"  Fix the   build\nplease  "}}"#;
        assert_eq!(first_user_message(line).as_deref(), Some("Fix the build please"));

        let blocks = br#"{"type":"user","message":{"content":[{"type":"text","text":"Hello"},{"type":"tool_result","content":"ignored"}]}}"#;
        assert_eq!(first_user_message(blocks).as_deref(), Some("Hello"));

        let meta = br#"{"type":"user","isMeta":true,"message":{"content":"hidden"}}"#;
        assert_eq!(first_user_message(meta), None);
    }

    #[test]
    fn reads_claude_log_head_and_tail_metadata() {
        let dir = std::env::temp_dir().join(format!("kynex-sessions-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("11111111-1111-1111-1111-111111111111.jsonl");
        let content = concat!(
            r#"{"type":"user","cwd":"/work/app","message":{"content":"Initial prompt"}}"#,
            "\n",
            r#"{"type":"assistant","gitBranch":"feat/cli","timestamp":"2026-11-20T12:00:00Z"}"#,
            "\n",
            r#"{"type":"custom-title","customTitle":"Renamed session"}"#,
            "\n",
        );
        std::fs::write(&file, content).unwrap();
        let names = read_claude_names(&file).unwrap();
        assert_eq!(names.first, "Initial prompt");
        assert_eq!(names.title.as_deref(), Some("Renamed session"));
        assert_eq!(names.branch.as_deref(), Some("feat/cli"));
        assert_eq!(names.ran_in, Some(PathBuf::from("/work/app")));
        assert!(names.last.is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parses_iso_timestamps() {
        assert!(parse_iso_time("2026-11-20T12:00:00Z").is_some());
        assert_eq!(parse_iso_time("not a time"), None);
    }
}
