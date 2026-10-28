//! Entry points for the fuzz targets in `crates/kynex-core/fuzz/`: one per
//! format Kynex reads from outside, each feeding arbitrary text through the
//! parsers for it. None may panic, whatever the input. Not part of the API.

use std::path::PathBuf;

use serde_json::Value;

use crate::session::{Kind, Session};
use crate::{cli, commands, git, markdown, rewind, session, tasks, update};

/// The CLI's stream-json output, one event per line, as a live session reads it.
pub fn stream_json(text: &str) {
    let mut s = Session::new(Kind::Chat, PathBuf::new());
    for line in text.lines() {
        if let Ok(v) = serde_json::from_str::<Value>(line) {
            s.handle(v);
        }
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
            git_status(t);
            git_diff(t);
            git_log(t);
            git_branch(t);
            git_worktree(t);
            markdown(t);
            versions(t.as_bytes());
        }
    }
}
