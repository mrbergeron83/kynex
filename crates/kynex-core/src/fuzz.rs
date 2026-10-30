//! Entry points for the fuzz targets in `crates/kynex-core/fuzz/`: one per
//! format Kynex reads from outside, each feeding arbitrary text through the
//! parsers for it. None may panic, whatever the input. Not part of the API.

use std::path::PathBuf;

use serde_json::Value;

use crate::session::{Kind, Session};
use crate::{cli, commands, extensions, git, markdown, mcp_config, rewind, session, tasks, update};

/// The CLI's stream-json output, one event per line, as a live session reads it.
pub fn stream_json(text: &str) {
    let mut s = Session::new(Kind::Chat, PathBuf::new());
    for line in text.lines() {
        if let Ok(v) = serde_json::from_str::<Value>(line) {
            s.handle(v);
        }
        let _ = s.pulse().map(|p| p.label());
    }
    let _ = crate::mcp::handoff(&s.items);
    let _ = crate::mcp::health(&s.mcp);
    let _ = crate::transcript::rows(&s.items);
    if let Some(limits) = s.take_limits() {
        let now = limits.at(crate::usage::now());
        let _ = [now.five_hour, now.seven_day].map(|w| w.map(|w| (w.percent(), w.level())));
        let _ = limits.describe(0);
    }
}

/// A Claude Code transcript on disk: replayed into a session, listed, rewound.
pub fn transcript(text: &str) {
    let mut s = Session::new(Kind::Chat, PathBuf::new());
    for v in rewind::branch(text) {
        s.replay_entry(v);
    }
    let _ = rewind::points_in(text);
    for line in text.lines() {
        let _ = session::first_message(line);
        let _ = rewind::parse_time(line);
        let _ = tasks::parse_notification(line);
    }
}

/// A session log read back from its end: every line, at a chunk size the first
/// byte picks, must come out as reading forward finds it; then the branch is
/// walked and replayed as a session opens.
pub fn session_log(bytes: &[u8]) {
    use std::io::Cursor;

    use crate::session_log::{Branch, lines_back, read_span};
    let chunk = u64::from(bytes.first().copied().unwrap_or(1) % 64) + 1;
    let mut back = Vec::new();
    lines_back(&mut Cursor::new(bytes), bytes.len() as u64, chunk, u64::MAX, |offset, line| {
        back.push((offset, line.to_vec()));
        true
    })
    .unwrap();
    let mut forward = Vec::new();
    let mut offset = 0;
    for line in bytes.split_inclusive(|b| *b == b'\n') {
        let l = line.strip_suffix(b"\n").unwrap_or(line);
        let l = l.strip_suffix(b"\r").unwrap_or(l);
        if !l.is_empty() {
            forward.push((offset, l.to_vec()));
        }
        offset += line.len() as u64;
    }
    forward.reverse();
    assert_eq!(back, forward);

    let mut r = Cursor::new(bytes);
    let mut recent = Vec::new();
    let walked = Branch { recent: chunk as usize }.walk(&mut r, None, |v| {
        recent = v;
        true
    });
    if let Ok(Some(w)) = walked {
        let mut buf = Vec::new();
        let older: Vec<Value> = w.older.iter().rev().filter_map(|s| read_span(&mut r, *s, &mut buf)).collect();
        let mut s = Session::new(Kind::Chat, PathBuf::new());
        for v in older.into_iter().chain(recent.into_iter().rev().map(|(_, v)| v)) {
            s.replay_entry(v);
        }
    }
}

/// `git status --porcelain=v2 -z`.
pub fn git_status(text: &str) {
    let _ = git::status::parse(text);
}

/// `git diff` hunks and `--numstat`.
pub fn git_diff(text: &str) {
    let _ = git::diff::parse(text);
    let _ = git::diff::parse_numstat(text);
}

/// `git log` / `git show` in Kynex's format, and ref decorations.
pub fn git_log(text: &str) {
    let _ = git::log::parse(text);
    let _ = git::log::parse_refs(text);
}

/// `git branch` in Kynex's format.
pub fn git_branch(text: &str) {
    let _ = git::branch::parse(text);
    let _ = git::branch::parse_cherry(text);
    let _ = git::branch::parse_overrides(text);
}

/// What `gh` says about pull requests, protected branches and rules.
pub fn github(text: &str) {
    git::github::fuzz_json(text);
}

/// `git worktree list --porcelain` and `rev-list --left-right --count`.
pub fn git_worktree(text: &str) {
    let _ = git::worktree::parse_worktrees(text);
    let _ = git::worktree::parse_left_right(text);
}

/// Claude's replies, as markdown.
pub fn markdown(text: &str) {
    let _ = markdown::parse(text);
}

/// Versions and update info: `claude --version` and installer output, npm and
/// GitHub release info, `SHA256SUMS`, typed slash commands.
pub fn versions(bytes: &[u8]) {
    let text = String::from_utf8_lossy(bytes);
    let _ = cli::last_line(bytes);
    let _ = cli::strip_escapes(&text);
    let (a, b) = text.split_once('\n').unwrap_or((&text, ""));
    let _ = cli::is_newer(a, b);
    let _ = update::parse_latest(bytes);
    let _ = update::expected_sum(&text, a);
    let _ = commands::parse(&text);
    let _ = commands::unwrap(&text);
}

/// The CLI's config files: `~/.claude.json`, `.mcp.json`, settings, installed plugins.
pub fn claude_config(text: &str) {
    let Ok(v) = serde_json::from_str::<Value>(text) else { return };
    let settings = [v.clone()];
    for cwd in ["", "/", "C:/", "C:\\x\\", "/work/app"] {
        let cwd = std::path::Path::new(cwd);
        let servers = extensions::servers(&v, &v, &settings, cwd);
        let _ = extensions::plugins(&v, &settings, cwd);
        let live = crate::mcp::servers(&v);
        for row in extensions::rows(&servers, &live) {
            let _ = (row.label(), row.source.label(), row.transport.map(|t| t.detail()));
        }
    }
    let _ = extensions::parse_mcp_json(&v);
}

/// What's typed into the Add server form, and `claude mcp list`'s output.
pub fn mcp_config(text: &str) {
    let (name, what) = text.split_once('\n').unwrap_or((text, text));
    if let Ok(server) = mcp_config::NewServer::parse(name, mcp_config::Scope::Local, what) {
        let _ = server.add_args();
    }
    let _ = mcp_config::parse_list(text);
}

/// An HTTP reply from a browser server Kynex runs: status, session, chunked
/// or event-stream body.
pub fn browser_reply(bytes: &[u8]) {
    crate::browser_server::fuzz_reply(bytes);
}

/// File paths in Claude's text, for clickable links.
pub fn file_links(text: &str) {
    let mut last = 0;
    for r in crate::links::find(text) {
        assert!(last <= r.range.start && r.range.start < r.range.end && r.range.end <= text.len(), "{r:?}");
        assert!(text.is_char_boundary(r.range.start) && text.is_char_boundary(r.range.end), "{r:?}");
        last = r.range.end;
        let _ = crate::links::resolve(std::path::Path::new("/w"), &r.path);
    }
}

/// `@file` mentions: the token at every cursor position, and a list of paths
/// (the lines after the first) matched against the first line. Matched
/// positions are in order, in bounds and on char boundaries.
pub fn mentions(text: &str) {
    use crate::mentions::{Files, at_cursor, insert_text, replace_range};
    for (cursor, _) in text.char_indices() {
        if let Some((range, query)) = at_cursor(text, cursor) {
            assert!(range.end == cursor && text[range.clone()].ends_with(query), "{range:?}");
            let whole = replace_range(text, range);
            assert!(text.is_char_boundary(whole.end));
        }
    }
    // Find in the transcript: the first line looked for in the rest.
    let (query, paths) = text.split_once('\n').unwrap_or((text, ""));
    let mut last = 0;
    for r in crate::find::occurrences(paths, query) {
        assert!(last <= r.start && r.start < r.end && r.end <= paths.len(), "{r:?}");
        assert!(paths.is_char_boundary(r.start) && paths.is_char_boundary(r.end), "{r:?}");
        last = r.end;
    }
    let files = Files::new(paths.lines().map(str::to_string).collect());
    for m in files.matching(query, 50) {
        let entry = &files.entries[m.index];
        let _ = insert_text(entry);
        let mut last = None;
        for &p in &m.positions {
            assert!(p < entry.len() && entry.is_char_boundary(p) && last.is_none_or(|l| l < p), "{m:?} in {entry:?}");
            last = Some(p);
        }
    }
}

/// Code in every language the highlighter knows, whole and line by line:
/// ranges sorted, not overlapping, in bounds and on char boundaries.
pub fn highlight(text: &str) {
    use crate::highlight::{Highlighter, Lang, Side, Token};
    fn check(text: &str, ranges: &[(std::ops::Range<usize>, Token)]) {
        let mut last = 0;
        for (r, _) in ranges {
            assert!(last <= r.start && r.start < r.end && r.end <= text.len(), "{ranges:?}");
            assert!(text.is_char_boundary(r.start) && text.is_char_boundary(r.end), "{ranges:?}");
            last = r.end;
        }
    }
    for lang in Lang::ALL {
        check(text, &crate::highlight::highlight(lang, text));
        let mut h = Highlighter::new(lang);
        for line in text.split('\n') {
            check(line, &h.line(line));
        }
        // As a unified diff: `-` old, `+` new, the rest context.
        let lines: Vec<_> = text
            .split('\n')
            .map(|l| match l.as_bytes().first() {
                Some(b'-') => (Side::Old, &l[1..]),
                Some(b'+') => (Side::New, &l[1..]),
                _ => (Side::Both, l),
            })
            .collect();
        let tokens = crate::highlight::diff(lang, lines.iter().copied());
        assert_eq!(tokens.len(), lines.len());
        for ((_, line), ranges) in lines.iter().zip(&tokens) {
            check(line, ranges);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every entry point once, on odd input, so they stay wired up.
    #[test]
    fn entry_points_take_anything() {
        let inputs =
            ["", "\0", "{}", "\n\n", "@@ -1 +1 @@\n", "{\"type\":\"result\"}\n{\"type\":\"user\"}", "\u{1b}[", "é\0\t"];
        for t in inputs {
            stream_json(t);
            transcript(t);
            session_log(t.as_bytes());
            git_status(t);
            git_diff(t);
            git_log(t);
            git_branch(t);
            git_worktree(t);
            markdown(t);
            versions(t.as_bytes());
            claude_config(t);
            mcp_config(t);
            browser_reply(t.as_bytes());
            github(t);
            file_links(t);
            highlight(t);
        }
        claude_config(
            r#"{"mcpServers":{"a":{"command":"x","args":[1]}},"projects":{"/work/app":{}},"plugins":{"p":[{}]}}"#,
        );
    }
}
