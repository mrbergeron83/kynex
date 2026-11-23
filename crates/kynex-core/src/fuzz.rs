//! Entry points for the fuzz targets in `crates/kynex-core/fuzz/`: one per
//! format Kynex reads from outside, each feeding arbitrary text through the
//! parsers for it. None may panic, whatever the input. Not part of the API.

use serde_json::Value;

use crate::{cli, git, hooks, sessions, update};

/// Grok's `summary.json`, as the sessions list reads it.
pub fn grok_history(text: &str) {
    if let Ok(v) = serde_json::from_str::<Value>(text) {
        let _ = crate::grok::log::info_from("x", &v, || std::time::SystemTime::UNIX_EPOCH);
    }
}

/// Native CLI hook JSON payloads fed to `knx hook <provider> [event]`.
pub fn hook_payload(text: &str) {
    for provider in ["claude", "claude-openrouter", "agy", "agy-openrouter", "other"] {
        let _ = hooks::parse_payload(provider, None, text);
        let _ = hooks::parse_payload(provider, Some("Stop"), text);
    }
}

/// Claude Code session log metadata lines (first message, timestamps).
pub fn session_summary(text: &str) {
    for line in text.lines() {
        let _ = sessions::first_user_message(line.as_bytes());
        let _ = sessions::parse_iso_time(line);
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

/// Files the Git pane's viewer previews: images, SVGs, PDF pages, and caret movement.
pub fn file_view(bytes: &[u8]) {
    use crate::file_view::{Kind, caret, media, pdf};
    let _ = Kind::of("x", bytes);
    let _ = media::decode_image(bytes);
    media::fuzz_svg(bytes);
    let _ = pdf::read_frames(&mut &bytes[..], |_| true);
    let text = String::from_utf8_lossy(bytes);
    let lines: Vec<Option<&str>> = text.split('\n').map(|l| (!l.starts_with('@')).then_some(l)).collect();
    let mut c = caret::Caret::start(lines.as_slice());
    for (i, b) in bytes.iter().take(64).enumerate() {
        use caret::Move::*;
        let m =
            [Left, Right, WordLeft, WordRight, Up, Down, PageUp(3), PageDown(3), LineStart, LineEnd, DocStart, DocEnd]
                [usize::from(*b) % 12];
        c.move_by(lines.as_slice(), m, i % 2 == 0);
        if b % 7 == 0 {
            c.drag_to(lines.as_slice(), caret::Pos::new(usize::from(*b), i));
        }
    }
    let _ = caret::selected_text(lines.as_slice(), &c);
}

/// OpenRouter's answers to a key check, catalogue search, and curl config quoting.
pub fn openrouter(text: &str) {
    if let Some((status, body)) = crate::openrouter::split_status(text) {
        let _ = crate::openrouter::answer(status, &body, "m").map_err(|p| p.message());
    }
    let _ = crate::openrouter::answer(200, text, "m");
    let _ = crate::openrouter::key_info(text).map(|k| k.credit());
    if let Ok(models) = crate::openrouter::catalogue::parse(text) {
        for m in crate::openrouter::search(&models, text.get(..8).unwrap_or("")) {
            let _ = (m.price(), m.context_label(), m.is_free());
        }
    }
    let _ = crate::openrouter::valid_model_id(text);
    let _ = crate::openrouter::import::find(|_| None, Some(text));
    let _ = crate::openrouter::import::reg_value(text, "OPENROUTER_API_KEY");
    if let Some(config) = crate::openrouter::curl_config(text, "https://openrouter.ai/api/v1/key", None) {
        assert_eq!(config.lines().count(), 2, "{config:?}");
    }
}

/// An agent rule file (`agent_panes`), its patterns, and terminal link scanners over screen text.
pub fn agent_rule(text: &str) {
    if let Ok(rule) = crate::agent_panes::parse(text) {
        let _ = crate::agent_panes::detect(&rule, text);
        let id = crate::agent_panes::resume_id(&rule, text);
        let _ = crate::agent_panes::resume_argv(&rule, id.as_deref());
        let _ = rule.submit_bytes();
    }
    for r in crate::terminal::find_urls(text) {
        assert!(text.is_char_boundary(r.start) && text.is_char_boundary(r.end));
    }
    for f in crate::links::find(text) {
        assert!(text.is_char_boundary(f.range.start) && text.is_char_boundary(f.range.end));
    }
    let _ = crate::links::file_url(text);
}

/// Control protocol line parser (`control::protocol`).
pub fn control_request(text: &str) {
    for line in text.lines() {
        let _ = crate::control::protocol::parse_incoming(line);
    }
}

/// Kitty graphics protocol stream filter and command parser (`terminal::kitty`).
pub fn kitty_graphics(bytes: &[u8]) {
    crate::terminal::kitty::fuzz(bytes);
}

/// Syntax highlighting in every language: ranges must stay in bounds, on char boundaries,
/// sorted and non-overlapping.
pub fn highlight(text: &str) {
    for lang in crate::highlight::Lang::ALL {
        let mut h = crate::highlight::Highlighter::new(lang);
        for line in text.split('\n') {
            let line = line.strip_suffix('\r').unwrap_or(line);
            let mut end = 0;
            for (r, _) in h.line(line) {
                assert!(r.start >= end && r.start <= r.end && r.end <= line.len(), "{lang:?} {r:?} in {line:?}");
                assert!(line.is_char_boundary(r.start) && line.is_char_boundary(r.end));
                end = r.end;
            }
        }
    }
}

/// Named layouts, custom themes, custom commands, CLI version & update parsers.
pub fn config_files(text: &str) {
    let _ = crate::named_layouts::parse(text);
    let _ = crate::themes::parse("fuzz", text);
    let _ = crate::user_commands::parse(text);
    let _ = cli::is_newer(text, "1.0.0");
    let _ = update::expected_sum(text, "kynex-x86_64-pc-windows-msvc.zip");
    let _ = crate::migrate::migrate(text);
}

/// Claude Code bidirectional `stream-json` NDJSON parser (`claude::protocol`).
pub fn claude_stream(text: &str) {
    let mut state = crate::claude::protocol::StreamParseState::default();
    for line in text.lines() {
        let _ = crate::claude::protocol::parse_line(line, &mut state);
    }
}

/// Claude Code Markdown parser, diff engine, and composer trigger scanner.
/// The chat transcript's Markdown parser (`markdown::parse`).
pub fn markdown(text: &str) {
    let _ = crate::markdown::parse(text);
}

pub fn claude_markdown(text: &str) {
    let blocks = crate::claude::markdown::parse(text);
    let _ = crate::claude::markdown::extract_plain_text(&blocks);
    let _ = crate::claude::markdown::parse_inline(text);
    let mid = text.len() / 2;
    let split = text.floor_char_boundary(mid);
    let _ = crate::claude::diff::diff_edit(&text[..split], &text[split..]);
    let _ = crate::claude::diff::diff_write(text);
    let _ = crate::claude::composer::active_trigger(text, mid);
}

/// Claude Code `.jsonl` branch-aware transcript history replayer (`claude::history`).
pub fn claude_history(text: &str) {
    let (items, _meta) = crate::claude::history::parse_str(text, None);
    let _ = crate::claude::history::split_recent_turns(items, crate::claude::history::RECENT_TURNS);
}
