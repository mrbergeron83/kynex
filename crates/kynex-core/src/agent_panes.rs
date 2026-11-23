//! Agent CLIs in terminal panes: screen-pattern rules (`agent_rules/*.json`, user overrides in
//! `<data dir>/agents/`) plus a hybrid [`Tracker`] that takes authoritative native hook events
//! when a CLI supports them (`agy`) and falls back to screen-pattern detection for CLIs without
//! hooks (`grok`, `codex`, `copilot`, `cursor`, `opencode`, custom rules).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::hooks::{HookReport, HookState};
use crate::paths::data_dir;

const BUILT_IN: [&str; 6] = [
    include_str!("agent_rules/grok.json"),
    include_str!("agent_rules/agy.json"),
    include_str!("agent_rules/codex.json"),
    include_str!("agent_rules/copilot.json"),
    include_str!("agent_rules/cursor.json"),
    include_str!("agent_rules/opencode.json"),
];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentRule {
    /// What a pane and an override file call it (`codex`, `claude`, `grok`, `agy`…).
    pub id: String,
    pub name: String,
    /// How it starts, as an argv.
    pub argv: Vec<String>,
    /// Text on the screen while it works.
    #[serde(default)]
    pub working: Vec<String>,
    /// Text on the screen while it waits on you (a permission, a question).
    #[serde(default)]
    pub blocked: Vec<String>,
    /// Text that means a turn is over, for agents that say so.
    #[serde(default)]
    pub done: Vec<String>,
    /// What comes right before the id in the line it prints to be resumed (`codex resume `).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_after: Option<String>,
    /// How to resume, `{id}` standing for the id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_argv: Option<Vec<String>>,
    /// The key that sends a typed message (`knx pane prompt`), as `enter`, `ctrl-enter`,
    /// `alt-enter`…; Enter if not said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submit: Option<String>,
    /// Delay after pasting before pressing submit, in milliseconds (80 if not said).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submit_after_ms: Option<u64>,
    /// How long it may take to start working on a message before a wait gives up, in seconds (120).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_within_secs: Option<u64>,
}

impl AgentRule {
    /// The bytes of its [`submit`](Self::submit) key.
    pub fn submit_bytes(&self) -> Vec<u8> {
        let name = self.submit.as_deref().unwrap_or("enter").to_ascii_lowercase();
        let mut parts: Vec<&str> = name.split('-').collect();
        let key = parts.pop().unwrap_or("enter");
        let k = crate::terminal::Key {
            key,
            text: None,
            ctrl: parts.contains(&"ctrl"),
            alt: parts.contains(&"alt"),
            shift: parts.contains(&"shift"),
        };
        crate::terminal::key_bytes(&k, crate::terminal::KeyMode::default()).unwrap_or_else(|| b"\r".to_vec())
    }

    pub fn submit_after(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.submit_after_ms.unwrap_or(80))
    }

    pub fn start_within(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.start_within_secs.unwrap_or(120))
    }
}

/// What the screen says now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Seen {
    Working,
    Blocked,
    Done,
    Quiet,
}

/// An `agent_cli` pane's live state, shown on lamps and in the Agents pane.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AgentState {
    #[default]
    Idle,
    Working,
    NeedsYou,
    /// A turn ended and you haven't looked at it yet.
    Done,
}

/// The rule in `json`. `Err` if it isn't one, or has no id, name or program.
pub fn parse(json: &str) -> Result<AgentRule, String> {
    let mut r: AgentRule = serde_json::from_str(json).map_err(|e| format!("Couldn't read the agent rule: {e}"))?;
    r.id = r.id.trim().to_string();
    r.name = r.name.trim().to_string();
    let blank = |v: &mut Vec<String>| v.retain(|p| !p.trim().is_empty());
    blank(&mut r.working);
    blank(&mut r.blocked);
    blank(&mut r.done);
    if r.id.is_empty() || r.name.is_empty() || r.argv.first().is_none_or(|p| p.trim().is_empty()) {
        return Err("An agent rule needs an id, a name and a program.".into());
    }
    Ok(r)
}

/// The folder of the user's rules.
pub fn user_dir() -> PathBuf {
    data_dir().join("agents")
}

/// Every agent rule: the built-in rules with the user's from `dir` over them by id, sorted by
/// name. A file that doesn't read is left out and named in the errors. Blocks on the folder.
pub fn rules(dir: &Path) -> (Vec<AgentRule>, Vec<String>) {
    let mut all: Vec<AgentRule> = BUILT_IN.iter().filter_map(|s| parse(s).ok()).collect();
    let mut errors = Vec::new();
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|d| d.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "json")).collect())
        .unwrap_or_default();
    files.sort();
    for f in files {
        match std::fs::read_to_string(&f).map_err(|e| e.to_string()).and_then(|s| parse(&s)) {
            Ok(r) => match all.iter_mut().find(|a| a.id == r.id) {
                Some(a) => *a = r,
                None => all.push(r),
            },
            Err(e) => errors.push(format!("{}: {e}", f.display())),
        }
    }
    all.sort_by_key(|a| a.name.to_lowercase());
    (all, errors)
}

/// What `screen` (a terminal's text) says the agent is doing.
pub fn detect(rule: &AgentRule, screen: &str) -> Seen {
    let text = screen.to_lowercase();
    let any = |ps: &[String]| ps.iter().any(|p| text.contains(&p.to_lowercase()));
    if any(&rule.blocked) {
        Seen::Blocked
    } else if any(&rule.working) {
        Seen::Working
    } else if any(&rule.done) {
        Seen::Done
    } else {
        Seen::Quiet
    }
}

/// The id the agent printed to be resumed with, the last one on `screen`.
pub fn resume_id(rule: &AgentRule, screen: &str) -> Option<String> {
    let after = rule.resume_after.as_deref().filter(|a| !a.is_empty())?;
    let at = screen.rfind(after)? + after.len();
    let id: String =
        screen[at..].chars().take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')).collect();
    (!id.is_empty()).then_some(id)
}

/// The argv that resumes session `id`, or the plain one if the agent can't resume.
pub fn resume_argv(rule: &AgentRule, id: Option<&str>) -> Vec<String> {
    match (&rule.resume_argv, id) {
        (Some(argv), Some(id)) if !argv.is_empty() => argv.iter().map(|a| a.replace("{id}", id)).collect(),
        _ => rule.argv.clone(),
    }
}

/// Result of applying a native CLI hook to [`Tracker`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HookOutcome {
    pub state: AgentState,
    /// The pane's primary (top-level) session ID when newly learned or switched to another
    /// top-level session; `None` when unchanged or when the hook came from a subagent.
    pub session_id: Option<String>,
}

/// Follows an `agent_cli` pane's state via native hooks (when available) and screen fallback.
#[derive(Clone, Debug, Default)]
pub struct Tracker {
    state: AgentState,
    /// Short description of what the agent is doing ("Using Bash", "Needs permission", "Finished").
    detail: String,
    /// True once this pane has received at least one native hook event.
    hooked: bool,
    /// Screen detected `Blocked` while hooked (cleared once the prompt leaves the screen).
    screen_blocked: bool,
    /// Looks in a row with no sign of work since it was working (for screen-only tracking).
    quiet: u8,
    /// The pane's top-level session ID, used to separate parent hooks from child/subagent hooks
    /// on CLIs like Antigravity where subagents open separate `conversationId`s.
    primary_session: Option<String>,
    /// Whether the primary session itself is currently inside an active turn.
    parent_working: bool,
    /// Whether the primary session paused its turn with background tasks still running (`fullyIdle: false`).
    parent_waiting_bg: bool,
    /// Subagent session IDs seen under `primary_session` so late or re-woken subagent hooks never
    /// hijack the primary session.
    known_subagents: HashSet<String>,
    /// Subagent session IDs currently running (between start/work and their `Stop`).
    active_subagents: HashSet<String>,
}

/// Looks in a row without the working text before a turn counts as over on screen-only CLIs.
pub const QUIET_LOOKS: u8 = 3;

fn subagent_summary(count: usize, action: Option<&str>) -> String {
    match (count, action) {
        (0 | 1, Some(a)) => format!("Subagent · {a}"),
        (0 | 1, None) => "Running subagent".into(),
        (n, Some(a)) => format!("{n} subagents · {a}"),
        (n, None) => format!("Running {n} subagents"),
    }
}

impl Tracker {
    pub fn state(&self) -> AgentState {
        self.state
    }

    pub fn detail(&self) -> &str {
        &self.detail
    }

    pub fn is_hooked(&self) -> bool {
        self.hooked
    }

    /// Records the pane's known top-level session ID (e.g. when resuming or scraped from screen).
    pub fn set_primary_session(&mut self, id: String) {
        if self.primary_session.as_deref() != Some(id.as_str()) {
            self.primary_session = Some(id);
            self.known_subagents.clear();
            self.active_subagents.clear();
            self.parent_working = false;
            self.parent_waiting_bg = false;
        }
    }

    /// Marks the pane as seen by the user (focusing it clears `Done` → `Idle`).
    pub fn mark_seen(&mut self) -> bool {
        if self.state == AgentState::Done {
            self.state = AgentState::Idle;
            self.detail.clear();
            true
        } else {
            false
        }
    }

    /// Applies an authoritative native hook event. `looking` is whether the user is currently
    /// focused on this pane in the active window.
    pub fn on_hook(&mut self, report: &HookReport, looking: bool) -> AgentState {
        self.on_hook_for("", report, looking).state
    }

    /// Applies a native hook event for `provider`, distinguishing Antigravity subagent sessions
    /// from the pane's primary top-level session so subagents never overwrite session history
    /// while still keeping the agent marked as working in the Agents pane.
    pub fn on_hook_for(&mut self, provider: &str, report: &HookReport, looking: bool) -> HookOutcome {
        self.hooked = true;
        self.quiet = 0;

        let separate_subagent_sessions = matches!(crate::agent_cli::rule_id(provider), crate::agent_cli::AGY);
        let sid = report.session_id.as_deref();

        let from_subagent = if report.subagent {
            if let Some(id) = sid {
                self.known_subagents.insert(id.to_string());
            }
            true
        } else if separate_subagent_sessions {
            match (self.primary_session.as_deref(), sid) {
                (Some(primary), Some(id)) if id != primary => {
                    let is_child = self.known_subagents.contains(id)
                        || self.parent_working
                        || self.parent_waiting_bg
                        || !self.active_subagents.is_empty()
                        || matches!(self.state, AgentState::Working | AgentState::NeedsYou)
                        || report.state == Some(HookState::Done);
                    if is_child {
                        self.known_subagents.insert(id.to_string());
                    }
                    is_child
                }
                _ => false,
            }
        } else {
            false
        };

        if from_subagent {
            self.apply_subagent_hook(sid, report, looking);
            return HookOutcome { state: self.state, session_id: None };
        }

        self.screen_blocked = false;
        let mut session_id = None;
        if let Some(id) = sid
            && self.primary_session.as_deref() != Some(id)
        {
            self.primary_session = Some(id.to_string());
            self.known_subagents.clear();
            self.active_subagents.clear();
            self.parent_working = false;
            self.parent_waiting_bg = false;
            session_id = Some(id.to_string());
        }

        if let Some(next) = report.state {
            match next {
                HookState::Working => {
                    if report.background {
                        self.parent_working = false;
                        self.parent_waiting_bg = true;
                    } else {
                        self.parent_working = true;
                        self.parent_waiting_bg = false;
                    }
                    self.state = AgentState::Working;
                }
                HookState::NeedsYou => {
                    self.state = AgentState::NeedsYou;
                }
                HookState::Done => {
                    self.parent_working = false;
                    self.parent_waiting_bg = false;
                    self.active_subagents.clear();
                    self.state = if looking { AgentState::Idle } else { AgentState::Done };
                }
                HookState::Idle => {
                    self.parent_working = false;
                    self.parent_waiting_bg = false;
                    self.active_subagents.clear();
                    self.state = AgentState::Idle;
                }
            }
        }

        if report.background && !self.active_subagents.is_empty() {
            if !self.detail.contains("ubagent") {
                self.detail = subagent_summary(self.active_subagents.len(), None);
            }
        } else if let Some(d) = &report.detail {
            self.detail = d.clone();
        } else if self.state == AgentState::Idle {
            self.detail.clear();
        }

        HookOutcome { state: self.state, session_id }
    }

    fn apply_subagent_hook(&mut self, sid: Option<&str>, report: &HookReport, looking: bool) {
        match report.state {
            Some(HookState::Idle) => {
                // Subagent `SessionStart`: it just spawned; keep the pane Working and count it.
                if let Some(id) = sid {
                    self.active_subagents.insert(id.to_string());
                }
                if self.state != AgentState::NeedsYou {
                    self.state = AgentState::Working;
                    self.detail = subagent_summary(self.active_subagents.len().max(1), None);
                }
            }
            Some(HookState::Working) => {
                if let Some(id) = sid {
                    self.active_subagents.insert(id.to_string());
                }
                if self.state != AgentState::NeedsYou {
                    self.state = AgentState::Working;
                    let action = report.detail.as_deref().filter(|d| !d.is_empty());
                    self.detail = subagent_summary(self.active_subagents.len().max(1), action);
                }
            }
            Some(HookState::NeedsYou) => {
                if let Some(id) = sid {
                    self.active_subagents.insert(id.to_string());
                }
                self.state = AgentState::NeedsYou;
                if let Some(d) = &report.detail {
                    self.detail = d.clone();
                }
            }
            Some(HookState::Done) => {
                if let Some(id) = sid {
                    self.active_subagents.remove(id);
                }
                if self.state == AgentState::NeedsYou {
                    return;
                }
                if !self.active_subagents.is_empty() {
                    self.state = AgentState::Working;
                    self.detail = subagent_summary(self.active_subagents.len(), None);
                } else if self.parent_working || self.parent_waiting_bg {
                    self.state = AgentState::Working;
                    self.detail = "Thinking".into();
                } else if self.state == AgentState::Working {
                    // A background subagent ran while the parent was idle and has now finished.
                    self.state = if looking { AgentState::Idle } else { AgentState::Done };
                    if self.state == AgentState::Idle {
                        self.detail.clear();
                    } else {
                        self.detail = report.detail.clone().unwrap_or_else(|| "Finished".into());
                    }
                }
            }
            None => {}
        }
    }

    /// Updates state from a periodic screen scan. When native hooks have reported for this
    /// pane, screen scanning only catches `NeedsYou` permission prompts (e.g. `agy`) and
    /// clears `Done` when focused, leaving hook transitions authoritative.
    pub fn update(&mut self, seen: Seen, looking: bool) -> AgentState {
        if self.hooked {
            if seen == Seen::Blocked {
                self.screen_blocked = true;
                self.state = AgentState::NeedsYou;
                if self.detail.is_empty() {
                    self.detail = "Wants you".into();
                }
            } else if self.screen_blocked {
                self.screen_blocked = false;
                if self.state == AgentState::NeedsYou {
                    self.state = if seen == Seen::Working { AgentState::Working } else { AgentState::Idle };
                }
            }
            if looking && self.state == AgentState::Done {
                self.state = AgentState::Idle;
                self.detail.clear();
            }
            return self.state;
        }
        self.quiet = if seen == Seen::Quiet { self.quiet.saturating_add(1) } else { 0 };
        self.state = match seen {
            Seen::Blocked => AgentState::NeedsYou,
            Seen::Working => AgentState::Working,
            Seen::Done if !looking => AgentState::Done,
            Seen::Quiet if self.state == AgentState::Working && self.quiet < QUIET_LOOKS => AgentState::Working,
            Seen::Quiet if !looking && matches!(self.state, AgentState::Working | AgentState::Done) => AgentState::Done,
            _ => AgentState::Idle,
        };
        self.detail = match self.state {
            AgentState::NeedsYou => "Wants you".into(),
            AgentState::Done => "Finished".into(),
            AgentState::Working => "Working".into(),
            AgentState::Idle => String::new(),
        };
        self.state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codex() -> AgentRule {
        rules(Path::new("/no/such/dir")).0.into_iter().find(|r| r.id == "codex").unwrap()
    }

    #[test]
    fn a_message_is_sent_with_the_rules_key() {
        let (all, _) = rules(Path::new("/no/such/dir"));
        let mut rule = all.into_iter().next().unwrap();
        assert_eq!(rule.submit_bytes(), b"\r");
        assert_eq!(rule.submit_after(), std::time::Duration::from_millis(80));
        rule.submit = Some("alt-enter".into());
        assert_eq!(rule.submit_bytes(), b"\x1b\r");
        rule.submit = Some("tab".into());
        assert_eq!(rule.submit_bytes(), b"\t");
        rule.submit = Some("nonsense".into());
        assert_eq!(rule.submit_bytes(), b"\r");
    }

    #[test]
    fn the_built_in_rules_read() {
        let (all, errors) = rules(Path::new("/no/such/dir"));
        assert!(errors.is_empty());
        let ids: Vec<&str> = all.iter().map(|r| r.id.as_str()).collect();
        for id in ["grok", "agy", "codex", "copilot", "cursor", "opencode"] {
            assert!(ids.contains(&id), "{id}");
        }
        assert!(!ids.contains(&"claude"));
    }

    #[test]
    fn a_users_rule_replaces_the_built_in_one_and_adds_new_ones() {
        let dir = std::env::temp_dir().join(format!("kynex-agents-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("codex.json"), r#"{"id":"codex","name":"My Codex","argv":["codex","--x"]}"#).unwrap();
        std::fs::write(dir.join("aider.json"), r#"{"id":"aider","name":"Aider","argv":["aider"],"working":["…"]}"#)
            .unwrap();
        std::fs::write(dir.join("bad.json"), "{").unwrap();
        let (all, errors) = rules(&dir);
        let codex = all.iter().find(|r| r.id == "codex").unwrap();
        assert_eq!((codex.name.as_str(), codex.argv.len()), ("My Codex", 2));
        assert!(all.iter().any(|r| r.id == "aider"));
        assert_eq!(errors.len(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_rule_without_a_program_is_refused() {
        assert!(parse(r#"{"id":"x","name":"X","argv":[]}"#).is_err());
        assert!(parse(r#"{"id":" ","name":"X","argv":["x"]}"#).is_err());
        assert!(parse("[]").is_err());
    }

    #[test]
    fn the_screen_says_what_the_agent_does() {
        let r = AgentRule {
            id: "a".into(),
            name: "A".into(),
            argv: vec!["a".into()],
            working: vec!["Esc to interrupt".into()],
            blocked: vec!["Allow command?".into()],
            done: vec![],
            resume_after: None,
            resume_argv: None,
            submit: None,
            submit_after_ms: None,
            start_within_secs: None,
        };
        assert_eq!(detect(&r, "• Working (3s • esc to interrupt)"), Seen::Working);
        assert_eq!(detect(&r, "allow command?\n  esc to interrupt"), Seen::Blocked, "waiting on you wins");
        assert_eq!(detect(&r, "> "), Seen::Quiet);
    }

    #[test]
    fn a_turn_that_stops_working_is_done_until_looked_at() {
        let mut t = Tracker::default();
        assert_eq!(t.update(Seen::Quiet, false), AgentState::Idle);
        assert_eq!(t.update(Seen::Working, false), AgentState::Working);
        for _ in 1..QUIET_LOOKS {
            assert_eq!(t.update(Seen::Quiet, false), AgentState::Working, "a redraw isn't the end");
        }
        assert_eq!(t.update(Seen::Quiet, false), AgentState::Done);
        assert_eq!(t.update(Seen::Quiet, false), AgentState::Done, "still unseen");
        assert_eq!(t.update(Seen::Quiet, true), AgentState::Idle);
        assert_eq!(t.update(Seen::Blocked, true), AgentState::NeedsYou);
        t.update(Seen::Working, true);
        for _ in 0..QUIET_LOOKS {
            t.update(Seen::Quiet, true);
        }
        assert_eq!(t.state(), AgentState::Idle);
    }

    #[test]
    fn hybrid_tracker_trusts_hooks_while_allowing_screen_blocked() {
        let mut t = Tracker::default();
        let working = HookReport {
            state: Some(HookState::Working),
            session_id: None,
            detail: Some("Using Bash".into()),
            ..HookReport::default()
        };
        assert_eq!(t.on_hook(&working, false), AgentState::Working);
        assert_eq!(t.detail(), "Using Bash");
        // Quiet screen ticks must not prematurely end a hooked working turn.
        for _ in 0..10 {
            assert_eq!(t.update(Seen::Quiet, false), AgentState::Working);
        }
        // Screen-detected permission prompt can elevate to NeedsYou.
        assert_eq!(t.update(Seen::Blocked, false), AgentState::NeedsYou);
        // Clearing blocked screen restores working.
        assert_eq!(t.update(Seen::Working, false), AgentState::Working);
        // Hook Stop finishes the turn.
        let done = HookReport {
            state: Some(HookState::Done),
            session_id: None,
            detail: Some("Finished".into()),
            ..HookReport::default()
        };
        assert_eq!(t.on_hook(&done, false), AgentState::Done);
        assert!(t.mark_seen());
        assert_eq!(t.state(), AgentState::Idle);
    }

    #[test]
    fn agy_subagents_never_replace_primary_session_and_keep_agent_working() {
        let mut t = Tracker::default();
        let hook = |json: &str| crate::hooks::parse_payload("agy", None, json);

        // Parent starts and claims the pane's primary session.
        let out = t.on_hook_for("agy", &hook(r#"{"hookEventName":"SessionStart","conversationId":"parent"}"#), true);
        assert_eq!(out, HookOutcome { state: AgentState::Idle, session_id: Some("parent".into()) });

        // Parent starts working.
        let out = t.on_hook_for("agy", &hook(r#"{"hookEventName":"PreInvocation","conversationId":"parent"}"#), true);
        assert_eq!(out, HookOutcome { state: AgentState::Working, session_id: None });

        // Two subagents open their own conversations while parent is working.
        let out = t.on_hook_for("agy", &hook(r#"{"hookEventName":"SessionStart","conversationId":"sub-1"}"#), true);
        assert_eq!(out, HookOutcome { state: AgentState::Working, session_id: None });
        assert_eq!(t.detail(), "Running subagent");

        let out = t.on_hook_for("agy", &hook(r#"{"hookEventName":"SessionStart","conversationId":"sub-2"}"#), true);
        assert_eq!(out, HookOutcome { state: AgentState::Working, session_id: None });
        assert_eq!(t.detail(), "Running 2 subagents");

        let out = t.on_hook_for(
            "agy",
            &hook(r#"{"hookEventName":"PostToolUse","conversationId":"sub-1","toolCall":{"name":"generate_image"}}"#),
            true,
        );
        assert_eq!(out, HookOutcome { state: AgentState::Working, session_id: None });
        assert_eq!(t.detail(), "2 subagents · Used generate_image");

        // Parent yields while subagents are still running (`fullyIdle: false`).
        let out = t.on_hook_for(
            "agy",
            &hook(r#"{"hookEventName":"Stop","conversationId":"parent","terminationReason":"NO_TOOL_CALL","fullyIdle":false}"#),
            true,
        );
        assert_eq!(out, HookOutcome { state: AgentState::Working, session_id: None });
        assert_eq!(t.detail(), "2 subagents · Used generate_image");

        // Subagent 1 stops while subagent 2 is still running: stays Working.
        let out = t.on_hook_for(
            "agy",
            &hook(r#"{"hookEventName":"Stop","conversationId":"sub-1","terminationReason":"NO_TOOL_CALL","fullyIdle":true}"#),
            false,
        );
        assert_eq!(out, HookOutcome { state: AgentState::Working, session_id: None });
        assert_eq!(t.detail(), "Running subagent");

        // Subagent 2 stops while parent is waiting to wake up: stays Working.
        let out = t.on_hook_for(
            "agy",
            &hook(r#"{"hookEventName":"Stop","conversationId":"sub-2","terminationReason":"NO_TOOL_CALL","fullyIdle":true}"#),
            false,
        );
        assert_eq!(out, HookOutcome { state: AgentState::Working, session_id: None });
        assert_eq!(t.detail(), "Thinking");

        // Parent wakes up and finally completes with `fullyIdle: true`.
        let out = t.on_hook_for("agy", &hook(r#"{"hookEventName":"PreInvocation","conversationId":"parent"}"#), false);
        assert_eq!(out, HookOutcome { state: AgentState::Working, session_id: None });
        let out = t.on_hook_for(
            "agy",
            &hook(r#"{"hookEventName":"Stop","conversationId":"parent","terminationReason":"NO_TOOL_CALL","fullyIdle":true}"#),
            false,
        );
        assert_eq!(out, HookOutcome { state: AgentState::Done, session_id: None });
        assert_eq!(t.detail(), "Finished");

        // Late out-of-order Stop from a subagent after the user viewed the pane leaves it Idle.
        assert!(t.mark_seen());
        let out = t.on_hook_for(
            "agy",
            &hook(r#"{"hookEventName":"Stop","conversationId":"sub-1","terminationReason":"NO_TOOL_CALL"}"#),
            true,
        );
        assert_eq!(out, HookOutcome { state: AgentState::Idle, session_id: None });
    }

    #[test]
    fn agy_new_session_while_idle_switches_primary_but_subagent_never_does() {
        let mut t = Tracker::default();
        t.set_primary_session("parent-1".into());
        let hook = |json: &str| crate::hooks::parse_payload("agy-openrouter", None, json);

        // Starting a fresh top-level conversation (/new) while idle switches primary session.
        let out = t.on_hook_for(
            "agy-openrouter",
            &hook(r#"{"hookEventName":"SessionStart","conversationId":"parent-2"}"#),
            true,
        );
        assert_eq!(out, HookOutcome { state: AgentState::Idle, session_id: Some("parent-2".into()) });

        // An explicit subagent payload while idle still never replaces primary session.
        let out = t.on_hook_for(
            "agy-openrouter",
            &hook(r#"{"hookEventName":"PreInvocation","conversationId":"sub-a","parentConversationId":"parent-2"}"#),
            false,
        );
        assert_eq!(out, HookOutcome { state: AgentState::Working, session_id: None });
        let out = t.on_hook_for(
            "agy-openrouter",
            &hook(r#"{"hookEventName":"Stop","conversationId":"sub-a","terminationReason":"NO_TOOL_CALL"}"#),
            false,
        );
        assert_eq!(out, HookOutcome { state: AgentState::Done, session_id: None });
    }

    #[test]
    fn subagent_activity_does_not_hide_a_permission_prompt() {
        let mut t = Tracker::default();
        t.set_primary_session("parent".into());
        let hook = |json: &str| crate::hooks::parse_payload("agy", None, json);
        t.on_hook_for("agy", &hook(r#"{"hookEventName":"PreInvocation","conversationId":"parent"}"#), false);
        assert_eq!(t.update(Seen::Blocked, false), AgentState::NeedsYou);

        let out = t.on_hook_for("agy", &hook(r#"{"hookEventName":"PreInvocation","conversationId":"sub-1"}"#), false);
        assert_eq!(out, HookOutcome { state: AgentState::NeedsYou, session_id: None });
    }

    #[test]
    fn ordinary_prose_does_not_false_trigger_agy_or_grok_rules() {
        let (all, _) = rules(Path::new("/no/such/dir"));
        let prose = "I am thinking about working on this PR. Please approve and accept the changes.";
        for id in ["agy", "grok"] {
            let rule = all.iter().find(|r| r.id == id).unwrap();
            assert_eq!(detect(rule, prose), Seen::Quiet, "{id} false-triggered on prose");
            assert_eq!(detect(rule, "Press esc to cancel"), Seen::Working, "{id} should detect working");
            assert_eq!(detect(rule, "Allow once  Always allow"), Seen::Blocked, "{id} should detect blocked");
        }
    }

    #[test]
    fn the_resume_id_is_read_off_the_screen() {
        let r = codex();
        let screen = "To continue this session, run codex resume 0199a-b2c3\n> ";
        let id = resume_id(&r, screen);
        assert_eq!(id.as_deref(), Some("0199a-b2c3"));
        assert_eq!(resume_argv(&r, id.as_deref()), ["codex", "resume", "0199a-b2c3"]);
        assert_eq!(resume_argv(&r, None), r.argv);
        assert_eq!(resume_id(&r, "nothing here"), None);
    }
}
