//! Grok Build's own record of a session, read back for the sessions list and
//! a reopened session's transcript.
//!
//! Grok keeps each session in `~/.grok/sessions/<cwd, percent-encoded>/<id>/`:
//! `summary.json` names it and dates it, `chat_history.jsonl` holds its turns.
//! Kynex only reads the ones it started (their ids are in `sessions.json`).
//! The layout isn't documented, so everything here reads what it can and
//! leaves out what it can't.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::{Value, json};

use crate::session::SessionInfo;

/// `~/.grok/sessions`.
fn root() -> Option<PathBuf> {
    Some(dirs::home_dir()?.join(".grok").join("sessions"))
}

/// `cwd` as Grok names its folder: percent-encoded, all but `A–Z a–z 0–9 - . _ ~`.
fn folder_name(cwd: &Path) -> String {
    let mut out = String::new();
    for b in cwd.to_string_lossy().bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// A session id that can only name a folder: no separators, no `..`.
fn safe_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Where Grok keeps session `id` of `cwd`; looked for under every folder if
/// it isn't where `cwd` says (a moved folder, a drive letter in other case).
/// Blocks on the file system.
pub fn session_dir(cwd: &Path, id: &str) -> Option<PathBuf> {
    if !safe_id(id) {
        return None;
    }
    let root = root()?;
    let dir = root.join(folder_name(cwd)).join(id);
    if dir.is_dir() {
        return Some(dir);
    }
    std::fs::read_dir(root).ok()?.filter_map(Result::ok).map(|e| e.path().join(id)).find(|p| p.is_dir())
}

/// Session `id` of `cwd` for the sessions list, from Grok's `summary.json`;
/// `None` if Grok has no such session. Blocks on a small file read.
pub fn info(cwd: &Path, id: &str) -> Option<SessionInfo> {
    let dir = session_dir(cwd, id)?;
    let summary = std::fs::read_to_string(dir.join("summary.json")).ok().and_then(|s| serde_json::from_str(&s).ok());
    let modified = || std::fs::metadata(&dir).and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
    Some(info_from(id, &summary.unwrap_or(Value::Null), modified))
}

/// [`info`] from a parsed `summary.json`; `modified` dates it when it has no time of its own.
pub(crate) fn info_from(id: &str, v: &Value, modified: impl FnOnce() -> SystemTime) -> SessionInfo {
    let text = |k: &str| v[k].as_str().map(str::trim).filter(|t| !t.is_empty()).map(str::to_string);
    let title = text("generated_title").or_else(|| text("session_summary")).or_else(|| crate::titles::stored(id));
    let at = ["last_active_at", "updated_at", "created_at"]
        .iter()
        .find_map(|k| v[*k].as_str().and_then(crate::rewind::parse_time));
    SessionInfo {
        title: title.clone().unwrap_or_else(|| "Grok session".into()),
        first: title.clone().unwrap_or_default(),
        named: title.is_some(),
        id: id.to_string(),
        modified: at.unwrap_or_else(modified),
        branch: None,
        ran_in: text("cwd").or_else(|| v["info"]["cwd"].as_str().map(str::to_string)).map(PathBuf::from),
    }
}

/// Session `id`'s turns as the log entries a transcript replays; `None` if
/// Grok has no history for it. Blocks on reading the history.
pub(crate) fn history(cwd: &Path, id: &str) -> Option<Vec<Value>> {
    let text = std::fs::read_to_string(session_dir(cwd, id)?.join("chat_history.jsonl")).ok()?;
    Some(entries(&text))
}

/// Grok's `chat_history.jsonl` as Claude-shaped log entries: what the user
/// said, the replies with their tool calls, and the tools' results. The system
/// prompt, the context Grok adds as user messages, and reasoning are left out.
pub(crate) fn entries(text: &str) -> Vec<Value> {
    text.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).filter_map(|v| entry(&v)).collect()
}

fn entry(v: &Value) -> Option<Value> {
    match v["type"].as_str()? {
        "user" => {
            let text = match &v["content"] {
                Value::String(s) => s.clone(),
                Value::Array(blocks) => blocks.iter().filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("\n"),
                _ => return None,
            };
            // A prompt carries its index; one typed in Grok's own UI comes wrapped.
            let said = match text.trim().strip_prefix("<user_query>").and_then(|t| t.strip_suffix("</user_query>")) {
                Some(inner) => inner.trim().to_string(),
                None if !v["prompt_index"].is_null() => text,
                None => return None,
            };
            (!said.trim().is_empty()).then(|| json!({ "type": "user", "message": { "content": said } }))
        }
        "assistant" => {
            let mut blocks = Vec::new();
            if let Some(t) = v["content"].as_str().filter(|t| !t.trim().is_empty()) {
                blocks.push(json!({ "type": "text", "text": t }));
            }
            for call in v["tool_calls"].as_array().into_iter().flatten() {
                let input = match &call["arguments"] {
                    Value::String(s) => serde_json::from_str(s).unwrap_or_else(|_| Value::String(s.clone())),
                    other => other.clone(),
                };
                blocks.push(json!({
                    "type": "tool_use",
                    "id": call["id"].as_str().unwrap_or(""),
                    "name": call["name"].as_str().unwrap_or("tool"),
                    "input": input,
                }));
            }
            let model = v["model_id"].as_str().unwrap_or("");
            (!blocks.is_empty())
                .then(|| json!({ "type": "assistant", "message": { "model": model, "content": blocks } }))
        }
        "tool_result" => Some(json!({ "type": "user", "message": { "content": [{
            "type": "tool_result",
            "tool_use_id": v["tool_call_id"].as_str().unwrap_or(""),
            "content": v["content"].clone(),
        }] } })),
        _ => None,
    }
}

/// Deletes Grok's record of session `id`; false if it had none. Blocks.
pub(crate) fn delete(cwd: &Path, id: &str) -> Result<bool, String> {
    let Some(dir) = session_dir(cwd, id) else { return Ok(false) };
    std::fs::remove_dir_all(&dir).map_err(|e| format!("couldn't delete {}: {e}", dir.display()))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_are_named_as_grok_names_them() {
        assert_eq!(folder_name(Path::new(r"C:\Users\me\repos\kynex")), "C%3A%5CUsers%5Cme%5Crepos%5Ckynex");
        assert_eq!(folder_name(Path::new(r"C:\Programs\Gaea 2.0")), "C%3A%5CPrograms%5CGaea%202.0");
        assert_eq!(folder_name(Path::new("/home/me/a_b~c")), "%2Fhome%2Fme%2Fa_b~c");
    }

    #[test]
    fn only_plain_ids_name_a_folder() {
        assert!(safe_id("01a1997d-0dc6-7d11-b22d-3a5e9997eeb5"));
        for bad in ["", "..", "../x", r"a\b", "a/b"] {
            assert!(!safe_id(bad), "{bad}");
        }
    }

    #[test]
    fn a_summary_names_and_dates_the_session() {
        let v = json!({ "info": { "id": "x", "cwd": "C:\\work" }, "generated_title": "Add a smoke line",
                        "session_summary": "older", "last_active_at": "2026-11-02T00:19:30.424996700Z" });
        let info = info_from("x", &v, || SystemTime::UNIX_EPOCH);
        assert_eq!(info.title, "Add a smoke line");
        assert!(info.named);
        assert_eq!(info.ran_in, Some(PathBuf::from("C:\\work")));
        assert!(info.modified > SystemTime::UNIX_EPOCH);
        // Nothing in it: still listed, dated by the folder.
        let bare = info_from("y", &Value::Null, || SystemTime::UNIX_EPOCH);
        assert_eq!(bare.title, "Grok session");
        assert!(!bare.named);
    }

    #[test]
    fn history_becomes_the_turns_said() {
        let history = r#"{"type":"system","content":"You are Grok"}
{"type":"user","content":[{"type":"text","text":"<user_info>\nOS: windows\n</user_info>"}]}
{"type":"user","content":[{"type":"text","text":"Add smoke to note.txt"}],"prompt_index":0}
{"type":"reasoning","id":"rs_1","summary":[],"encrypted_content":"x"}
{"type":"assistant","content":"On it.","tool_calls":[{"id":"call-1","name":"search_replace","arguments":"{\"file_path\":\"note.txt\",\"old_string\":\"a\",\"new_string\":\"b\"}"}],"model_id":"grok-4.7-build"}
{"type":"tool_result","tool_call_id":"call-1","content":"updated"}
{"type":"assistant","content":"Done.","model_id":"grok-4.7-build"}
{"type":"user","content":[{"type":"text","text":"<user_query>\nand again\n</user_query>"}]}
not json"#;
        let e = entries(history);
        assert_eq!(e.len(), 5, "{e:#?}");
        assert_eq!(e[0]["message"]["content"], "Add smoke to note.txt");
        assert_eq!(e[1]["message"]["content"][1]["input"]["file_path"], "note.txt");
        assert_eq!(e[2]["message"]["content"][0]["tool_use_id"], "call-1");
        assert_eq!(e[4]["message"]["content"], "and again");

        // Replayed, it reads as a transcript.
        let mut s = crate::session::Session::new(crate::session::Kind::Code, std::env::temp_dir());
        for v in e {
            s.replay_entry(v);
        }
        use crate::session::Item;
        assert!(matches!(&s.items[0], Item::User { text, .. } if text == "Add smoke to note.txt"));
        assert!(s.items.iter().any(|i| matches!(i, Item::ToolUse { name, .. } if name == "search_replace")));
        assert!(s.items.iter().any(|i| matches!(i, Item::ToolResult { .. })));
        assert!(matches!(s.items.last(), Some(Item::User { text, .. }) if text == "and again"));
    }
}
