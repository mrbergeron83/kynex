//! Short session names, like "Git worktree cleanup".
//!
//! Claude Code's interactive clients (Desktop, VS Code) write an `ai-title` line
//! into a transcript, and `--name` / renames write a `custom-title`; sessions run
//! over stream-json, like Kynex's, get neither. For those Kynex asks a small model
//! once and keeps the answer in `<data dir>/titles.json`, never touching the
//! transcript.

use std::collections::HashMap;
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};

use crate::paths::data_dir;

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
    if let Ok(s) = serde_json::to_string_pretty(&*map) {
        let _ = std::fs::create_dir_all(data_dir());
        let _ = std::fs::write(data_dir().join("titles.json"), s);
    }
}

/// Asks a small model for a title for a conversation that opened with
/// `first_message`. One short `claude -p` call: no tools, nothing saved as a
/// session. Blocks for a few seconds.
pub fn generate(first_message: &str) -> Result<String, String> {
    let excerpt: String = first_message.chars().take(1500).collect();
    let prompt = format!(
        "Name this coding conversation in 2 to 6 words, the way a session list would, \
         e.g. \"Git worktree cleanup\" or \"Fix login redirect\". Reply with the name only.\n\n\
         The conversation opened with:\n{excerpt}"
    );
    let mut cmd = Command::new(crate::cli::program());
    cmd.args(["-p", "--model", "haiku", "--no-session-persistence", "--tools", "", "--output-format", "text"])
        .current_dir(std::env::temp_dir())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
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
        let t = generate("the git view is lacking polish, add staging and a commit box").unwrap();
        assert!(!t.is_empty() && t.len() <= 61, "{t}");
    }
}
