//! What a session does for the composer's CLI features: `!` commands in its
//! transcript and sent with the next message, the thinking switch, and bypass mode.

use serde_json::Value;

use super::{Item, Kind, Session};
use crate::composer::shell::{self, Output, Part, Run};
use crate::composer::thinking;
use crate::project::{self, Mode};

impl Session {
    /// Shows `!` command `command` as running; returns the id to finish it with.
    pub fn start_shell(&mut self, command: String) -> u64 {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.items.push(Item::Shell(Run { id, command, output: None, sent: false }));
        self.revision += 1;
        id
    }

    /// `!` command `id` finished with `output`; it goes to Claude with the next message.
    pub fn finish_shell(&mut self, id: u64, output: Output) {
        for item in self.items.iter_mut().rev() {
            if let Item::Shell(run) = item
                && run.id == id
            {
                run.output = Some(output);
                self.revision += 1;
                return;
            }
        }
    }

    /// The blocks of the finished `!` commands Claude hasn't seen, now marked seen.
    pub(super) fn unsent_shell_blocks(&mut self) -> Vec<String> {
        let mut blocks = Vec::new();
        for item in &mut self.items {
            if let Item::Shell(run) = item
                && !run.sent
                && run.output.is_some()
            {
                blocks.extend(shell::blocks(run));
                run.sent = true;
            }
        }
        blocks
    }

    /// Takes a logged user message's text block in if it's a `!` command or its
    /// output; says whether it was.
    pub(super) fn replay_shell(&mut self, text: &str) -> bool {
        match shell::parse(text) {
            Some(Part::Input(command)) => {
                self.items.push(Item::Shell(Run { id: 0, command, output: None, sent: true }));
            }
            Some(Part::Output(out)) => match self.items.last_mut() {
                Some(Item::Shell(run)) if run.output.is_none() => run.output = Some(out),
                _ => self.items.push(Item::Shell(Run { id: 0, command: String::new(), output: Some(out), sent: true })),
            },
            None => return false,
        }
        true
    }

    /// Switches thinking on or off: live on a running `claude`, else from its next start.
    pub fn set_thinking(&mut self, on: bool) {
        self.thinking_off = !on;
        let harness = self.provider.provider() == crate::provider::Provider::OpenRouterChat;
        if self.is_running() && self.acp.is_none() && self.kind == Kind::Code && !harness {
            self.control(thinking::request(on));
        }
    }

    /// Whether bypass mode can be switched to: it's on in Settings, and a running
    /// process was started allowed to (the next start will be).
    pub fn can_bypass(&self) -> bool {
        self.kind == Kind::Code && project::bypass_allowed() && (self.bypass || !self.is_running())
    }

    /// Before a start: whether the process may bypass permissions, and out of bypass
    /// mode (to the project's) if it may not.
    pub(super) fn bypass_on_start(&mut self) {
        self.bypass = self.kind == Kind::Code && project::bypass_allowed();
        if self.mode == Mode::Bypass && !self.bypass {
            let default = project::ClaudeSettings::load(self.settings_folder()).mode;
            self.mode = if default == Mode::Bypass { Mode::Manual } else { default };
        }
    }

    /// The answer to a `set_permission_mode`: a refusal puts the mode back and says why.
    pub(super) fn mode_answered(&mut self, response: &Value) {
        let Some(at) = self.mode_requests.iter().position(|(id, _)| response["request_id"] == id.as_str()) else {
            return;
        };
        let (_, before) = self.mode_requests.remove(at);
        if response["subtype"] == "error" {
            self.mode = before;
            let why = response["error"].as_str().unwrap_or("the CLI refused");
            self.items.push(Item::Error(format!("Couldn't switch mode: {why}")));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::mpsc::channel;

    use serde_json::{Value, json};

    use super::*;

    /// A session whose process is "starting": what it writes waits in `outbox`.
    fn starting() -> Session {
        let mut s = Session::new(Kind::Code, PathBuf::from("/nowhere"));
        let (_tx, rx) = channel();
        s.starting = Some(rx);
        s
    }

    fn written(s: &Session) -> Vec<Value> {
        s.outbox.iter().map(|l| serde_json::from_str(l).unwrap()).collect()
    }

    #[test]
    fn shell_output_goes_with_the_next_message() {
        let mut s = starting();
        let id = s.start_shell("ls".into());
        let running = s.start_shell("sleep 99".into());
        s.finish_shell(id, Output { stdout: "a.txt".into(), stderr: String::new(), code: Some(0) });
        s.send("what's here?".into(), Vec::new());
        let msg = written(&s).pop().unwrap();
        let texts: Vec<&str> =
            msg["message"]["content"].as_array().unwrap().iter().map(|b| b["text"].as_str().unwrap()).collect();
        assert_eq!(
            texts,
            [
                "<bash-input>ls</bash-input>",
                "<bash-stdout>a.txt</bash-stdout><bash-stderr></bash-stderr><bash-exit-code>0</bash-exit-code>",
                "what's here?"
            ]
        );
        // The one still running goes with a later message, and each goes once.
        s.finish_shell(running, Output::default());
        s.busy = false;
        s.send("and now?".into(), Vec::new());
        let msg = written(&s).pop().unwrap();
        assert_eq!(msg["message"]["content"].as_array().unwrap().len(), 3);
        assert!(msg["message"]["content"][0]["text"].as_str().unwrap().contains("sleep 99"));
    }

    #[test]
    fn a_resumed_log_shows_shell_runs() {
        let mut s = Session::new(Kind::Code, PathBuf::from("/nowhere"));
        let user = |content: Value| json!({ "type": "user", "message": { "role": "user", "content": content } });
        // The CLI's own `!`: two messages.
        s.replay_entry(user(json!("<bash-input>git status</bash-input>")));
        s.replay_entry(user(json!("<bash-stdout>clean</bash-stdout><bash-stderr></bash-stderr>")));
        // Kynex's: blocks in front of the message.
        s.replay_entry(user(json!([
            { "type": "text", "text": "<bash-input>ls</bash-input>" },
            { "type": "text", "text": "<bash-stdout></bash-stdout><bash-stderr>no</bash-stderr><bash-exit-code>1</bash-exit-code>" },
            { "type": "text", "text": "why?" },
        ])));
        let shells: Vec<&Run> =
            s.items.iter().filter_map(|i| if let Item::Shell(r) = i { Some(r) } else { None }).collect();
        assert_eq!(shells.len(), 2);
        assert_eq!(shells[0].command, "git status");
        assert_eq!(shells[0].output.as_ref().unwrap().stdout, "clean");
        assert!(shells[1].output.as_ref().unwrap().failed());
        assert!(shells.iter().all(|r| r.sent));
        assert!(matches!(s.items.last(), Some(Item::User { text, .. }) if text == "why?"));
    }

    #[test]
    fn thinking_switches_live_and_on_start() {
        let mut s = starting();
        s.set_thinking(false);
        assert_eq!(written(&s).pop().unwrap()["request"], thinking::request(false));
        assert!(s.thinking_off);
        s.set_thinking(true);
        assert_eq!(written(&s).pop().unwrap()["request"]["max_thinking_tokens"], Value::Null);
    }

    #[test]
    fn ultrathink_typed_only_when_the_cli_takes_it() {
        let mut s = starting();
        s.send_with("ultrathink it".into(), Vec::new(), true);
        assert!(written(&s).pop().unwrap().get("ultrathink_typed").is_none());
        s.busy = false;
        s.handle(json!({ "type": "system", "subtype": "init", "ultrathink_available": true, "ultrathink_typed_raises": true }));
        s.send_with("ultrathink it".into(), Vec::new(), true);
        assert_eq!(written(&s).pop().unwrap()["ultrathink_typed"], true);
    }

    #[test]
    fn a_refused_mode_switch_goes_back() {
        let mut s = starting();
        s.mode = Mode::Auto;
        s.set_mode(Mode::Bypass);
        let id = written(&s).pop().unwrap()["request_id"].as_str().unwrap().to_string();
        s.handle(json!({ "type": "control_response",
            "response": { "subtype": "error", "request_id": id, "error": "not launched with it" } }));
        assert_eq!(s.mode, Mode::Auto);
        assert!(matches!(s.items.last(), Some(Item::Error(e)) if e.contains("not launched with it")));
        // A switch that went through stays.
        s.set_mode(Mode::Plan);
        let id = written(&s).pop().unwrap()["request_id"].as_str().unwrap().to_string();
        s.handle(json!({ "type": "control_response", "response": { "subtype": "success", "request_id": id } }));
        assert_eq!(s.mode, Mode::Plan);
    }

    #[test]
    fn bypass_needs_the_setting_at_start() {
        let mut s = Session::new(Kind::Code, PathBuf::from("/nowhere"));
        s.mode = Mode::Bypass;
        // Not allowed (the default): it starts in the project's mode instead.
        s.bypass_on_start();
        assert_ne!(s.mode, Mode::Bypass);
        assert!(!s.can_bypass());
    }
}
