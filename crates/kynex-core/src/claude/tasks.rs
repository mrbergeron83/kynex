//! Background tasks: `run_in_background` commands, background agents and
//! Monitors a session started. The CLI reports them over stream-json as
//! `system` messages (`task_started`, `task_updated`, `task_progress`,
//! `task_notification`); session logs keep only the `<task-notification>`
//! text it hands Claude when one ends, so a replay reads that instead.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde_json::Value;

use crate::terminal::ShellKind;

/// How a task ended, or `Event` for a notice from one still running (a Monitor's).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Completed,
    Failed,
    /// Killed or stopped, by the user or by Claude.
    Stopped,
    Event,
}

impl Status {
    fn parse(s: &str) -> Self {
        match s {
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            "killed" | "stopped" => Self::Stopped,
            _ => Self::Event,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Completed => "Completed",
            Self::Failed => "Failed",
            Self::Stopped => "Stopped",
            Self::Event => "Event",
        }
    }
}

/// A background task still running.
#[derive(Clone, Debug)]
pub struct Task {
    pub id: String,
    /// The tool call that started it.
    pub tool_use_id: String,
    pub description: String,
    /// The CLI's `task_type`: `local_bash`, `local_agent`, `monitor`…
    pub kind: String,
    /// Where its output goes, once known; follow it to watch the task live.
    pub output_file: Option<PathBuf>,
    pub started: Instant,
    /// An agent's latest tool, from its progress reports.
    pub last_tool: Option<String>,
    /// Asked to stop; it leaves `running` once the CLI reports it stopped.
    pub stopping: bool,
}

/// A task ended (or a Monitor saw something), for the transcript.
#[derive(Clone, Debug, PartialEq)]
pub struct Notice {
    pub task_id: String,
    pub tool_use_id: String,
    pub status: Status,
    /// The CLI's one line, e.g. `Background command "Run tests" completed (exit code 0)`.
    pub summary: String,
    pub output_file: Option<PathBuf>,
    /// How long it ran, when this run saw it start.
    pub duration_ms: Option<u64>,
}

/// A session's running background tasks.
#[derive(Default)]
pub struct Tasks {
    pub running: Vec<Task>,
    /// Output files named by tool results before their task was reported, by tool call.
    outputs: HashMap<String, PathBuf>,
}

impl Tasks {
    /// A `system` message about a task. Returns a notice when one ends or
    /// reports an event that belongs in the transcript.
    pub fn apply(&mut self, v: &Value) -> Option<Notice> {
        let id = v["task_id"].as_str()?.to_string();
        let tool_use_id = v["tool_use_id"].as_str().unwrap_or("").to_string();
        match v["subtype"].as_str()? {
            "task_started" => {
                // A foreground agent is part of the turn; it only counts once sent to the background.
                if v["is_backgrounded"] == false || self.running.iter().any(|t| t.id == id) {
                    return None;
                }
                let output_file = self.outputs.remove(&tool_use_id);
                self.running.push(Task {
                    id,
                    tool_use_id,
                    description: v["description"].as_str().unwrap_or("").to_string(),
                    kind: v["task_type"].as_str().unwrap_or("").to_string(),
                    output_file,
                    started: Instant::now(),
                    last_tool: None,
                    stopping: false,
                });
                None
            }
            "task_updated" => {
                let patch = &v["patch"];
                if let Some(t) = self.running.iter_mut().find(|t| t.id == id)
                    && let Some(d) = patch["description"].as_str()
                {
                    t.description = d.to_string();
                }
                None
            }
            "task_progress" => {
                if let Some(t) = self.running.iter_mut().find(|t| t.id == id) {
                    t.last_tool = v["last_tool_name"].as_str().map(str::to_string).or(t.last_tool.take());
                }
                None
            }
            "task_notification" => {
                let status = Status::parse(v["status"].as_str().unwrap_or(""));
                let task = self.running.iter().position(|t| t.id == id);
                let task = if status == Status::Event {
                    task.map(|i| self.running[i].clone())
                } else {
                    task.map(|i| self.running.remove(i))
                };
                if v["skip_transcript"] == true {
                    return None;
                }
                let output_file = v["output_file"]
                    .as_str()
                    .filter(|f| !f.is_empty())
                    .map(PathBuf::from)
                    .or_else(|| task.as_ref().and_then(|t| t.output_file.clone()));
                let duration_ms = (status != Status::Event)
                    .then(|| task.as_ref().map(|t| t.started.elapsed().as_millis() as u64))
                    .flatten()
                    .or_else(|| v["usage"]["duration_ms"].as_u64());
                Some(Notice {
                    task_id: id,
                    tool_use_id: if tool_use_id.is_empty() {
                        task.map(|t| t.tool_use_id).unwrap_or_default()
                    } else {
                        tool_use_id
                    },
                    status,
                    summary: v["summary"].as_str().unwrap_or("").to_string(),
                    output_file,
                    duration_ms,
                })
            }
            _ => None,
        }
    }

    /// A tool result: a background command's names the file its output goes to.
    pub fn tool_result(&mut self, tool_use_id: &str, content: &str) {
        let Some(path) = output_path(content) else { return };
        match self.running.iter_mut().find(|t| t.tool_use_id == tool_use_id) {
            Some(t) => t.output_file = Some(path),
            None => {
                self.outputs.insert(tool_use_id.to_string(), path);
            }
        }
    }

    /// The process ended: its tasks went with it.
    pub fn clear(&mut self) {
        self.running.clear();
        self.outputs.clear();
    }
}

/// `…Output is being written to: <path>. You will be notified…`, from a
/// `run_in_background` command's tool result.
fn output_path(content: &str) -> Option<PathBuf> {
    let rest = content.split_once("Output is being written to: ")?.1;
    let rest = rest.lines().next().unwrap_or("");
    // The path itself can hold ". ", as in `C:\Users\J. Doe\…`.
    let path = rest.split_once(". You ").map_or(rest, |(p, _)| p).trim().trim_end_matches('.');
    (!path.is_empty()).then(|| PathBuf::from(path))
}

/// A terminal command that shows the end of a task's output, and with
/// `follow` keeps streaming it until Ctrl+C: PowerShell on Windows, the login
/// shell elsewhere.
pub fn output_command(path: &Path, follow: bool) -> (ShellKind, String) {
    let p = path.to_string_lossy();
    if cfg!(windows) {
        let wait = if follow { " -Wait" } else { "" };
        (ShellKind::PowerShell, format!("Get-Content -LiteralPath '{}' -Tail 200{wait}", p.replace('\'', "''")))
    } else {
        let f = if follow { " -F" } else { "" };
        (ShellKind::Bash, format!("tail -n 200{f} '{}'", p.replace('\'', r"'\''")))
    }
}

/// The `<task-notification>` a session log keeps for Claude, as a notice.
/// `None` if `text` isn't one.
pub fn parse_notification(text: &str) -> Option<Notice> {
    let body = text.trim_start().strip_prefix("<task-notification>")?;
    let tag = |name: &str| {
        let open = format!("<{name}>");
        let close = format!("</{name}>");
        let start = body.find(&open)? + open.len();
        let len = body[start..].find(&close)?;
        Some(body[start..start + len].trim().to_string())
    };
    Some(Notice {
        task_id: tag("task-id")?,
        tool_use_id: tag("tool-use-id").unwrap_or_default(),
        status: Status::parse(&tag("status").unwrap_or_default()),
        summary: tag("summary").unwrap_or_default(),
        output_file: tag("output-file").filter(|f| !f.is_empty()).map(PathBuf::from),
        duration_ms: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn output_paths_may_hold_a_full_stop() {
        let p = |s| output_path(s).map(|p| p.to_string_lossy().into_owned());
        assert_eq!(
            p(
                "ID: b1. Output is being written to: C:\\Users\\J. Doe\\b1.output. You will be notified when it completes."
            ),
            Some("C:\\Users\\J. Doe\\b1.output".into())
        );
        assert_eq!(p("Output is being written to: /tmp/b2.output.\nMore"), Some("/tmp/b2.output".into()));
        assert_eq!(p("Output is being written to: "), None);
    }

    #[test]
    fn follows_a_background_command_to_its_end() {
        let mut tasks = Tasks::default();
        // The tool result can come before or after the start report.
        tasks.tool_result(
            "toolu_1",
            "Command running in background with ID: b1. Output is being written to: C:\\tmp\\tasks\\b1.output. You will be notified when it completes.",
        );
        tasks.apply(&json!({ "type": "system", "subtype": "task_started", "task_id": "b1", "tool_use_id": "toolu_1",
            "description": "Run the tests", "task_type": "local_bash", "is_backgrounded": true }));
        assert_eq!(tasks.running.len(), 1);
        assert_eq!(tasks.running[0].output_file, Some(PathBuf::from("C:\\tmp\\tasks\\b1.output")));

        tasks
            .apply(&json!({ "subtype": "task_updated", "task_id": "b1", "patch": { "description": "Run all tests" } }));
        assert_eq!(tasks.running[0].description, "Run all tests");

        let notice = tasks
            .apply(&json!({ "type": "system", "subtype": "task_notification", "task_id": "b1", "tool_use_id": "toolu_1",
                "status": "failed", "output_file": "", "summary": "Background command \"Run all tests\" failed (exit code 1)" }))
            .unwrap();
        assert!(tasks.running.is_empty());
        assert_eq!(notice.status, Status::Failed);
        assert_eq!(notice.output_file, Some(PathBuf::from("C:\\tmp\\tasks\\b1.output")), "kept from the start");
        assert!(notice.duration_ms.is_some());
    }

    #[test]
    fn foreground_agents_and_hidden_notices_stay_out() {
        let mut tasks = Tasks::default();
        tasks.apply(&json!({ "subtype": "task_started", "task_id": "a1", "is_backgrounded": false }));
        assert!(tasks.running.is_empty());
        tasks.apply(&json!({ "subtype": "task_started", "task_id": "a2", "task_type": "local_agent" }));
        tasks.apply(&json!({ "subtype": "task_progress", "task_id": "a2", "last_tool_name": "Grep" }));
        assert_eq!(tasks.running[0].last_tool.as_deref(), Some("Grep"));
        let hidden = tasks.apply(&json!({ "subtype": "task_notification", "task_id": "a2", "status": "completed",
            "skip_transcript": true }));
        assert!(hidden.is_none() && tasks.running.is_empty());
    }

    #[test]
    fn a_monitor_event_leaves_it_running() {
        let mut tasks = Tasks::default();
        tasks.apply(&json!({ "subtype": "task_started", "task_id": "m1", "task_type": "monitor" }));
        let n = tasks
            .apply(&json!({ "subtype": "task_notification", "task_id": "m1", "summary": "Monitor event: \"CI\"" }));
        assert_eq!(n.unwrap().status, Status::Event);
        assert_eq!(tasks.running.len(), 1);
    }

    #[test]
    fn reads_a_logged_notification() {
        let text = "<task-notification>\n<task-id>bkv1</task-id>\n<tool-use-id>toolu_01R</tool-use-id>\n\
                    <output-file>C:\\Temp\\claude\\x\\tasks\\bkv1.output</output-file>\n<status>completed</status>\n\
                    <summary>Background command \"Run the suite\" completed (exit code 0)</summary>\n</task-notification>";
        let n = parse_notification(text).unwrap();
        assert_eq!(n.task_id, "bkv1");
        assert_eq!(n.tool_use_id, "toolu_01R");
        assert_eq!(n.status, Status::Completed);
        assert_eq!(n.summary, "Background command \"Run the suite\" completed (exit code 0)");
        assert_eq!(n.output_file, Some(PathBuf::from("C:\\Temp\\claude\\x\\tasks\\bkv1.output")));
        // A Monitor's event has no status or tool call.
        let ev = parse_notification(
            "<task-notification>\n<task-id>m</task-id>\n<summary>Monitor event: \"x\"</summary>\n</task-notification>",
        )
        .unwrap();
        assert_eq!(ev.status, Status::Event);
        assert!(parse_notification("check CI").is_none());
    }

    #[test]
    fn follows_output_in_the_platform_shell() {
        let path = Path::new("/tmp/it's.output");
        let (kind, cmd) = output_command(path, true);
        if cfg!(windows) {
            assert_eq!(kind, ShellKind::PowerShell);
            assert_eq!(cmd, "Get-Content -LiteralPath '/tmp/it''s.output' -Tail 200 -Wait");
            assert_eq!(output_command(path, false).1, "Get-Content -LiteralPath '/tmp/it''s.output' -Tail 200");
        } else {
            assert_eq!(kind, ShellKind::Bash);
            assert_eq!(cmd, r"tail -n 200 -F '/tmp/it'\''s.output'");
            assert_eq!(output_command(path, false).1, r"tail -n 200 '/tmp/it'\''s.output'");
        }
    }
}
