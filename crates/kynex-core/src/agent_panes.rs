//! Agent CLIs in terminal panes: screen-pattern rules (`agent_rules/*.json`, user overrides in
//! `<data dir>/agents/`) plus a hybrid [`Tracker`] that takes authoritative native hook events
//! when a CLI supports them (`claude`, `agy`) and falls back to screen-pattern detection for
//! CLIs without hooks (`grok`, `codex`, `copilot`, `cursor`, `opencode`, custom rules).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::hooks::{HookReport, HookState};
use crate::paths::data_dir;

const BUILT_IN: [&str; 7] = [
    include_str!("agent_rules/claude.json"),
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
}

/// Looks in a row without the working text before a turn counts as over on screen-only CLIs.
pub const QUIET_LOOKS: u8 = 3;

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
        self.hooked = true;
        self.quiet = 0;
        self.screen_blocked = false;
        if let Some(next) = report.state {
            self.state = match next {
                HookState::Working => AgentState::Working,
                HookState::NeedsYou => AgentState::NeedsYou,
                HookState::Done if looking => AgentState::Idle,
                HookState::Done => AgentState::Done,
                HookState::Idle => AgentState::Idle,
            };
        }
        if let Some(d) = &report.detail {
            self.detail = d.clone();
        } else if self.state == AgentState::Idle {
            self.detail.clear();
        }
        self.state
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
        for id in ["claude", "grok", "agy", "codex", "copilot", "cursor", "opencode"] {
            assert!(ids.contains(&id), "{id}");
        }
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
        let working =
            HookReport { state: Some(HookState::Working), session_id: None, detail: Some("Using Bash".into()) };
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
        let done = HookReport { state: Some(HookState::Done), session_id: None, detail: Some("Finished".into()) };
        assert_eq!(t.on_hook(&done, false), AgentState::Done);
        assert!(t.mark_seen());
        assert_eq!(t.state(), AgentState::Idle);
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
