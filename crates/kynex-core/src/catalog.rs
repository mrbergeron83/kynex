//! What else extends Claude in a folder, besides MCP servers: skills and slash
//! commands, subagents, and hooks, each with where it's defined (you, the
//! project, or a plugin). Read from their files, never changed here.
//!
//! - commands: `commands/**/*.md` (the file name is the command; folders
//!   become `folder:name`)
//! - skills: `skills/<name>/SKILL.md`
//! - agents: `agents/*.md`
//! - hooks: `hooks` in each settings file, and a plugin's `hooks/hooks.json`

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::extensions::{Paths, Plugin};

/// Where a piece is defined.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    User,
    Project,
    /// This folder only, not shared (`settings.local.json`).
    Local,
    /// Skills synced from your claude.ai account (`skills/synced/<bucket>/`).
    ClaudeAi,
    Plugin(String),
}

impl Origin {
    pub fn label(&self) -> String {
        match self {
            Self::User => "You".into(),
            Self::Project => "This project".into(),
            Self::Local => "This folder, just you".into(),
            Self::ClaudeAi => "claude.ai".into(),
            Self::Plugin(id) => format!("Plugin {}", id.split('@').next().unwrap_or(id)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Command,
    Skill,
    Agent,
}

/// A command, skill or agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub kind: Kind,
    pub name: String,
    pub description: String,
    pub origin: Origin,
    pub path: PathBuf,
}

/// A hook: a command run on an event (for tools matching `matcher`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hook {
    pub event: String,
    pub matcher: String,
    pub command: String,
    pub origin: Origin,
}

/// The folders to look in, each with where what's in it comes from: yours,
/// the project's, then each enabled plugin's.
fn roots(paths: &Paths, cwd: &Path, plugins: &[Plugin]) -> Vec<(PathBuf, Origin)> {
    let mut roots = vec![(paths.dir.clone(), Origin::User), (cwd.join(".claude"), Origin::Project)];
    roots.extend(plugins.iter().filter(|p| p.enabled).map(|p| (p.install.clone(), Origin::Plugin(p.id.clone()))));
    roots
}

/// Skills and slash commands, by origin then name. Blocks reading files.
pub fn skills_and_commands(paths: &Paths, cwd: &Path, plugins: &[Plugin]) -> Vec<Item> {
    let mut out = Vec::new();
    for (root, origin) in roots(paths, cwd, plugins) {
        let commands = root.join("commands");
        for file in markdown_files(&commands) {
            let rel = file.strip_prefix(&commands).unwrap_or(&file).with_extension("");
            let name = rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join(":");
            let text = std::fs::read_to_string(&file).unwrap_or_default();
            let meta = front_matter(&text);
            out.push(Item {
                kind: Kind::Command,
                name,
                description: meta.description,
                origin: origin.clone(),
                path: file,
            });
        }
        let synced = subdirs(&root.join("skills").join("synced"))
            .into_iter()
            .filter(|b| origin == Origin::User && !b.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')))
            .flat_map(|b| subdirs(&b))
            .map(|d| (d, Origin::ClaudeAi));
        let own = subdirs(&root.join("skills")).into_iter().map(|d| (d, origin.clone()));
        for (dir, origin) in own.chain(synced.collect::<Vec<_>>()) {
            let file = dir.join("SKILL.md");
            let Ok(text) = std::fs::read_to_string(&file) else { continue };
            let meta = front_matter(&text);
            let name = meta.name.unwrap_or_else(|| dir.file_name().unwrap_or_default().to_string_lossy().into());
            out.push(Item {
                kind: Kind::Skill,
                name,
                description: meta.description,
                origin: origin.clone(),
                path: file,
            });
        }
    }
    out
}

/// Subagents, by origin then name. Blocks reading files.
pub fn agents(paths: &Paths, cwd: &Path, plugins: &[Plugin]) -> Vec<Item> {
    let mut out = Vec::new();
    for (root, origin) in roots(paths, cwd, plugins) {
        for file in markdown_files(&root.join("agents")) {
            let text = std::fs::read_to_string(&file).unwrap_or_default();
            let meta = front_matter(&text);
            let name = meta.name.unwrap_or_else(|| file.file_stem().unwrap_or_default().to_string_lossy().into());
            out.push(Item {
                kind: Kind::Agent,
                name,
                description: meta.description,
                origin: origin.clone(),
                path: file,
            });
        }
    }
    out
}

/// Hooks from every settings file that applies to `cwd`, and from enabled
/// plugins. Blocks reading files.
pub fn hooks(paths: &Paths, cwd: &Path, plugins: &[Plugin]) -> Vec<Hook> {
    let read = |p: &Path| crate::extensions::read_json(p).unwrap_or(Value::Null);
    let mut out = Vec::new();
    for (file, origin) in [
        (paths.dir.join("settings.json"), Origin::User),
        (cwd.join(".claude").join("settings.json"), Origin::Project),
        (cwd.join(".claude").join("settings.local.json"), Origin::Local),
    ] {
        out.extend(parse_hooks(&read(&file)["hooks"], &origin));
    }
    for p in plugins.iter().filter(|p| p.enabled) {
        let v = read(&p.install.join("hooks").join("hooks.json"));
        let map = if v.get("hooks").is_some() { &v["hooks"] } else { &v };
        out.extend(parse_hooks(map, &Origin::Plugin(p.id.clone())));
    }
    out
}

/// A `hooks` map: event → [{ matcher, hooks: [{ type, command }] }].
pub(crate) fn parse_hooks(map: &Value, origin: &Origin) -> Vec<Hook> {
    let mut out = Vec::new();
    for (event, entries) in map.as_object().into_iter().flatten() {
        for entry in entries.as_array().into_iter().flatten() {
            let matcher = entry["matcher"].as_str().unwrap_or("").to_string();
            for h in entry["hooks"].as_array().into_iter().flatten() {
                let command = match h["type"].as_str() {
                    Some("command") | None => h["command"].as_str().unwrap_or("").to_string(),
                    Some(other) => format!("({other}) {}", h["prompt"].as_str().or(h["url"].as_str()).unwrap_or("")),
                };
                out.push(Hook { event: event.clone(), matcher: matcher.clone(), command, origin: origin.clone() });
            }
        }
    }
    out
}

/// A markdown file's front matter: its `name` and `description`, if any.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Meta {
    pub name: Option<String>,
    pub description: String,
}

/// Reads the `---` block at the top of a markdown file (simple `key: value`
/// lines; quotes trimmed). Without one, the first line of text describes it.
pub(crate) fn front_matter(text: &str) -> Meta {
    let mut meta = Meta::default();
    let text = text.trim_start_matches('\u{feff}');
    let mut lines = text.lines();
    if lines.clone().next().map(str::trim) == Some("---") {
        lines.next();
        for line in lines.by_ref() {
            if line.trim() == "---" {
                break;
            }
            let Some((key, value)) = line.split_once(':') else { continue };
            let value = value.trim().trim_matches(['"', '\'']).to_string();
            match key.trim() {
                "name" if !value.is_empty() => meta.name = Some(value),
                "description" => meta.description = value,
                _ => {}
            }
        }
    }
    if meta.description.is_empty() {
        meta.description = lines
            .map(|l| l.trim().trim_start_matches('#').trim())
            .find(|l| !l.is_empty())
            .unwrap_or("")
            .chars()
            .take(200)
            .collect();
    }
    meta
}

/// Every `.md` file under `dir`, sorted, at most a few folders deep.
fn markdown_files(dir: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for e in entries.filter_map(Result::ok) {
            let path = e.path();
            if path.is_dir() && depth < 3 {
                walk(&path, depth + 1, out);
            } else if path.extension().is_some_and(|x| x.eq_ignore_ascii_case("md")) {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, 0, &mut out);
    out.sort();
    out
}

fn subdirs(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_front_matter() {
        let m = front_matter("---\nname: reviewer\ndescription: \"Reviews diffs\"\nmodel: opus\n---\n# Body\n");
        assert_eq!(m, Meta { name: Some("reviewer".into()), description: "Reviews diffs".into() });
        // No front matter: the first line of text says what it is.
        assert_eq!(front_matter("\n# Deploy the app\nSteps…").description, "Deploy the app");
        assert_eq!(front_matter("").description, "");
        assert_eq!(front_matter("---\nname:\n").name, None);
    }

    #[test]
    fn reads_hooks() {
        let map = json!({
            "PreToolUse": [{ "matcher": "Bash", "hooks": [{ "type": "command", "command": "check.sh" }] }],
            "Stop": [{ "hooks": [{ "type": "command", "command": "notify" }, { "type": "prompt", "prompt": "Done?" }] }],
            "Bad": "nope"
        });
        let hooks = parse_hooks(&map, &Origin::User);
        let got: Vec<_> = hooks.iter().map(|h| (h.event.as_str(), h.matcher.as_str(), h.command.as_str())).collect();
        assert_eq!(got, [("PreToolUse", "Bash", "check.sh"), ("Stop", "", "notify"), ("Stop", "", "(prompt) Done?")]);
    }

    #[test]
    fn lists_files_with_their_origins() {
        let dir = std::env::temp_dir().join(format!("kynex-catalog-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (home, cwd) = (dir.join("home"), dir.join("proj"));
        let write = |p: PathBuf, t: &str| {
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, t).unwrap();
        };
        write(home.join("commands/git/ship.md"), "---\ndescription: Ship it\n---\n");
        write(home.join("skills/pdf/SKILL.md"), "---\nname: pdf\ndescription: Read PDFs\n---\n");
        write(home.join("skills/synced/b1/docx/SKILL.md"), "---\nname: docx\n---\n");
        write(home.join("skills/synced/.bucket-b1/x/SKILL.md"), "");
        write(cwd.join(".claude/agents/reviewer.md"), "---\nname: reviewer\ndescription: Reviews\n---\n");
        write(
            cwd.join(".claude/settings.json"),
            r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"x"}]}]}}"#,
        );
        let paths = Paths { global: home.join(".claude.json"), dir: home.clone() };
        let items = skills_and_commands(&paths, &cwd, &[]);
        let got: Vec<_> = items.iter().map(|i| (i.kind, i.name.as_str(), i.origin.clone())).collect();
        assert_eq!(
            got,
            [
                (Kind::Command, "git:ship", Origin::User),
                (Kind::Skill, "pdf", Origin::User),
                (Kind::Skill, "docx", Origin::ClaudeAi)
            ]
        );
        let agents = agents(&paths, &cwd, &[]);
        assert_eq!((agents[0].name.as_str(), &agents[0].origin), ("reviewer", &Origin::Project));
        assert_eq!(hooks(&paths, &cwd, &[])[0].origin, Origin::Project);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
