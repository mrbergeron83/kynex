//! How a project's Claude sessions start and what they may do, kept beside its other
//! settings ([`super::dir`]):
//!
//! ```text
//!     permissions.json    allow / deny rules Kynex hands to every Claude session
//!     claude.json         how sessions start: permission mode, effort, extra directories
//! ```

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{load, save};

/// Permission rules in Claude Code's own syntax, e.g. `Bash(cargo check:*)`,
/// `Edit`, `WebFetch(domain:docs.rs)`. Kynex owns them; the CLI enforces them.
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Permissions {
    pub allow: Vec<String>,
    pub deny: Vec<String>,
}

impl Permissions {
    const FILE: &str = "permissions.json";

    pub fn load(project: &Path) -> Self {
        load(project, Self::FILE)
    }

    pub fn save(&self, project: &Path) -> Result<(), String> {
        save(project, Self::FILE, self)
    }

    /// Adds allow rules not already present. Returns whether anything changed.
    pub fn allow(&mut self, rules: impl IntoIterator<Item = String>) -> bool {
        let before = self.allow.len();
        for r in rules {
            if !self.allow.contains(&r) {
                self.allow.push(r);
            }
        }
        self.allow.len() != before
    }

    /// The rules as a `--settings` JSON string, or `None` if there are none.
    /// These merge with the user's and the repo's own Claude settings; a deny
    /// anywhere still wins.
    pub fn as_cli_settings(&self) -> Option<String> {
        if self.allow.is_empty() && self.deny.is_empty() {
            return None;
        }
        Some(json!({ "permissions": { "allow": self.allow, "deny": self.deny } }).to_string())
    }
}

/// Claude Code's permission modes, as the CLI names them (`--permission-mode`).
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Mode {
    /// Ask before anything not covered by an allow rule.
    Manual,
    /// Edit files without asking; still ask for other tools.
    AcceptEdits,
    /// A classifier approves safe actions and only asks about risky ones, like the CLI's auto mode.
    #[default]
    Auto,
    /// Read-only research, then propose a plan for approval.
    Plan,
    /// Skip every permission check (`bypassPermissions`). Only in the cycle when
    /// turned on in Settings, as the CLI needs `--allow-dangerously-skip-permissions`.
    #[serde(rename = "bypassPermissions")]
    Bypass,
}

/// Whether bypass mode may be used: the app's setting, read when a process starts.
static BYPASS_ALLOWED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Turns bypass mode on or off for the processes started from now on (the app's setting).
pub fn set_bypass_allowed(on: bool) {
    BYPASS_ALLOWED.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// Whether bypass mode is turned on in Settings.
pub fn bypass_allowed() -> bool {
    BYPASS_ALLOWED.load(std::sync::atomic::Ordering::Relaxed)
}

impl Mode {
    /// The modes a project starts in and Shift+Tab goes through; [`Mode::Bypass`] joins
    /// the cycle only when it's allowed ([`Mode::cycle`]).
    pub const ALL: [Mode; 4] = [Mode::Manual, Mode::AcceptEdits, Mode::Auto, Mode::Plan];

    /// The modes to pick from: [`Self::ALL`], and bypass when `bypass` is allowed.
    pub fn choices(bypass: bool) -> Vec<Mode> {
        let mut all = Self::ALL.to_vec();
        if bypass {
            all.push(Mode::Bypass);
        }
        all
    }

    /// The mode after this one in the Shift+Tab cycle; bypass is in it when `bypass`.
    pub fn cycle(self, bypass: bool) -> Mode {
        let all = Self::choices(bypass);
        all.iter().position(|m| *m == self).map_or(all[0], |i| all[(i + 1) % all.len()])
    }

    pub fn cli(self) -> &'static str {
        match self {
            Mode::Manual => "manual",
            Mode::AcceptEdits => "acceptEdits",
            Mode::Auto => "auto",
            Mode::Plan => "plan",
            Mode::Bypass => "bypassPermissions",
        }
    }

    pub fn from_cli(s: &str) -> Option<Self> {
        Self::choices(true).into_iter().find(|m| m.cli() == s).or((s == "default").then_some(Mode::Manual))
    }

    pub fn label(self) -> &'static str {
        match self {
            Mode::Manual => "Ask first",
            Mode::AcceptEdits => "Accept edits",
            Mode::Auto => "Auto",
            Mode::Plan => "Plan",
            Mode::Bypass => "Bypass",
        }
    }

    pub fn help(self) -> &'static str {
        match self {
            Mode::Manual => "Ask before anything not covered by an allow rule",
            Mode::AcceptEdits => "Edit files without asking; ask for other tools",
            Mode::Auto => "Approve safe actions automatically; ask about risky ones",
            Mode::Plan => "Research read-only, then propose a plan for you to approve",
            Mode::Bypass => "Skip every permission check: Claude runs anything without asking",
        }
    }
}

/// Effort levels for `--effort`; empty means "whatever your Claude settings say".
pub const EFFORTS: [(&str, &str); 6] =
    [("", "Default"), ("low", "Low"), ("medium", "Medium"), ("high", "High"), ("xhigh", "Extra high"), ("max", "Max")];

/// How a project's Claude sessions start. Anything left empty falls back to the
/// user's own Claude settings, exactly as in the CLI.
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClaudeSettings {
    /// The mode new sessions start in; the composer switches a live session.
    pub mode: Mode,
    pub effort: String,
    /// Extra folders Claude may read and edit, like `--add-dir`.
    pub add_dirs: Vec<PathBuf>,
    /// The model this project's new sessions start on; empty is the CLI's
    /// default. A session can pin its own over it ([`crate::groups::SessionGroups`]).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub model: String,
    /// Whether this project's sessions hand routine work to Kynex's subagents;
    /// `None` follows the app-wide default. Kept for older settings files.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delegate: Option<bool>,
    /// Kynex's subagents this project doesn't use, by name.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub agents_off: Vec<String>,
    /// This project's own rule for what goes to the subagents; empty is the
    /// one made from their descriptions.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub delegate_rule: String,
    /// Who this project's new sessions start on; a new session can pick the other.
    #[serde(skip_serializing_if = "crate::provider::Provider::is_anthropic_ref")]
    pub provider: crate::provider::Provider,
    /// The OpenRouter model its new OpenRouter sessions start on; empty is
    /// the default of My models.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub openrouter_model: String,
    /// The OpenRouter model its new Chat sessions start on (OpenRouter chat), its own
    /// pick apart from Work's; empty is the default of My models.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub openrouter_chat_model: String,
    /// The model its new Grok sessions start on; empty is Grok's default.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub grok_model: String,
    /// The OpenRouter model its new Antigravity sessions start on; empty is
    /// [`crate::provider::DEFAULT_ANTIGRAVITY_MODEL`].
    #[serde(skip_serializing_if = "String::is_empty")]
    pub antigravity_model: String,
    /// Thinking turned off for this project's sessions (the composer's switch).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub thinking_off: bool,
}

impl ClaudeSettings {
    /// These settings as a session on `provider` uses them: no effort where
    /// the provider's models don't take it.
    pub fn for_provider(&self, provider: crate::provider::Provider) -> Self {
        let mut s = self.clone();
        if !provider.takes_effort() {
            s.effort.clear();
        }
        s
    }

    const FILE: &str = "claude.json";

    pub fn load(project: &Path) -> Self {
        load(project, Self::FILE)
    }

    pub fn save(&self, project: &Path) -> Result<(), String> {
        save(project, Self::FILE, self)
    }

    /// CLI flags for everything but the mode, which each session tracks itself.
    pub fn cli_args(&self) -> Vec<String> {
        let mut args = Vec::new();
        if !self.effort.is_empty() {
            args.extend(["--effort".to_string(), self.effort.clone()]);
        }
        for d in &self.add_dirs {
            args.extend(["--add-dir".to_string(), d.display().to_string()]);
        }
        args
    }
}

/// How a running process takes a change to its project's rules, extra folders or effort.
#[derive(Debug, PartialEq)]
pub enum Update {
    /// It already runs on them.
    Same,
    /// This `apply_flag_settings` patch brings it up to date.
    Live(Value),
    /// It has to start again: a rule or folder it started with went away, or
    /// effort went back to the default. A patch only adds to what `--settings`,
    /// `--add-dir` and `--effort` gave it, and can't unset a key.
    Restart,
}

/// What a process got from its project's settings: the rules, extra folders and
/// effort it started with, and the last patch it was sent since.
#[derive(Clone, Default, PartialEq)]
pub struct Launched {
    perms: Permissions,
    dirs: Vec<PathBuf>,
    effort: String,
    patched: Option<Value>,
}

impl Launched {
    pub fn new(perms: &Permissions, claude: &ClaudeSettings) -> Self {
        Self { perms: perms.clone(), dirs: claude.add_dirs.clone(), effort: claude.effort.clone(), patched: None }
    }

    /// How to bring the process to `perms` and `claude`. A [`Update::Live`]
    /// patch is taken as sent. It replaces the last one, so it always carries
    /// every rule and folder added since the start.
    pub fn update(&mut self, perms: &Permissions, claude: &ClaudeSettings) -> Update {
        let kept = |started: &[String], now: &[String]| started.iter().all(|r| now.contains(r));
        let effort_set = !self.effort.is_empty() || self.patched.as_ref().is_some_and(|p| p["effortLevel"].is_string());
        if !kept(&self.perms.allow, &perms.allow)
            || !kept(&self.perms.deny, &perms.deny)
            || !self.dirs.iter().all(|d| claude.add_dirs.contains(d))
            || (claude.effort.is_empty() && effort_set)
        {
            return Update::Restart;
        }
        let added = |started: &[String], now: &[String]| -> Vec<String> {
            now.iter().filter(|r| !started.contains(r)).cloned().collect()
        };
        let dirs: Vec<String> =
            claude.add_dirs.iter().filter(|d| !self.dirs.contains(d)).map(|d| d.display().to_string()).collect();
        let mut patch = json!({ "permissions": {
            "allow": added(&self.perms.allow, &perms.allow),
            "deny": added(&self.perms.deny, &perms.deny),
            "additionalDirectories": dirs,
        } });
        let nothing_added = patch["permissions"].as_object().is_some_and(|p| p.values().all(|v| v == &json!([])));
        if !claude.effort.is_empty() {
            patch["effortLevel"] = json!(claude.effort);
        }
        let same = match &self.patched {
            Some(last) => *last == patch,
            None => nothing_added && claude.effort == self.effort,
        };
        if same {
            return Update::Same;
        }
        self.patched = Some(patch.clone());
        Update::Live(patch)
    }
}

/// The allow rules in a `can_use_tool` request's `permission_suggestions`, as
/// rule strings. Other suggestion kinds (mode switches, extra directories)
/// aren't project rules, so they're left out.
pub fn suggested_rules(suggestions: &Value) -> Vec<String> {
    let mut rules = Vec::new();
    for s in suggestions.as_array().into_iter().flatten() {
        if s["type"] != "addRules" || s["behavior"] != "allow" {
            continue;
        }
        for r in s["rules"].as_array().into_iter().flatten() {
            let Some(tool) = r["toolName"].as_str() else { continue };
            rules.push(match r["ruleContent"].as_str() {
                Some(c) if !c.is_empty() => format!("{tool}({c})"),
                _ => tool.to_string(),
            });
        }
    }
    rules
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_allow_rules_from_suggestions() {
        let s = json!([
            { "type": "addRules", "behavior": "allow", "destination": "session",
              "rules": [{ "toolName": "Bash", "ruleContent": "cargo check:*" }, { "toolName": "Edit" }] },
            { "type": "setMode", "mode": "acceptEdits", "destination": "session" },
            { "type": "addRules", "behavior": "deny", "rules": [{ "toolName": "WebFetch" }] },
        ]);
        assert_eq!(suggested_rules(&s), ["Bash(cargo check:*)", "Edit"]);
        assert!(suggested_rules(&Value::Null).is_empty());
    }

    #[test]
    fn modes_round_trip_cli_names() {
        for m in Mode::choices(true) {
            assert_eq!(Mode::from_cli(m.cli()), Some(m));
            assert_eq!(serde_json::to_value(m).unwrap(), m.cli());
        }
        assert_eq!(Mode::from_cli("default"), Some(Mode::Manual));
        assert_eq!(ClaudeSettings::default().mode, Mode::Auto);
    }

    #[test]
    fn bypass_joins_the_cycle_only_when_allowed() {
        assert_eq!(Mode::Plan.cycle(false), Mode::Manual);
        assert_eq!(Mode::Plan.cycle(true), Mode::Bypass);
        assert_eq!(Mode::Bypass.cycle(true), Mode::Manual);
        // Bypass turned off while on it: back to the start.
        assert_eq!(Mode::Bypass.cycle(false), Mode::Manual);
        assert_eq!(Mode::Manual.cycle(true), Mode::AcceptEdits);
        assert!(!Mode::choices(false).contains(&Mode::Bypass));
    }

    #[test]
    fn claude_settings_args() {
        assert!(ClaudeSettings::default().cli_args().is_empty());
        let s = ClaudeSettings {
            mode: Mode::Plan,
            effort: "high".into(),
            add_dirs: vec![PathBuf::from("/srv/shared")],
            model: "opus".into(),
            ..Default::default()
        };
        // The model goes to each session, not through these flags.
        assert_eq!(s.cli_args(), ["--effort", "high", "--add-dir", "/srv/shared"]);
    }

    #[test]
    fn changes_apply_live_unless_something_is_taken_away() {
        let perms = Permissions { allow: vec!["Read".into()], deny: Vec::new() };
        let claude = ClaudeSettings { effort: "medium".into(), ..Default::default() };
        let mut l = Launched::new(&perms, &claude);
        assert_eq!(l.update(&perms, &claude), Update::Same);

        // Effort switches live; the patch carries no rules yet.
        let mut claude = ClaudeSettings { effort: "low".into(), ..claude };
        assert_eq!(
            l.update(&perms, &claude),
            Update::Live(json!({ "permissions": { "allow": [], "deny": [], "additionalDirectories": [] },
                "effortLevel": "low" }))
        );
        assert_eq!(l.update(&perms, &claude), Update::Same);

        // Added rules and folders go on top of the ones it started with, in one patch with the effort.
        let mut perms = Permissions { allow: vec!["Read".into(), "Edit".into()], deny: vec!["WebFetch".into()] };
        claude.add_dirs = vec![PathBuf::from("/srv")];
        assert_eq!(
            l.update(&perms, &claude),
            Update::Live(json!({ "permissions": { "allow": ["Edit"], "deny": ["WebFetch"],
                "additionalDirectories": ["/srv"] }, "effortLevel": "low" }))
        );
        // One added since goes again: the patch replaces the last.
        perms.allow.retain(|r| r != "Edit");
        assert_eq!(
            l.update(&perms, &claude),
            Update::Live(json!({ "permissions": { "allow": [], "deny": ["WebFetch"],
                "additionalDirectories": ["/srv"] }, "effortLevel": "low" }))
        );

        // A rule it started with going, or effort back to the default, needs a new process.
        assert_eq!(l.clone().update(&Permissions::default(), &claude), Update::Restart);
        let default_effort = ClaudeSettings { effort: String::new(), ..claude.clone() };
        assert_eq!(l.update(&perms, &default_effort), Update::Restart);
        // Started on the default, effort set live can't go back either.
        let mut l = Launched::new(&perms, &default_effort);
        assert!(matches!(l.update(&perms, &claude), Update::Live(_)));
        assert_eq!(l.update(&perms, &default_effort), Update::Restart);
    }

    #[test]
    fn project_model_round_trips_and_older_files_still_load() {
        let s = ClaudeSettings { model: "haiku".into(), ..Default::default() };
        let back: ClaudeSettings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back.model, "haiku");
        // Written before the field existed, or on the CLI's default: no key at all.
        let old: ClaudeSettings = serde_json::from_str(r#"{"mode":"auto","effort":"max"}"#).unwrap();
        assert_eq!(old.model, "");
        assert!(!serde_json::to_string(&ClaudeSettings::default()).unwrap().contains("model"));
    }

    #[test]
    fn a_project_picks_its_provider_and_openrouter_drops_effort() {
        use crate::provider::Provider;
        let s = ClaudeSettings {
            provider: Provider::OpenRouter,
            openrouter_model: "qwen/qwen3-coder:free".into(),
            effort: "high".into(),
            ..Default::default()
        };
        let back: ClaudeSettings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!((back.provider, back.openrouter_model.as_str()), (Provider::OpenRouter, "qwen/qwen3-coder:free"));
        assert_eq!(serde_json::from_str::<ClaudeSettings>("{}").unwrap().provider, Provider::Anthropic);
        assert!(s.for_provider(Provider::OpenRouter).cli_args().iter().all(|a| a != "--effort"));
        assert!(s.for_provider(Provider::Anthropic).cli_args().contains(&"--effort".to_string()));
    }

    #[test]
    fn cli_settings_only_when_there_are_rules() {
        assert_eq!(Permissions::default().as_cli_settings(), None);
        let p = Permissions { allow: vec!["Bash(ls:*)".into()], deny: vec![] };
        let v: Value = serde_json::from_str(&p.as_cli_settings().unwrap()).unwrap();
        assert_eq!(v["permissions"]["allow"][0], "Bash(ls:*)");
    }
}
