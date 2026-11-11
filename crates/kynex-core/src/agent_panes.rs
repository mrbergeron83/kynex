//! Other agent CLIs (Codex, Copilot, Cursor, opencode…) in terminal panes, their state read
//! off the screen, after herdr: each agent has a rule, a data file of text that, when it's
//! on the screen, means the agent is working or waiting on you, and of what it prints to be
//! resumed. The built-in rules are in `agent_rules/`; a file in `<data dir>/agents/` with
//! the same `id` replaces one, and a new id is a new agent.
//!
//! Patterns are plain text, matched without regard to case anywhere on the screen. Waiting
//! on you wins over working; a turn that stops working is done until you look at it.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::paths::data_dir;

const BUILT_IN: [&str; 4] = [
    include_str!("agent_rules/codex.json"),
    include_str!("agent_rules/copilot.json"),
    include_str!("agent_rules/cursor.json"),
    include_str!("agent_rules/opencode.json"),
];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentRule {
    /// What a pane and an override file call it (`codex`).
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
    /// What comes right before the id in the line it prints to be resumed
    /// (`codex resume `).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_after: Option<String>,
    /// How to resume, `{id}` standing for the id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_argv: Option<Vec<String>>,
}

/// What the screen says now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Seen {
    Working,
    Blocked,
    Done,
    Quiet,
}

/// An agent pane's state, as the lamps show a session's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AgentState {
    #[default]
    Idle,
    Working,
    NeedsYou,
    /// A turn ended and you haven't looked at it.
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

/// Every agent: the built-in rules with the user's from `dir` over them by id, sorted by
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

/// Follows an agent pane's screen from one look to the next.
#[derive(Clone, Debug, Default)]
pub struct Tracker {
    state: AgentState,
    /// Looks in a row with no sign of work since it was working.
    quiet: u8,
}

/// Looks in a row without the working text before a turn counts as over: an agent redraws
/// between steps, and a moment without the text isn't the end.
pub const QUIET_LOOKS: u8 = 3;

impl Tracker {
    pub fn state(&self) -> AgentState {
        self.state
    }

    /// The state after the screen says `seen`; `looking` is whether you are on the pane,
    /// which clears a done turn.
    pub fn update(&mut self, seen: Seen, looking: bool) -> AgentState {
        self.quiet = if seen == Seen::Quiet { self.quiet.saturating_add(1) } else { 0 };
        self.state = match seen {
            Seen::Blocked => AgentState::NeedsYou,
            Seen::Working => AgentState::Working,
            Seen::Done if !looking => AgentState::Done,
            // Still working until it has been quiet a while.
            Seen::Quiet if self.state == AgentState::Working && self.quiet < QUIET_LOOKS => AgentState::Working,
            Seen::Quiet if !looking && matches!(self.state, AgentState::Working | AgentState::Done) => AgentState::Done,
            _ => AgentState::Idle,
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
    fn the_built_in_rules_read() {
        let (all, errors) = rules(Path::new("/no/such/dir"));
        assert!(errors.is_empty());
        let ids: Vec<&str> = all.iter().map(|r| r.id.as_str()).collect();
        for id in ["codex", "copilot", "cursor", "opencode"] {
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
        // Done while you watch is just idle.
        t.update(Seen::Working, true);
        for _ in 0..QUIET_LOOKS {
            t.update(Seen::Quiet, true);
        }
        assert_eq!(t.state(), AgentState::Idle);
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
