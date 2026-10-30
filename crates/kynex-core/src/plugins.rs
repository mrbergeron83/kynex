//! Managing plugins through the CLI: what each installed one contributes,
//! enabling, disabling, updating, removing, and installing from marketplaces.
//! Every change runs `claude plugin … --json`; Kynex never edits the CLI's files.

use std::path::Path;

use serde_json::Value;

use crate::catalog::{self, Origin};
use crate::cli;
use crate::extensions::{Paths, Plugin};

/// What a plugin adds to sessions, counted from its files.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Contributes {
    pub commands: usize,
    pub skills: usize,
    pub agents: usize,
    pub hooks: usize,
    pub servers: Vec<String>,
}

impl Contributes {
    /// "2 skills · 1 agent · MCP: playwright", or "" when it adds nothing.
    pub fn summary(&self) -> String {
        let count = |n: usize, one: &str| match n {
            0 => None,
            1 => Some(format!("1 {one}")),
            n => Some(format!("{n} {one}s")),
        };
        let mut parts: Vec<String> = [
            count(self.skills, "skill"),
            count(self.commands, "command"),
            count(self.agents, "agent"),
            count(self.hooks, "hook"),
        ]
        .into_iter()
        .flatten()
        .collect();
        if !self.servers.is_empty() {
            parts.push(format!("MCP: {}", self.servers.join(", ")));
        }
        parts.join(" · ")
    }
}

/// What `plugin` adds, read from its install folder (disabled ones too).
/// Blocks reading files.
pub fn contributes(plugin: &Plugin) -> Contributes {
    let on = Plugin { enabled: true, ..plugin.clone() };
    let one = std::slice::from_ref(&on);
    // Only the plugin's own folder: user and project roots point nowhere.
    let nowhere = plugin.install.join("\0none");
    let paths = Paths { global: nowhere.clone(), dir: nowhere.clone() };
    let mine = |o: &Origin| matches!(o, Origin::Plugin(_));
    let items = catalog::skills_and_commands(&paths, &nowhere, one);
    let read = |p: &Path| crate::extensions::read_json(p).unwrap_or(Value::Null);
    let manifest = read(&plugin.install.join(".claude-plugin").join("plugin.json"));
    let mcp = read(&plugin.install.join(".mcp.json"));
    let mut servers: Vec<String> = [&manifest["mcpServers"], &mcp["mcpServers"], &mcp]
        .into_iter()
        .filter_map(Value::as_object)
        .flat_map(|m| m.iter().filter(|(_, v)| v.is_object()).map(|(k, _)| k.clone()))
        .collect();
    servers.sort();
    servers.dedup();
    Contributes {
        commands: items.iter().filter(|i| mine(&i.origin) && i.kind == catalog::Kind::Command).count(),
        skills: items.iter().filter(|i| mine(&i.origin) && i.kind == catalog::Kind::Skill).count(),
        agents: catalog::agents(&paths, &nowhere, one).iter().filter(|i| mine(&i.origin)).count(),
        hooks: catalog::hooks(&paths, &nowhere, one).iter().filter(|h| mine(&h.origin)).count(),
        servers,
    }
}

/// A plugin a marketplace offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Available {
    /// `name@marketplace`.
    pub id: String,
    pub name: String,
    pub description: String,
    pub marketplace: String,
    pub installs: u64,
}

/// A marketplace plugins come from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Marketplace {
    pub name: String,
    /// `owner/repo`, a URL or a path.
    pub source: String,
}

/// What every added marketplace offers, most installed first. Blocks on
/// `claude plugin list --json --available`.
pub fn available(cwd: &Path) -> Result<Vec<Available>, String> {
    let out = claude(cwd, &["plugin", "list", "--json", "--available"])?;
    let v: Value = serde_json::from_slice(&out.stdout).map_err(|_| failure(&["plugin", "list"], &out))?;
    Ok(parse_available(&v))
}

pub(crate) fn parse_available(v: &Value) -> Vec<Available> {
    let mut out: Vec<Available> = v["available"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| {
            let id = p["pluginId"].as_str()?.to_string();
            Some(Available {
                name: p["name"]
                    .as_str()
                    .map_or_else(|| id.split('@').next().unwrap_or(&id).to_string(), str::to_string),
                description: p["description"].as_str().unwrap_or("").trim().to_string(),
                marketplace: p["marketplaceName"].as_str().unwrap_or("").to_string(),
                installs: p["installCount"].as_u64().unwrap_or(0),
                id,
            })
        })
        .collect();
    out.sort_by(|a, b| b.installs.cmp(&a.installs).then_with(|| a.name.cmp(&b.name)));
    out
}

/// The marketplaces added. Blocks on `claude plugin marketplace list --json`.
pub fn marketplaces(cwd: &Path) -> Result<Vec<Marketplace>, String> {
    let out = claude(cwd, &["plugin", "marketplace", "list", "--json"])?;
    let v: Value = serde_json::from_slice(&out.stdout).map_err(|_| failure(&["plugin", "marketplace"], &out))?;
    Ok(parse_marketplaces(&v))
}

pub(crate) fn parse_marketplaces(v: &Value) -> Vec<Marketplace> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| {
            let source = ["repo", "url", "path", "installLocation"].iter().find_map(|k| m[*k].as_str()).unwrap_or("");
            Some(Marketplace { name: m["name"].as_str()?.to_string(), source: source.to_string() })
        })
        .collect()
}

/// Adds a marketplace (`owner/repo`, a git URL or a path) for you. Blocks on
/// `claude plugin marketplace add`, which clones it.
pub fn add_marketplace(cwd: &Path, source: &str) -> Result<(), String> {
    let source = source.trim();
    if source.is_empty() {
        return Err("Say where the marketplace is: owner/repo, a git URL or a folder.".into());
    }
    let out = claude(cwd, &["plugin", "marketplace", "add", source])?;
    if out.status.success() { Ok(()) } else { Err(failure(&["plugin", "marketplace"], &out)) }
}

/// A change to one plugin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Enable,
    Disable,
    Update,
    Uninstall,
    Install,
}

impl Action {
    fn verb(self) -> &'static str {
        match self {
            Self::Enable => "enable",
            Self::Disable => "disable",
            Self::Update => "update",
            Self::Uninstall => "uninstall",
            Self::Install => "install",
        }
    }
}

/// How a change went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Done(String),
    /// The marketplace wants to run a command to install it: show it, and run
    /// again with `accept` set to `sha256` if the person agrees.
    Confirm {
        command: String,
        sha256: String,
    },
}

/// Runs `claude plugin <action> <id> -s <scope> --json`. `accept` is the
/// sha256 of a marketplace command the person has seen and agreed to. Blocks
/// on the CLI (an install or update downloads).
pub fn change(cwd: &Path, action: Action, id: &str, scope: &str, accept: Option<&str>) -> Result<Outcome, String> {
    let mut args = vec!["plugin", action.verb(), id, "--json"];
    if !scope.is_empty() {
        args.extend(["-s", scope]);
    }
    if let Some(sha) = accept {
        args.extend(["--accept-command", sha]);
    }
    let out = claude(cwd, &args)?;
    let line = String::from_utf8_lossy(&out.stdout);
    let line = line.lines().rev().find(|l| l.trim_start().starts_with('{')).unwrap_or("");
    match serde_json::from_str::<Value>(line) {
        Ok(v) => parse_result(&v),
        Err(_) if out.status.success() => Ok(Outcome::Done(String::new())),
        Err(_) => Err(failure(&["plugin", action.verb()], &out)),
    }
}

/// One `--json` result line.
pub(crate) fn parse_result(v: &Value) -> Result<Outcome, String> {
    let message = v["message"].as_str().unwrap_or("").trim().to_string();
    let shown = &v["shownCommand"];
    if let Some(sha) = shown["sha256"].as_str() {
        let command = match shown["command"].as_str() {
            Some(c) => c.to_string(),
            None => {
                let mut rest = shown.clone();
                rest.as_object_mut().map(|o| o.remove("sha256"));
                serde_json::to_string_pretty(&rest).unwrap_or_default()
            }
        };
        return Ok(Outcome::Confirm { command, sha256: sha.to_string() });
    }
    match v["outcome"].as_str() {
        Some("failed") => Err(if message.is_empty() { "The CLI didn't say why.".into() } else { message }),
        _ => Ok(Outcome::Done(message)),
    }
}

/// Signs in to MCP server `name` (OAuth): the CLI opens the browser and waits
/// for the sign-in to finish. Blocks until then.
pub fn mcp_login(cwd: &Path, name: &str) -> Result<(), String> {
    let out = claude(cwd, &["mcp", "login", name])?;
    if out.status.success() { Ok(()) } else { Err(failure(&["mcp", "login"], &out)) }
}

fn claude(cwd: &Path, args: &[&str]) -> Result<std::process::Output, String> {
    cli::command(cli::program())
        .args(args)
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("Couldn't start `claude`: {e}"))
}

fn failure(what: &[&str], out: &std::process::Output) -> String {
    let said = cli::last_line(&out.stderr).or_else(|| cli::last_line(&out.stdout));
    format!("`claude {}` failed: {}", what.join(" "), said.unwrap_or_else(|| "no message".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_what_marketplaces_offer() {
        let v = json!({ "installed": [], "available": [
            { "pluginId": "a@m", "name": "a", "description": " A ", "marketplaceName": "m", "installCount": 3 },
            { "pluginId": "b@m", "installCount": 9 },
            { "name": "no id" }
        ]});
        let got = parse_available(&v);
        assert_eq!(got.iter().map(|p| (p.name.as_str(), p.installs)).collect::<Vec<_>>(), [("b", 9), ("a", 3)]);
        assert_eq!(got[1].description, "A");
        let m = parse_marketplaces(&json!([{ "name": "official", "source": "github", "repo": "o/r" }, {}]));
        assert_eq!(m, [Marketplace { name: "official".into(), source: "o/r".into() }]);
    }

    #[test]
    fn reads_results() {
        let ok = json!({ "command": "enable", "outcome": "success", "message": "Enabled" });
        assert_eq!(parse_result(&ok), Ok(Outcome::Done("Enabled".into())));
        let failed = json!({ "outcome": "failed", "message": "Plugin \"x\" not found" });
        assert_eq!(parse_result(&failed), Err("Plugin \"x\" not found".into()));
        let ask = json!({ "outcome": "failed", "shownCommand": { "command": "curl x | sh", "sha256": "ab" } });
        assert_eq!(parse_result(&ask), Ok(Outcome::Confirm { command: "curl x | sh".into(), sha256: "ab".into() }));
        let other = json!({ "shownCommand": { "argv": ["a"], "sha256": "cd" } });
        let Ok(Outcome::Confirm { command, .. }) = parse_result(&other) else { panic!() };
        assert!(command.contains("argv") && !command.contains("cd"));
    }

    #[test]
    fn counts_what_a_plugin_adds() {
        let dir = std::env::temp_dir().join(format!("kynex-plugin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let write = |p: &str, t: &str| {
            let p = dir.join(p);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, t).unwrap();
        };
        write("skills/a/SKILL.md", "---\nname: a\n---");
        write("skills/b/SKILL.md", "");
        write("agents/r.md", "");
        write("hooks/hooks.json", r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"x"}]}]}}"#);
        write(".mcp.json", r#"{"pw":{"command":"npx"}}"#);
        let p = Plugin {
            id: "p@m".into(),
            version: "1".into(),
            scope: "user".into(),
            enabled: false,
            install: dir.clone(),
        };
        let c = contributes(&p);
        assert_eq!((c.skills, c.agents, c.hooks, c.servers.clone()), (2, 1, 1, vec!["pw".to_string()]));
        assert_eq!(c.summary(), "2 skills · 1 agent · 1 hook · MCP: pw");
        assert_eq!(Contributes::default().summary(), "");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
