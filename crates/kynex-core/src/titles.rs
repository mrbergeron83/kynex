//! Short session names, like "Git worktree cleanup".
//!
//! Claude Code's interactive clients (Desktop, VS Code) write an `ai-title` line
//! into a transcript, and `--name` / renames write a `custom-title`; sessions run
//! over stream-json, like Kynex's, get neither. For those Kynex asks a small model
//! once and keeps the answer in `<data dir>/titles.json`, never touching the
//! transcript.

use std::collections::HashMap;
use std::io::Write;
use std::process::Stdio;
use std::sync::{Mutex, OnceLock};

use crate::paths::data_dir;

/// How many of the first message's words make a title when no model names it.
pub const FIRST_WORDS: usize = 8;

fn store() -> &'static Mutex<HashMap<String, String>> {
    static STORE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    STORE.get_or_init(|| {
        let saved = std::fs::read_to_string(data_dir().join("titles.json"))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Mutex::new(saved)
    })
}

/// The title Kynex made for session `id`, if it has made one.
pub fn stored(id: &str) -> Option<String> {
    store().lock().ok()?.get(id).cloned()
}

pub fn remember(id: &str, title: &str) {
    let Ok(mut map) = store().lock() else { return };
    map.insert(id.to_string(), title.to_string());
    let _ = std::fs::create_dir_all(data_dir());
    let _ = crate::project::write_json(&data_dir().join("titles.json"), &*map);
}

/// A title for a conversation that opened with `first_message`, on `provider`.
/// Anthropic sessions ask a small model: one short `claude -p` call, no tools,
/// nothing saved as a session; it blocks for a few seconds. OpenRouter sessions
/// spend nothing on it and take the message's first words.
pub fn generate(first_message: &str, provider: crate::provider::Provider) -> Result<String, String> {
    if provider.is_openrouter() {
        return Some(crate::fmt::first_words(first_message, FIRST_WORDS))
            .filter(|t| !t.is_empty())
            .ok_or_else(|| "the first message has no words".into());
    }
    let excerpt: String = first_message.chars().take(1500).collect();
    let prompt = format!(
        "Name this coding conversation in 2 to 6 words, the way a session list would, \
         e.g. \"Git worktree cleanup\" or \"Fix login redirect\". Reply with the name only.\n\n\
         The conversation opened with:\n{excerpt}"
    );
    let mut cmd = crate::session::claude_command(&std::env::temp_dir(), &crate::provider::Launch::Anthropic);
    cmd.args(["-p", "--model", "haiku", "--no-session-persistence", "--tools", "", "--output-format", "text"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child =
        crate::process::dies_with_parent(&mut cmd).spawn().map_err(|e| format!("couldn't run claude: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(prompt.as_bytes());
    }
    let out = child.wait_with_output().map_err(|e| format!("claude: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    clean(&String::from_utf8_lossy(&out.stdout)).ok_or_else(|| "claude gave no title".into())
}

/// The model's reply as a title: its first line, without quotes, labels or a
/// final full stop, at most 60 characters.
fn clean(reply: &str) -> Option<String> {
    let line = reply.lines().map(str::trim).find(|l| !l.is_empty())?;
    let line = line.strip_prefix("Title:").or_else(|| line.strip_prefix("Name:")).unwrap_or(line).trim();
    let line = line.trim_matches(|c: char| matches!(c, '"' | '\'' | '`' | '*' | '#') || c.is_whitespace());
    let line = line.trim_end_matches('.').trim();
    if line.is_empty() {
        return None;
    }
    let mut title: String = line.chars().take(60).collect();
    if line.chars().count() > 60 {
        title = title.trim_end().to_string() + "…";
    }
    Some(title)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::Provider;

    #[test]
    fn openrouter_titles_are_the_first_words() {
        let first = "fix the scrolling bug in the git view when a diff is long";
        assert_eq!(generate(first, Provider::OpenRouter).unwrap(), "fix the scrolling bug in the git view…");
        assert!(generate("  ", Provider::OpenRouter).is_err());
    }

    #[test]
    fn cleans_model_replies() {
        assert_eq!(clean("Git worktree cleanup\n").as_deref(), Some("Git worktree cleanup"));
        assert_eq!(clean("\n\"Fix login redirect.\"").as_deref(), Some("Fix login redirect"));
        assert_eq!(clean("Title: **Terminal panel**").as_deref(), Some("Terminal panel"));
        assert_eq!(clean("   \n  "), None);
        assert!(clean(&"word ".repeat(30)).unwrap().ends_with('…'));
    }

    #[test]
    #[ignore = "needs the claude CLI and uses a little quota"]
    fn generates_a_title() {
        let t = generate("the git view is lacking polish, add staging and a commit box", Provider::Anthropic).unwrap();
        assert!(!t.is_empty() && t.len() <= 61, "{t}");
    }
}
