//! Rewinding a session to one of its earlier messages, like Claude Code's
//! Esc Esc: the conversation continues from before that message, and the files
//! Claude changed since can go back too.
//!
//! The CLI does both. A session log is a tree (every entry names its parent),
//! so `--resume <id> --resume-session-at <uuid>` continues from any entry and
//! the next message starts a new branch; later resumes follow the newest one.
//! Files come back with `--resume <id> --rewind-files <prompt uuid>`, from the
//! checkpoints the CLI keeps when started with
//! `CLAUDE_CODE_ENABLE_SDK_FILE_CHECKPOINTING` (Kynex sets it for every
//! session, so sessions started elsewhere may have none).

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;

/// Turns on the CLI's file checkpoints, off by default outside its own UI.
pub(crate) const CHECKPOINTS_ENV: &str = "CLAUDE_CODE_ENABLE_SDK_FILE_CHECKPOINTING";

/// A message the session can go back to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Point {
    /// The message's own entry in the session log; file checkpoints go by it.
    pub uuid: String,
    /// The entry before it, which the conversation continues from; `None` for
    /// the first message, where rewinding means starting over.
    pub before: Option<String>,
    /// What was sent, as typed.
    pub text: String,
    /// When it was sent.
    pub time: Option<SystemTime>,
    /// The CLI kept a checkpoint of the files as they were before it.
    pub checkpoint: bool,
    /// Files Claude changed from this message on, as the checkpoints track them.
    pub changed: Vec<String>,
}

/// The entries of the conversation's current branch, oldest first: from the
/// newest entry back through its parents. A `/compact` boundary has no parent
/// but names the entry it follows (`logicalParentUuid`), so it's crossed too.
pub(crate) fn branch(log: &str) -> Vec<Value> {
    let mut by_uuid: HashMap<String, Value> = HashMap::new();
    let mut leaf = None;
    for line in log.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        let Some(uuid) = v["uuid"].as_str().map(str::to_string) else { continue };
        if v["isSidechain"] == true {
            continue;
        }
        leaf = Some(uuid.clone());
        by_uuid.insert(uuid, v);
    }
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut next = leaf;
    while let Some(uuid) = next.filter(|u| seen.insert(u.clone())) {
        let Some(v) = by_uuid.remove(&uuid) else { break };
        next = v["parentUuid"].as_str().or(v["logicalParentUuid"].as_str()).map(str::to_string);
        out.push(v);
    }
    out.reverse();
    out
}

/// A log timestamp, UTC like `2026-10-27T15:57:20.276Z`; fractions are dropped.
fn parse_time(s: &str) -> Option<SystemTime> {
    let n = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, m, d) = (n(0..4)?, n(5..7)?, n(8..10)?);
    let secs = n(11..13)? * 3600 + n(14..16)? * 60 + n(17..19)?;
    // Days since 1970-01-01 in the proleptic Gregorian calendar (Howard Hinnant's days_from_civil).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let total = u64::try_from(days * 86_400 + secs).ok()?;
    Some(UNIX_EPOCH + Duration::from_secs(total))
}

/// What the user typed, if `entry` is a message they sent rather than a tool
/// result, an interruption note or something the CLI added.
fn prompt_text(entry: &Value) -> Option<String> {
    if entry["type"] != "user" || entry["isMeta"] == true || entry["isCompactSummary"] == true {
        return None;
    }
    let text = match &entry["message"]["content"] {
        Value::String(t) => t.clone(),
        Value::Array(blocks) => blocks.iter().filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("\n"),
        _ => return None,
    };
    let text = crate::commands::unwrap(&text).unwrap_or(text);
    let text = text.trim();
    let added = text.starts_with("[Request interrupted") || text.starts_with('<');
    (!text.is_empty() && !added).then(|| text.to_string())
}

/// The messages in `log` the session can go back to, newest first.
pub(crate) fn points_in(log: &str) -> Vec<Point> {
    let branch = branch(log);
    let mut points: Vec<Point> = Vec::new();
    let mut parent: Option<String> = None;
    for entry in &branch {
        if let Some(text) = prompt_text(entry) {
            points.push(Point {
                uuid: entry["uuid"].as_str().unwrap_or("").to_string(),
                before: parent.clone(),
                text,
                time: entry["timestamp"].as_str().and_then(parse_time),
                checkpoint: false,
                changed: Vec::new(),
            });
        }
        parent = entry["uuid"].as_str().map(str::to_string);
    }
    // Checkpoints sit beside the conversation: a snapshot per message, and a
    // delta naming the message's snapshot for each file changed after it.
    let index: HashMap<String, usize> = points.iter().enumerate().map(|(i, p)| (p.uuid.clone(), i)).collect();
    for line in log.lines().filter(|l| l.contains(r#""type":"file-history-"#)) {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        match v["type"].as_str() {
            Some("file-history-snapshot") => {
                if let Some(&i) = v["messageId"].as_str().and_then(|m| index.get(m)) {
                    points[i].checkpoint = true;
                }
            }
            Some("file-history-delta") => {
                let (Some(&at), Some(path)) =
                    (v["snapshotMessageId"].as_str().and_then(|m| index.get(m)), v["trackingPath"].as_str())
                else {
                    continue;
                };
                for p in &mut points[..=at] {
                    if !p.changed.iter().any(|c| c == path) {
                        p.changed.push(path.to_string());
                    }
                }
            }
            _ => {}
        }
    }
    points.reverse();
    points
}

/// The messages session `id` in `cwd` can go back to, newest first. Blocks on
/// reading the session log.
pub fn points(cwd: &Path, id: &str) -> Result<Vec<Point>, String> {
    let path = crate::session::find_transcript(cwd, id).ok_or("This session's log wasn't found.")?;
    let log = std::fs::read_to_string(&path).map_err(|e| format!("Couldn't read {}: {e}", path.display()))?;
    Ok(points_in(&log))
}

/// Puts the files Claude changed since `point` back as they were before it.
/// Blocks while a `claude` process does it (a second or two); the session's
/// own process should be stopped first.
pub fn restore_files(cwd: &Path, id: &str, point: &Point) -> Result<(), String> {
    let mut cmd = crate::session::claude_command(cwd);
    cmd.args(["-p", "--resume", id, "--rewind-files", &point.uuid])
        .env(CHECKPOINTS_ENV, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let out =
        crate::process::dies_with_parent(&mut cmd).output().map_err(|e| format!("failed to start `claude`: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let said = [&out.stderr, &out.stdout].map(|b| String::from_utf8_lossy(b).trim().to_string());
    let said = said.into_iter().find(|s| !s.is_empty()).unwrap_or_else(|| out.status.to_string());
    Err(format!("Couldn't restore the files: {said}"))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn log(entries: &[Value]) -> String {
        entries.iter().map(Value::to_string).collect::<Vec<_>>().join("\n")
    }

    fn user(uuid: &str, parent: Option<&str>, text: &str) -> Value {
        json!({ "type": "user", "uuid": uuid, "parentUuid": parent, "timestamp": "2026-10-27T15:57:20Z",
                "message": { "role": "user", "content": [{ "type": "text", "text": text }] } })
    }

    fn reply(uuid: &str, parent: &str, text: &str) -> Value {
        json!({ "type": "assistant", "uuid": uuid, "parentUuid": parent,
                "message": { "role": "assistant", "content": [{ "type": "text", "text": text }] } })
    }

    #[test]
    fn branch_follows_the_newest_leaf() {
        // b1 was rewound: c1 continues from a2, so b1 and b2 are off the branch.
        let l = log(&[
            user("a1", None, "first"),
            reply("a2", "a1", "ok"),
            user("b1", Some("a2"), "second"),
            reply("b2", "b1", "ok"),
            user("c1", Some("a2"), "again"),
            json!({ "type": "last-prompt", "lastPrompt": "again" }),
        ]);
        let uuids: Vec<_> = branch(&l).iter().map(|v| v["uuid"].as_str().unwrap().to_string()).collect();
        assert_eq!(uuids, ["a1", "a2", "c1"]);
    }

    #[test]
    fn branch_crosses_compact_boundaries_and_skips_sidechains() {
        let l = log(&[
            user("a1", None, "first"),
            reply("a2", "a1", "ok"),
            json!({ "type": "system", "subtype": "compact_boundary", "uuid": "c", "parentUuid": null, "logicalParentUuid": "a2" }),
            json!({ "type": "assistant", "uuid": "s", "parentUuid": "c", "isSidechain": true }),
            user("b1", Some("c"), "after"),
        ]);
        let uuids: Vec<_> = branch(&l).iter().map(|v| v["uuid"].as_str().unwrap().to_string()).collect();
        assert_eq!(uuids, ["a1", "a2", "c", "b1"]);
    }

    #[test]
    fn points_are_the_typed_messages_newest_first() {
        let l = log(&[
            user("a1", None, "first"),
            json!({ "type": "attachment", "uuid": "a2", "parentUuid": "a1" }),
            reply("a3", "a2", "ok"),
            json!({ "type": "user", "uuid": "t", "parentUuid": "a3",
                    "message": { "content": [{ "type": "tool_result", "tool_use_id": "x", "content": "done" }] } }),
            user("i", Some("t"), "[Request interrupted by user]"),
            json!({ "type": "user", "uuid": "m", "parentUuid": "i", "isMeta": true, "message": { "content": "caveat" } }),
            json!({ "type": "user", "uuid": "cmd", "parentUuid": "m",
                    "message": { "content": "<command-name>/compact</command-name>\n<command-args></command-args>" } }),
            user("b1", Some("cmd"), "second"),
        ]);
        let p = points_in(&l);
        let got: Vec<_> = p.iter().map(|p| (p.text.as_str(), p.before.as_deref())).collect();
        assert_eq!(got, [("second", Some("cmd")), ("/compact", Some("m")), ("first", None)]);
        assert_eq!(p[2].time, Some(UNIX_EPOCH + Duration::from_secs(1_793_116_640)));
    }

    #[test]
    fn parses_log_times() {
        let at = |s| parse_time(s).and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs());
        assert_eq!(at("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(at("2000-03-01T00:00:01.5Z"), Some(951_868_801));
        assert_eq!(at("2024-02-29T23:59:59.999Z"), Some(1_709_251_199));
        assert_eq!(at("garbage"), None);
    }

    #[test]
    fn points_know_their_checkpoints_and_the_files_changed_since() {
        let l = log(&[
            user("a1", None, "first"),
            json!({ "type": "file-history-snapshot", "messageId": "a1", "snapshot": {} }),
            reply("a2", "a1", "ok"),
            user("b1", Some("a2"), "write it"),
            json!({ "type": "file-history-snapshot", "messageId": "b1", "snapshot": {} }),
            json!({ "type": "file-history-delta", "messageId": "x", "snapshotMessageId": "b1", "trackingPath": "fruit.txt" }),
            json!({ "type": "file-history-delta", "messageId": "y", "snapshotMessageId": "b1", "trackingPath": "fruit.txt" }),
            reply("b2", "b1", "ok"),
            user("c1", Some("b2"), "just talk"),
        ]);
        let p = points_in(&l);
        let got: Vec<_> = p.iter().map(|p| (p.text.as_str(), p.checkpoint, p.changed.clone())).collect();
        assert_eq!(
            got,
            [
                ("just talk", false, vec![]),
                ("write it", true, vec!["fruit.txt".to_string()]),
                ("first", true, vec!["fruit.txt".to_string()]),
            ]
        );
    }
}
