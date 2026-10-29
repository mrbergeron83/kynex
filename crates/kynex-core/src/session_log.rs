//! Reading Claude Code's session logs (`~/.claude/projects/<folder>/<id>.jsonl`)
//! without reading them whole. They get big (100 MB and more: screenshots are
//! logged twice, in base64), but what Kynex needs is near the ends: the first
//! message at the top, the newest title and the current branch at the bottom.
//!
//! [`lines_back`] reads a log from its end, a chunk at a time. [`Branch`]
//! follows the current branch back through its parent links on it, parsing
//! only the lines it's looking for, so the newest turns are there after a few
//! milliseconds and the rest of the history follows.
//!
//! Screenshots stay in the log: a [`LoggedImage`] says where one is, and it's
//! read again only when it's shown.

use std::collections::{HashMap, HashSet};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::Arc;

use serde::Deserialize;
use serde_json::Value;

/// An image in a session log, kept as where it is rather than its bytes
/// (screenshots run to hundreds of KB each). A session replayed from its log
/// has these as its images' [`crate::session::Preview`]s.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct LoggedImage {
    pub path: Arc<PathBuf>,
    /// The log line it's in.
    pub span: Span,
    /// The image block in that line, as a JSON pointer (`/message/content/0/content/1`).
    pub pointer: String,
    pub media_type: String,
    /// Its size in pixels, from its header, if that could be read.
    pub size: Option<(u32, u32)>,
}

impl LoggedImage {
    /// The image's bytes, read from the log. Blocks on the file.
    pub fn load(&self) -> Result<Vec<u8>, String> {
        let mut file = std::fs::File::open(&*self.path).map_err(|e| format!("Couldn't open the session log: {e}"))?;
        let v = read_span(&mut file, self.span, &mut Vec::new()).ok_or("The image's log entry is gone.")?;
        let data = v.pointer(&self.pointer).and_then(|b| b["source"]["data"].as_str()).ok_or("No image there.")?;
        crate::attach::decode_base64(data).ok_or_else(|| "The image doesn't decode.".into())
    }
}

/// Bytes read at a time going back through a whole log: big enough that a
/// screenshot's line spans only a few.
pub(crate) const CHUNK: u64 = 1024 * 1024;

/// Every line of `r` (`len` bytes long), newest first, with the offset it
/// starts at. Line ends (`\n`, `\r\n`) are dropped and blank lines skipped.
/// Reads `chunk` bytes at a time from the end, and no more than the last `cap`
/// bytes: a line that starts before them isn't given. `f` returns false to stop.
pub(crate) fn lines_back<R: Read + Seek>(
    r: &mut R,
    len: u64,
    chunk: u64,
    cap: u64,
    mut f: impl FnMut(u64, &[u8]) -> bool,
) -> io::Result<()> {
    let chunk = chunk.max(1);
    let floor = len.saturating_sub(cap);
    let mut pos = len;
    // The start of a line cut by the last chunk read, which ends in it.
    let mut carry: Vec<u8> = Vec::new();
    while pos > floor {
        let start = pos.saturating_sub(chunk).max(floor);
        // The byte before the chunk too, which says whether a line starts at it.
        let lead = usize::from(start > 0);
        let base = start - lead as u64;
        let mut data = vec![0; (pos - base) as usize];
        r.seek(SeekFrom::Start(base))?;
        r.read_exact(&mut data)?;
        data.extend_from_slice(&carry);
        pos = start;
        // Up to the first newline is the end of a line that starts further back.
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
            if !line.is_empty() && !f(base + from as u64, line) {
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

/// The few fields of a log entry that place it in the conversation's tree.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Link {
    uuid: Option<String>,
    parent_uuid: Option<String>,
    /// A `/compact` boundary has no parent, but names the entry it follows.
    logical_parent_uuid: Option<String>,
    #[serde(default)]
    is_sidechain: bool,
}

impl Link {
    fn parse(line: &[u8]) -> Option<Self> {
        serde_json::from_slice::<Self>(line).ok().filter(|l| l.uuid.is_some() && !l.is_sidechain)
    }

    fn parent(self) -> Option<String> {
        self.parent_uuid.or(self.logical_parent_uuid)
    }
}

/// Where an entry's line is in the log.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Span {
    pub offset: u64,
    pub len: usize,
}

/// A message the user sent (or the CLI did for them), as opposed to a tool's
/// result: a turn starts there, so history cut just above it keeps every tool
/// call with its result.
fn starts_turn(line: &[u8]) -> bool {
    let has = |needle: &[u8]| line.windows(needle.len()).any(|w| w == needle);
    has(br#""type":"user""#) && !has(br#""type":"tool_result""#)
}

/// Walks a log's current branch back from its newest entry (or from entry
/// `from`) through the parent links, the way the CLI resumes it.
pub(crate) struct Branch {
    /// Turns' worth of entries to hand over first.
    pub recent: usize,
}

/// What a [`Branch`] walk found, newest first.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Walked {
    /// The newest entries, from the start of a turn on, parsed.
    pub recent: Vec<(Span, Value)>,
    /// Where the rest of the branch is.
    pub older: Vec<Span>,
}

impl Branch {
    /// Follows the branch back to its start. The newest `recent` entries or
    /// so (up to where their turn starts) go to `on_recent` as soon as they're
    /// read, newest first; it returns false to stop there. Sidechains (subagents'
    /// entries) are skipped. A parent above its child in the log (never seen,
    /// but nothing forbids it) is found with a second, forward pass.
    pub fn walk<R: Read + Seek>(
        &self,
        r: &mut R,
        from: Option<&str>,
        mut on_recent: impl FnMut(Vec<(Span, Value)>) -> bool,
    ) -> io::Result<Option<Walked>> {
        let len = r.seek(SeekFrom::End(0))?;
        let mut out = Walked::default();
        let mut handed = false;
        let mut stopped = false;
        let mut want: Option<String> = from.map(str::to_string);
        // Before the first entry is found, `want` is `None` only when looking for the newest.
        let mut found_any = false;
        let mut needle = want.as_deref().map(uuid_needle);
        let mut seen = HashSet::new();
        lines_back(r, len, CHUNK, u64::MAX, |offset, line| {
            let candidate = match &needle {
                Some(n) => contains(line, n.as_bytes()),
                None => contains(line, br#""uuid":""#),
            };
            if !candidate {
                return true;
            }
            let Some(link) = Link::parse(line) else { return true };
            if want.as_ref().is_some_and(|w| link.uuid.as_ref() != Some(w)) {
                return true;
            }
            found_any = true;
            // A loop in the links: the branch ends here.
            if !seen.insert(link.uuid.clone().unwrap_or_default()) {
                want = None;
                return false;
            }
            let turn = starts_turn(line);
            if handed {
                out.older.push(Span { offset, len: line.len() });
            } else if let Ok(v) = serde_json::from_slice::<Value>(line) {
                out.recent.push((Span { offset, len: line.len() }, v));
                if turn && out.recent.len() >= self.recent {
                    handed = true;
                    if !on_recent(std::mem::take(&mut out.recent)) {
                        stopped = true;
                        return false;
                    }
                }
            }
            want = link.parent();
            needle = want.as_deref().map(uuid_needle);
            want.is_some()
        })?;
        if stopped {
            return Ok(None);
        }
        // The branch goes on above where reading back reached: a parent written
        // after its child, or lines spaced in a way the search doesn't expect.
        if want.is_some() || !found_any {
            let start = if found_any { want.as_deref() } else { from };
            let rest = index_from(r, start, seen)?;
            if handed {
                out.older.extend(rest);
            } else {
                let mut buf = Vec::new();
                for span in rest {
                    if let Some(v) = read_span(r, span, &mut buf) {
                        out.recent.push((span, v));
                    }
                }
            }
        }
        if !handed && !on_recent(std::mem::take(&mut out.recent)) {
            return Ok(None);
        }
        Ok(Some(out))
    }
}

fn uuid_needle(uuid: &str) -> String {
    format!(r#""uuid":"{uuid}""#)
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && hay.windows(needle.len()).any(|w| w == needle)
}

/// One pass from the top: every entry's parent and place, then the branch
/// from `from` (else the newest entry) back to its start, newest first,
/// stopping at an entry already `seen`.
fn index_from<R: Read + Seek>(r: &mut R, from: Option<&str>, mut seen: HashSet<String>) -> io::Result<Vec<Span>> {
    r.seek(SeekFrom::Start(0))?;
    let mut reader = BufReader::new(&mut *r);
    let mut at: HashMap<String, (Option<String>, Span)> = HashMap::new();
    let (mut line, mut offset, mut leaf) = (Vec::new(), 0u64, None);
    loop {
        line.clear();
        let n = reader.read_until(b'\n', &mut line)?;
        if n == 0 {
            break;
        }
        let text = line.strip_suffix(b"\n").unwrap_or(&line);
        let text = text.strip_suffix(b"\r").unwrap_or(text);
        if let Some(link) = Link::parse(text) {
            let uuid = link.uuid.clone().unwrap_or_default();
            leaf = Some(uuid.clone());
            at.insert(uuid, (link.parent(), Span { offset, len: text.len() }));
        }
        offset += n as u64;
    }
    let mut out = Vec::new();
    let mut next = from.map(str::to_string).or(leaf);
    while let Some(uuid) = next.filter(|u| seen.insert(u.clone())) {
        let Some((parent, span)) = at.remove(&uuid) else { break };
        out.push(span);
        next = parent;
    }
    Ok(out)
}

/// The entry at `span`, parsed.
pub(crate) fn read_span<R: Read + Seek>(r: &mut R, span: Span, buf: &mut Vec<u8>) -> Option<Value> {
    buf.resize(span.len, 0);
    r.seek(SeekFrom::Start(span.offset)).ok()?;
    r.read_exact(buf).ok()?;
    serde_json::from_slice(buf).ok()
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use serde_json::json;

    use super::*;

    fn back(text: &str, chunk: u64, cap: u64) -> Vec<(u64, String)> {
        let mut out = Vec::new();
        lines_back(&mut Cursor::new(text), text.len() as u64, chunk, cap, |o, l| {
            out.push((o, String::from_utf8_lossy(l).into_owned()));
            true
        })
        .unwrap();
        out
    }

    fn forward(text: &str) -> Vec<(u64, String)> {
        let mut out = Vec::new();
        let mut offset = 0;
        for line in text.split_inclusive('\n') {
            let l = line.trim_end_matches('\n').trim_end_matches('\r');
            if !l.is_empty() {
                out.push((offset, l.to_string()));
            }
            offset += line.len() as u64;
        }
        out.reverse();
        out
    }

    #[test]
    fn lines_back_match_lines_forward_at_any_chunk_size() {
        for text in ["", "\n", "a", "a\n", "ab\ncd", "ab\r\ncd\r\n", "\n\nlong line here\nx\n\ny", "é\nü\n"] {
            for chunk in [1, 2, 3, 5, 64, 1 << 20] {
                assert_eq!(back(text, chunk, u64::MAX), forward(text), "{text:?} by {chunk}");
            }
        }
    }

    #[test]
    fn lines_back_stop_at_the_cap_and_when_told() {
        let text = "first\nsecond\nthird\n";
        // The last 9 bytes are "\nthird\n" and the end of "second": only whole lines in it count.
        assert_eq!(back(text, 4, 9), [(13, "third".to_string())]);
        assert_eq!(back(text, 4, 13), [(13, "third".to_string()), (6, "second".to_string())]);
        let mut seen = 0;
        lines_back(&mut Cursor::new(text), text.len() as u64, 2, u64::MAX, |_, _| {
            seen += 1;
            false
        })
        .unwrap();
        assert_eq!(seen, 1);
    }

    fn user(uuid: &str, parent: Option<&str>, text: &str) -> Value {
        json!({ "type": "user", "uuid": uuid, "parentUuid": parent,
                "message": { "role": "user", "content": [{ "type": "text", "text": text }] } })
    }

    fn reply(uuid: &str, parent: &str) -> Value {
        json!({ "type": "assistant", "uuid": uuid, "parentUuid": parent,
                "message": { "role": "assistant", "content": [{ "type": "text", "text": "ok" }] } })
    }

    fn tool_result(uuid: &str, parent: &str) -> Value {
        json!({ "type": "user", "uuid": uuid, "parentUuid": parent,
                "message": { "content": [{ "type": "tool_result", "tool_use_id": "t", "content": "done" }] } })
    }

    fn log(entries: &[Value]) -> String {
        entries.iter().map(|v| format!("{v}\n")).collect()
    }

    /// The branch's uuids, oldest first, and how many came first as recent.
    fn walk(text: &str, recent: usize, from: Option<&str>) -> (Vec<String>, usize) {
        let mut r = Cursor::new(text.as_bytes());
        let mut first = Vec::new();
        let w = Branch { recent }
            .walk(&mut r, from, |v| {
                first = v;
                true
            })
            .unwrap()
            .unwrap();
        let mut buf = Vec::new();
        let older: Vec<Value> = w.older.iter().filter_map(|s| read_span(&mut r, *s, &mut buf)).collect();
        let n = first.len();
        let uuids =
            first.iter().map(|(_, v)| v).chain(&older).rev().map(|v| v["uuid"].as_str().unwrap().to_string()).collect();
        (uuids, n)
    }

    #[test]
    fn walks_the_newest_branch_like_the_whole_log_parse() {
        // b1 was rewound: c1 continues from a2. A subagent's line and a compact boundary in between.
        let l = log(&[
            user("a1", None, "first"),
            reply("a2", "a1"),
            user("b1", Some("a2"), "second"),
            reply("b2", "b1"),
            user("c1", Some("a2"), "again"),
            json!({ "type": "assistant", "uuid": "s", "parentUuid": "c1", "isSidechain": true }),
            json!({ "type": "system", "subtype": "compact_boundary", "uuid": "k", "parentUuid": null, "logicalParentUuid": "c1" }),
            user("d1", Some("k"), "after"),
            json!({ "type": "last-prompt", "lastPrompt": "after" }),
        ]);
        let want = ["a1", "a2", "c1", "k", "d1"];
        assert_eq!(walk(&l, usize::MAX, None).0, want);
        assert_eq!(crate::rewind::branch(&l).iter().map(|v| v["uuid"].as_str().unwrap()).collect::<Vec<_>>(), want);
        // From an earlier entry, as a rewind does.
        assert_eq!(walk(&l, usize::MAX, Some("b2")).0, ["a1", "a2", "b1", "b2"]);
    }

    #[test]
    fn hands_over_recent_turns_whole() {
        let l = log(&[
            user("a1", None, "first"),
            reply("a2", "a1"),
            user("b1", Some("a2"), "second"),
            reply("b2", "b1"),
            tool_result("b3", "b2"),
            reply("b4", "b3"),
        ]);
        // Two entries asked for: the walk goes on to where that turn starts (b1).
        let (uuids, recent) = walk(&l, 2, None);
        assert_eq!(uuids, ["a1", "a2", "b1", "b2", "b3", "b4"]);
        assert_eq!(recent, 4);
        // Fewer entries than asked for: all of them are recent.
        assert_eq!(walk(&l, 100, None).1, 6);
    }

    #[test]
    fn finds_a_parent_written_after_its_child() {
        let l = log(&[reply("b", "a"), user("a", None, "first"), user("c", Some("b"), "next")]);
        assert_eq!(walk(&l, usize::MAX, None).0, ["a", "b", "c"]);
        assert_eq!(walk(&l, 1, None).0, ["a", "b", "c"]);
        // Spaced out JSON the quick search misses still reads.
        let spaced = "{\"type\": \"user\", \"uuid\": \"x\", \"parentUuid\": null}\n";
        assert_eq!(walk(spaced, 1, None).0, ["x"]);
    }

    /// Reads every real log under `KYNEX_LOGS_DIR` (a `~/.claude/projects`
    /// folder; read only) back from its end, and checks it finds the branch the
    /// whole-file parse does. Run with `cargo test --release -- --ignored --nocapture real_logs`.
    #[test]
    #[ignore = "reads your real Claude Code logs"]
    fn real_logs_walk_like_the_whole_log_parse() {
        let dir = std::path::PathBuf::from(std::env::var_os("KYNEX_LOGS_DIR").expect("KYNEX_LOGS_DIR"));
        let (mut files, mut slowest) = (0, (std::time::Duration::ZERO, String::new()));
        for e in std::fs::read_dir(dir).unwrap().filter_map(Result::ok) {
            let path = e.path();
            if path.extension().is_none_or(|x| x != "jsonl") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            let want: Vec<String> =
                crate::rewind::branch(&text).iter().map(|v| v["uuid"].as_str().unwrap().to_string()).collect();
            let started = std::time::Instant::now();
            let mut file = std::fs::File::open(&path).unwrap();
            let mut first = std::time::Duration::ZERO;
            let mut recent = Vec::new();
            let w = Branch { recent: 60 }
                .walk(&mut file, None, |v| {
                    first = started.elapsed();
                    recent = v;
                    true
                })
                .unwrap()
                .unwrap();
            let mut buf = Vec::new();
            let older: Vec<Value> = w.older.iter().filter_map(|s| read_span(&mut file, *s, &mut buf)).collect();
            let got: Vec<String> = recent
                .iter()
                .map(|(_, v)| v)
                .chain(&older)
                .rev()
                .map(|v| v["uuid"].as_str().unwrap().to_string())
                .collect();
            assert_eq!(got, want, "{}", path.display());
            if first > slowest.0 {
                slowest = (first, format!("{} ({} MB)", path.display(), text.len() / 1_000_000));
            }
            files += 1;
        }
        println!("{files} logs agree; slowest first paint {:?} for {}", slowest.0, slowest.1);
    }

    #[test]
    fn a_log_with_no_entries_or_a_loop_ends() {
        assert_eq!(walk("", 1, None).0, Vec::<String>::new());
        assert_eq!(walk("not json\n{}\n", 1, None).0, Vec::<String>::new());
        let l = log(&[reply("a", "b"), reply("b", "a")]);
        let (uuids, _) = walk(&l, 1, None);
        assert!(uuids.len() <= 2);
    }
}
