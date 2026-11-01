//! The harness's conversation log, in Claude Code's format and place
//! (`~/.claude/projects/<folder>/<id>.jsonl`), so Kynex lists, opens, names
//! and rewinds these sessions like any other; and the API messages it holds.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::stream;

/// One session's log, appended to as the conversation goes.
pub struct Log {
    pub session_id: String,
    path: Option<PathBuf>,
    cwd: String,
    /// The entry the next one follows.
    parent: Option<String>,
}

impl Log {
    /// The log of a conversation in `cwd`: session `resume`'s, continued from
    /// its entry `at` if given, or a new one. Returns it with the messages so
    /// far, as the API takes them. Blocks on reading the log.
    pub fn open(cwd: &Path, resume: Option<&str>, at: Option<&str>) -> (Log, Vec<Value>) {
        let session_id = resume.map_or_else(new_id, str::to_string);
        let path = crate::session::transcript_path(cwd, &session_id);
        let entries = match (&path, resume) {
            (Some(p), Some(_)) => {
                std::fs::read_to_string(p).map(|log| crate::rewind::branch_at(&log, at)).unwrap_or_default()
            }
            _ => Vec::new(),
        };
        let parent = entries.last().and_then(|v| v["uuid"].as_str()).map(str::to_string);
        let log = Log { session_id, path, cwd: cwd.to_string_lossy().into_owned(), parent };
        (log, messages(&entries))
    }

    /// Logs a message the user sent (`content` as Kynex sent it).
    pub fn user(&mut self, content: &Value) -> Result<(), String> {
        self.append(json!({ "type": "user", "message": { "role": "user", "content": content } }))
    }

    /// Logs a reply: `content` blocks (thinking, text) from `model`.
    pub fn assistant(
        &mut self,
        id: &str,
        model: &str,
        content: Vec<Value>,
        usage: &stream::Usage,
    ) -> Result<(), String> {
        self.append(json!({
            "type": "assistant",
            "message": {
                "id": id,
                "type": "message",
                "role": "assistant",
                "model": model,
                "content": content,
                "stop_reason": "end_turn",
                "usage": { "input_tokens": usage.input, "output_tokens": usage.output },
            },
        }))
    }

    fn append(&mut self, mut entry: Value) -> Result<(), String> {
        let uuid = new_id();
        let fields = [
            ("parentUuid", self.parent.clone().map_or(Value::Null, Value::String)),
            ("isSidechain", false.into()),
            ("userType", "external".into()),
            ("entrypoint", "kynex-chat".into()),
            ("cwd", self.cwd.clone().into()),
            ("sessionId", self.session_id.clone().into()),
            ("uuid", uuid.clone().into()),
            ("timestamp", chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string().into()),
        ];
        for (k, v) in fields {
            entry[k] = v;
        }
        self.parent = Some(uuid);
        let path = self.path.as_ref().ok_or("no home folder to keep the conversation in")?;
        let write = || -> std::io::Result<()> {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
            writeln!(file, "{entry}")
        };
        write().map_err(|e| format!("couldn't save the conversation to {}: {e}", path.display()))
    }
}

/// The API messages in a branch of log entries: what the user sent and the
/// replies' text. Thinking, tool calls and results (a CLI session's) and
/// failed requests are left out; a reply logged block by block is one message.
pub(crate) fn messages(entries: &[Value]) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    let mut last_id: Option<String> = None;
    for v in entries {
        if v["isMeta"] == true || v["isSidechain"] == true {
            continue;
        }
        match v["type"].as_str() {
            Some("user") if !crate::session::written_by_cli(v) => {
                if let Some(content) = stream::user_content(&v["message"]["content"]) {
                    out.push(json!({ "role": "user", "content": content }));
                    last_id = None;
                }
            }
            Some("assistant") if v["message"]["model"] != "<synthetic>" => {
                let text = stream::assistant_text(&v["message"]["content"]);
                if text.is_empty() {
                    continue;
                }
                let id = v["message"]["id"].as_str().map(str::to_string);
                match out.last_mut() {
                    Some(prev) if id.is_some() && id == last_id && prev["role"] == "assistant" => {
                        let joined = format!("{}{text}", prev["content"].as_str().unwrap_or(""));
                        prev["content"] = joined.into();
                    }
                    _ => out.push(json!({ "role": "assistant", "content": text })),
                }
                last_id = id;
            }
            _ => {}
        }
    }
    out
}

/// A random version 4 UUID, for session and entry IDs.
pub(crate) fn new_id() -> String {
    use sha2::{Digest, Sha256};
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNT: AtomicU64 = AtomicU64::new(0);
    let mut h = Sha256::new();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    h.update(now.as_nanos().to_le_bytes());
    h.update(std::process::id().to_le_bytes());
    h.update(COUNT.fetch_add(1, Ordering::Relaxed).to_le_bytes());
    h.update(format!("{:?}", std::thread::current().id()).as_bytes());
    let mut b: [u8; 16] = h.finalize()[..16].try_into().unwrap_or_default();
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &hex[..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_v4_uuids_and_differ() {
        let (a, b) = (new_id(), new_id());
        assert_ne!(a, b);
        assert_eq!(a.len(), 36);
        assert_eq!(&a[14..15], "4");
        assert!(matches!(&a[19..20], "8" | "9" | "a" | "b"), "{a}");
    }

    #[test]
    fn a_log_gives_back_the_conversation() {
        let entries = vec![
            json!({ "type": "user", "uuid": "1", "message": { "role": "user", "content": "hi" } }),
            json!({ "type": "user", "uuid": "m", "isMeta": true, "message": { "content": "caveat" } }),
            // The CLI logs a reply block by block, under one message ID.
            json!({ "type": "assistant", "uuid": "2", "message": { "id": "a", "model": "x",
                "content": [{ "type": "thinking", "thinking": "hm" }] } }),
            json!({ "type": "assistant", "uuid": "3", "message": { "id": "a", "model": "x",
                "content": [{ "type": "text", "text": "hello" }] } }),
            json!({ "type": "assistant", "uuid": "4", "message": { "id": "b", "model": "x",
                "content": [{ "type": "tool_use", "name": "Read", "input": {} }] } }),
            json!({ "type": "user", "uuid": "5", "message": { "content": [{ "type": "tool_result", "content": "x" }] } }),
            json!({ "type": "assistant", "uuid": "6", "message": { "id": "c", "model": "x",
                "content": [{ "type": "text", "text": " there" }] } }),
            json!({ "type": "assistant", "uuid": "7", "message": { "model": "<synthetic>",
                "content": [{ "type": "text", "text": "API Error: 400" }] } }),
            json!({ "type": "user", "uuid": "8", "message": { "content": "again" } }),
        ];
        assert_eq!(
            messages(&entries),
            vec![
                json!({ "role": "user", "content": "hi" }),
                json!({ "role": "assistant", "content": "hello" }),
                json!({ "role": "assistant", "content": " there" }),
                json!({ "role": "user", "content": "again" }),
            ]
        );
    }

    #[test]
    fn a_log_is_written_and_continued() {
        let home = std::env::temp_dir().join(format!("kynex-chat-log-{}", new_id()));
        let path = home.join("s.jsonl");
        let mut log = Log { session_id: "s".into(), path: Some(path.clone()), cwd: "C:\\w".into(), parent: None };
        log.user(&json!("hi")).unwrap();
        log.assistant("g1", "m/x", vec![json!({ "type": "text", "text": "hello" })], &stream::Usage::default())
            .unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let branch = crate::rewind::branch(&text);
        assert_eq!(branch.len(), 2);
        assert_eq!(branch[1]["parentUuid"], branch[0]["uuid"]);
        assert_eq!(branch[0]["sessionId"], "s");
        assert_eq!(crate::session::first_message(text.lines().next().unwrap()).as_deref(), Some("hi"));
        assert!(crate::rewind::parse_time(branch[1]["timestamp"].as_str().unwrap()).is_some());
        assert_eq!(messages(&branch).len(), 2);
        // Continued from the first entry: the reply is on another branch.
        let first = branch[0]["uuid"].as_str().unwrap();
        assert_eq!(messages(&crate::rewind::branch_at(&text, Some(first))).len(), 1);
        let _ = std::fs::remove_dir_all(&home);
    }
}
