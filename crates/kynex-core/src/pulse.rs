//! What a busy session is doing while nothing in its transcript shows it
//! (#157, #177): the process starting, the wait for Claude's first output,
//! the gaps between blocks. The app draws it as one quiet line at the end of
//! the transcript; [`crate::session::Session::pulse`] says when.

use std::time::Instant;

use serde_json::{Map, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Activity {
    /// The `claude` process hasn't answered yet.
    Starting,
    /// The message went; nothing has come back yet.
    Waiting,
    /// Between steps: after a block, after a tool's result.
    Working,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pulse {
    pub activity: Activity,
    /// When this stretch began.
    pub since: Instant,
}

impl Pulse {
    /// "Starting Claude…", "Waiting for Claude… 3s", "Working… 1m 4s".
    pub fn label(&self) -> String {
        let what = match self.activity {
            Activity::Starting => "Starting Claude…",
            Activity::Waiting => "Waiting for Claude…",
            Activity::Working => "Working…",
        };
        let took = self.since.elapsed();
        if took.as_secs() == 0 { what.into() } else { format!("{what} {}", crate::fmt::elapsed(took)) }
    }
}

/// The input keys a call's summary is made from (`session::tool_summary`).
const SUMMARY_KEYS: [&str; 7] = ["description", "file_path", "notebook_path", "command", "pattern", "url", "query"];

/// The summary fields already complete in a tool call's input as it streams
/// in (`{"file_path": "src/a.rs", "content": "…`), as an object; `None` until
/// one is. Only top-level string values count; a key that's merely inside
/// another value may be misread, which the full input corrects.
pub fn partial_input(json: &str) -> Option<Value> {
    let mut found = Map::new();
    for key in SUMMARY_KEYS {
        if let Some(v) = string_after(json, key) {
            found.insert(key.into(), Value::String(v));
        }
    }
    (!found.is_empty()).then_some(Value::Object(found))
}

/// The complete string value of `"key":` in `json`, if it's there yet.
fn string_after(json: &str, key: &str) -> Option<String> {
    let quoted = format!("\"{key}\"");
    let at = json.find(&quoted)? + quoted.len();
    let rest = json[at..].trim_start().strip_prefix(':')?.trim_start();
    if !rest.starts_with('"') {
        return None;
    }
    // Find the closing quote, skipping escaped characters.
    let mut escaped = false;
    for (i, c) in rest.char_indices().skip(1) {
        match c {
            _ if escaped => escaped = false,
            '\\' => escaped = true,
            '"' => return serde_json::from_str(&rest[..=i]).ok(),
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::json;

    use super::*;

    #[test]
    fn labels_count_up_after_a_second() {
        let now = Pulse { activity: Activity::Starting, since: Instant::now() };
        assert_eq!(now.label(), "Starting Claude…");
        let since = Instant::now() - Duration::from_secs(3);
        assert_eq!(Pulse { activity: Activity::Waiting, since }.label(), "Waiting for Claude… 3s");
        assert_eq!(Pulse { activity: Activity::Working, since }.label(), "Working… 3s");
    }

    #[test]
    fn partial_input_reads_complete_fields_only() {
        assert_eq!(partial_input(""), None);
        assert_eq!(partial_input(r#"{"file_pa"#), None);
        assert_eq!(partial_input(r#"{"file_path": "src/a"#), None);
        assert_eq!(
            partial_input(r#"{"file_path": "src/a.rs", "content": "fn main() {"#),
            Some(json!({ "file_path": "src/a.rs" }))
        );
        // Escapes are kept whole, and decoded.
        assert_eq!(partial_input(r#"{"command":"echo \"hi\" \\","desc"#), Some(json!({ "command": "echo \"hi\" \\" })));
        assert_eq!(partial_input(r#"{"timeout": 5, "command": "ls"}"#), Some(json!({ "command": "ls" })));
        // Not a string: nothing to read.
        assert_eq!(partial_input(r#"{"command": 5}"#), None);
        // Multi-byte text, cut anywhere, never panics.
        let full = r#"{"file_path": "dé/ü.rs", "x": "ß"}"#;
        for i in (0..=full.len()).filter(|&i| full.is_char_boundary(i)) {
            let _ = partial_input(&full[..i]);
        }
    }
}
