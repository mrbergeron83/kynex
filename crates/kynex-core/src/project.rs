//! Per-project settings, kept in Kynex's own data dir rather than in the repo:
//!
//! ```text
//! <data dir>/projects/<name>-<hash>/
//!     project.json        which folder this is (lets a moved repo be re-linked later)
//!     permissions.json    allow / deny rules Kynex hands to every Claude session
//!     claude.json         how sessions start: permission mode, effort, extra directories
//!     sessions.json       how the sessions panel is organised: groups, archive
//!     appearance.json     the project's colour
//! ```
//!
//! Each section is its own file, read and written on its own, so a new kind of
//! per-project setting is just another file next to these.

use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::paths::data_dir;

/// The settings folder for a project, e.g. `projects/kynex-3f9a1c0d`.
pub fn dir(project: &Path) -> PathBuf {
    data_dir().join("projects").join(folder_name(project))
}

/// Readable but unique: the folder's name plus a hash of its full path, so two
/// repos that are both called `api` don't share settings.
fn folder_name(project: &Path) -> String {
    let name = project.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut slug: String =
        name.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' { c.to_ascii_lowercase() } else { '-' }
            })
            .take(40)
            .collect();
    if slug.trim_matches(['-', '.']).is_empty() {
        slug = "project".into();
    }
    format!("{slug}-{:08x}", fnv1a(normalize(project).as_bytes()) as u32)
}

/// One spelling per folder: no trailing separator, and on Windows (where paths
/// are case-insensitive) one case and one kind of slash.
fn normalize(p: &Path) -> String {
    let s = p.to_string_lossy();
    let s = s.trim_end_matches(['/', '\\']);
    if cfg!(windows) { s.replace('/', "\\").to_lowercase() } else { s.to_string() }
}

/// FNV-1a: tiny and, unlike `DefaultHasher`, guaranteed stable across Rust versions,
/// which matters because the hash names folders on disk.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ *b as u64).wrapping_mul(0x0000_0100_0000_01b3))
}

#[derive(Serialize, Deserialize)]
struct ProjectInfo {
    path: PathBuf,
}

pub(crate) fn load<T: DeserializeOwned + Default>(project: &Path, file: &str) -> T {
    std::fs::read_to_string(dir(project).join(file))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Writes one section, creating the project's folder (and `project.json`) on first use.
/// Goes through a temp file so a crash mid-write never leaves a truncated file.
pub(crate) fn save<T: Serialize>(project: &Path, file: &str, value: &T) -> Result<(), String> {
    let dir = dir(project);
    std::fs::create_dir_all(&dir).map_err(|e| format!("Couldn't create {}: {e}", dir.display()))?;
    let info = dir.join("project.json");
    if !info.exists() {
        write_json(&info, &ProjectInfo { path: project.to_path_buf() })?;
    }
    write_json(&dir.join(file), value)
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let s = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, s)
        .and_then(|_| std::fs::rename(&tmp, path))
        .map_err(|e| format!("Couldn't save {}: {e}", path.display()))
}

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
}

impl Mode {
    pub const ALL: [Mode; 4] = [Mode::Manual, Mode::AcceptEdits, Mode::Auto, Mode::Plan];

    pub fn cli(self) -> &'static str {
        match self {
            Mode::Manual => "manual",
            Mode::AcceptEdits => "acceptEdits",
            Mode::Auto => "auto",
            Mode::Plan => "plan",
        }
    }

    pub fn from_cli(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.cli() == s).or((s == "default").then_some(Mode::Manual))
    }

    pub fn label(self) -> &'static str {
        match self {
            Mode::Manual => "Ask first",
            Mode::AcceptEdits => "Accept edits",
            Mode::Auto => "Auto",
            Mode::Plan => "Plan",
        }
    }

    pub fn help(self) -> &'static str {
        match self {
            Mode::Manual => "Ask before anything not covered by an allow rule",
            Mode::AcceptEdits => "Edit files without asking; ask for other tools",
            Mode::Auto => "Approve safe actions automatically; ask about risky ones",
            Mode::Plan => "Research read-only, then propose a plan for you to approve",
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
}

impl ClaudeSettings {
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

/// The colours a project can be marked with, by name; the app turns each into a
/// shade that suits the theme. Ordered round the colour wheel.
pub const COLOURS: [&str; 8] = ["red", "orange", "yellow", "green", "teal", "blue", "violet", "pink"];

/// A project's colour, to tell at a glance which project you're in.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Colour {
    /// One of [`COLOURS`], picked from the folder's path so it stays the same.
    #[default]
    Auto,
    /// No colour: the project looks like any other.
    Off,
    /// One of [`COLOURS`], by name.
    Named(String),
}

/// How a project looks in Kynex.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    pub colour: Colour,
}

impl Appearance {
    const FILE: &str = "appearance.json";

    pub fn load(project: &Path) -> Self {
        load(project, Self::FILE)
    }

    pub fn save(&self, project: &Path) -> Result<(), String> {
        save(project, Self::FILE, self)
    }

    /// The name of the project's colour from [`COLOURS`], or `None` when it's off
    /// (or names a colour Kynex no longer has).
    pub fn colour(&self, project: &Path) -> Option<&'static str> {
        match &self.colour {
            Colour::Auto => Some(auto_colour(project)),
            Colour::Off => None,
            Colour::Named(name) => COLOURS.iter().copied().find(|c| c == name),
        }
    }
}

/// The colour a project gets unless you pick one: from its path's hash, so the
/// same folder is always the same colour.
pub fn auto_colour(project: &Path) -> &'static str {
    COLOURS[(fnv1a(normalize(project).as_bytes()) % COLOURS.len() as u64) as usize]
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
    fn colours_are_stable_and_can_be_picked_or_turned_off() {
        let p = Path::new("/home/me/work/api");
        let auto = Appearance::default().colour(p).unwrap();
        assert_eq!(auto, auto_colour(Path::new("/home/me/work/api/")), "same folder, same colour");
        assert!(COLOURS.contains(&auto));
        let named = |n: &str| Appearance { colour: Colour::Named(n.into()) };
        assert_eq!(named("teal").colour(p), Some("teal"));
        assert_eq!(named("chartreuse").colour(p), None);
        assert_eq!(Appearance { colour: Colour::Off }.colour(p), None);
        // Different folders don't all land on one colour.
        let spread: std::collections::HashSet<_> =
            (0..20).map(|i| auto_colour(Path::new(&format!("/repos/project-{i}")))).collect();
        assert!(spread.len() > 3);
    }

    #[test]
    fn appearance_round_trips_through_json() {
        for colour in [Colour::Auto, Colour::Off, Colour::Named("blue".into())] {
            let a = Appearance { colour };
            let s = serde_json::to_string(&a).unwrap();
            assert_eq!(serde_json::from_str::<Appearance>(&s).unwrap(), a);
        }
        assert_eq!(serde_json::from_str::<Appearance>("{}").unwrap(), Appearance::default());
    }

    #[test]
    fn folder_names_are_readable_and_distinct() {
        let a = folder_name(Path::new("/home/me/work/api"));
        let b = folder_name(Path::new("/home/me/play/api"));
        assert!(a.starts_with("api-") && b.starts_with("api-"));
        assert_ne!(a, b);
        assert_eq!(a, folder_name(Path::new("/home/me/work/api/")));
        assert_eq!(folder_name(Path::new("/srv/My Repo!")).split_once('-').unwrap().0, "my");
        assert!(folder_name(Path::new("/")).starts_with("project-"));
    }

    #[cfg(windows)]
    #[test]
    fn windows_paths_ignore_case_and_slashes() {
        assert_eq!(folder_name(Path::new(r"C:\Repos\Kynex")), folder_name(Path::new("c:/repos/kynex")));
    }

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
        for m in Mode::ALL {
            assert_eq!(Mode::from_cli(m.cli()), Some(m));
            assert_eq!(serde_json::to_value(m).unwrap(), m.cli());
        }
        assert_eq!(Mode::from_cli("default"), Some(Mode::Manual));
        assert_eq!(ClaudeSettings::default().mode, Mode::Auto);
    }

    #[test]
    fn claude_settings_args() {
        assert!(ClaudeSettings::default().cli_args().is_empty());
        let s =
            ClaudeSettings { mode: Mode::Plan, effort: "high".into(), add_dirs: vec![PathBuf::from("/srv/shared")] };
        assert_eq!(s.cli_args(), ["--effort", "high", "--add-dir", "/srv/shared"]);
    }

    #[test]
    fn cli_settings_only_when_there_are_rules() {
        assert_eq!(Permissions::default().as_cli_settings(), None);
        let p = Permissions { allow: vec!["Bash(ls:*)".into()], deny: vec![] };
        let v: Value = serde_json::from_str(&p.as_cli_settings().unwrap()).unwrap();
        assert_eq!(v["permissions"]["allow"][0], "Bash(ls:*)");
    }
}
