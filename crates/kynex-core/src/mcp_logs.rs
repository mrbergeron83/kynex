//! What an MCP server said when it last failed. Claude Code logs each
//! server's connections per folder, one JSON line per event, under its cache
//! folder (`claude-cli-nodejs/Cache/<folder>/mcp-logs-<server>/<time>.jsonl`);
//! an `error` line carries the server's stderr or the transport's complaint.

use std::path::{Path, PathBuf};

use serde_json::Value;

/// Where Claude Code keeps its cache: `%LOCALAPPDATA%` on Windows,
/// `~/Library/Caches` on macOS, `$XDG_CACHE_HOME` or `~/.cache` elsewhere.
fn cache_root() -> Option<PathBuf> {
    #[cfg(windows)]
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    #[cfg(target_os = "macos")]
    let base = dirs::home_dir().map(|h| h.join("Library").join("Caches"));
    #[cfg(not(any(windows, target_os = "macos")))]
    let base =
        std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).or_else(|| dirs::home_dir().map(|h| h.join(".cache")));
    Some(base?.join("claude-cli-nodejs").join("Cache"))
}

/// A name as the CLI writes it in folder names: anything but letters and
/// digits becomes `-`.
fn slug(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
}

/// The last error MCP server `server` logged in folder `cwd`, from its newest
/// log; `None` if it has none (or logged no error). Blocks on a small read.
pub fn last_error(cwd: &Path, server: &str) -> Option<String> {
    let dir = cache_root()?.join(slug(&cwd.to_string_lossy())).join(format!("mcp-logs-{}", slug(server)));
    let newest = std::fs::read_dir(&dir)
        .ok()?
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl" || x == "txt"))
        .max_by_key(|e| e.file_name())?;
    let text = std::fs::read_to_string(newest.path()).ok()?;
    last_error_in(&text)
}

/// The last `error` in a log's lines, tidied: no `Server stderr:` prefix,
/// blank lines dropped, at most 800 characters.
pub(crate) fn last_error_in(text: &str) -> Option<String> {
    let raw = text
        .lines()
        .rev()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .find_map(|v| v["error"].as_str().map(str::to_string))?;
    let raw = raw.strip_prefix("Server stderr:").unwrap_or(&raw);
    let tidy: Vec<&str> = raw.lines().map(str::trim_end).filter(|l| !l.trim().is_empty()).collect();
    let mut out = tidy.join("\n").trim().to_string();
    if out.chars().count() > 800 {
        out = out.chars().take(800).collect::<String>() + "…";
    }
    (!out.is_empty()).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_last_error() {
        let log = r#"{"debug":"Starting connection","timestamp":"t"}
{"error":"Server stderr: first\r\n","timestamp":"t"}
{"debug":"retrying"}
{"error":"Server stderr: npm ERR! 404 Not Found\r\n\r\nnpm ERR! @nope/mcp is not in this registry.\r\n","timestamp":"t"}
not json
"#;
        assert_eq!(
            last_error_in(log).as_deref(),
            Some("npm ERR! 404 Not Found\nnpm ERR! @nope/mcp is not in this registry.")
        );
        assert_eq!(last_error_in(r#"{"debug":"fine"}"#), None);
        assert_eq!(last_error_in(r#"{"error":"   "}"#), None);
        let long = format!(r#"{{"error":"{}"}}"#, "x".repeat(900));
        assert_eq!(last_error_in(&long).unwrap().chars().count(), 801);
        assert_eq!(slug("claude.ai Figma"), "claude-ai-Figma");
        assert_eq!(slug(r"C:\Users\me\repo"), "C--Users-me-repo");
    }
}
