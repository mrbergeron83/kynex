//! What extends Claude in a folder, read from the CLI's own config files: MCP
//! servers (user and local ones in `~/.claude.json`, the repo's `.mcp.json`,
//! plugins') and installed plugins. Read-only: changes go through `claude mcp …`
//! and `claude plugin …`, so Kynex never edits the CLI's files. [`rows`] joins
//! the servers with a session's live status from its `init`.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::mcp::{self, Status};

/// Where a server is configured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// `mcpServers` in `~/.claude.json`: every folder.
    User,
    /// `projects["<folder>"].mcpServers` in `~/.claude.json`: this folder, only you.
    Local,
    /// `.mcp.json` in the folder, shared through the repo.
    Project,
    /// Comes with a plugin, by its id (`name@marketplace`).
    Plugin(String),
    /// A claude.ai connector, which comes with the claude.ai login.
    ClaudeAi,
    /// Reported by a session, but not found in any file Kynex reads.
    Other,
}

impl Source {
    pub fn label(&self) -> String {
        match self {
            Self::User => "User".into(),
            Self::Local => "Local".into(),
            Self::Project => "Project".into(),
            Self::Plugin(id) => format!("Plugin {}", id.split('@').next().unwrap_or(id)),
            Self::ClaudeAi => "claude.ai".into(),
            Self::Other => "Other".into(),
        }
    }
}

/// How Claude talks to a server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Transport {
    /// A local process.
    Stdio { command: String, args: Vec<String> },
    /// Streamable HTTP.
    Http { url: String },
    /// Server-sent events (the older HTTP transport).
    Sse { url: String },
    /// A `type` Kynex doesn't know.
    Other(String),
}

impl Transport {
    fn parse(v: &Value) -> Self {
        let s = |k: &str| v[k].as_str().unwrap_or("").to_string();
        match v["type"].as_str() {
            Some("stdio") | None if v["command"].is_string() => Self::Stdio {
                command: s("command"),
                args: v["args"].as_array().into_iter().flatten().filter_map(|a| a.as_str().map(Into::into)).collect(),
            },
            Some("http" | "streamable-http") | None if v["url"].is_string() => Self::Http { url: s("url") },
            Some("sse") => Self::Sse { url: s("url") },
            Some(other) => Self::Other(other.into()),
            None => Self::Other(String::new()),
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Self::Stdio { .. } => "stdio",
            Self::Http { .. } => "http",
            Self::Sse { .. } => "sse",
            Self::Other(t) if t.is_empty() => "unknown",
            Self::Other(t) => t,
        }
    }

    /// What it runs or where it connects: `npx @playwright/mcp@latest`, a URL.
    pub fn detail(&self) -> String {
        match self {
            Self::Stdio { command, args } => {
                std::iter::once(command).chain(args).cloned().collect::<Vec<_>>().join(" ")
            }
            Self::Http { url } | Self::Sse { url } => url.clone(),
            Self::Other(_) => String::new(),
        }
    }
}

/// Whether you've allowed a project server (`.mcp.json` ones ask first).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Approval {
    Approved,
    /// Not decided yet: the CLI won't start it until you allow it.
    Pending,
    Rejected,
}

/// An MCP server as configured. Its `env` and `headers` stay out: they hold secrets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Server {
    /// As the config names it; see [`Server::live_name`] for how sessions do.
    pub name: String,
    pub source: Source,
    pub transport: Transport,
    pub approval: Approval,
}

impl Server {
    /// The name a session's `init` reports it under: plugin servers are
    /// `plugin_<plugin>_<server>`.
    pub fn live_name(&self) -> String {
        match &self.source {
            Source::Plugin(id) => format!("plugin_{}_{}", id.split('@').next().unwrap_or(id), self.name),
            _ => self.name.clone(),
        }
    }
}

/// An installed plugin that applies to the folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plugin {
    /// `name@marketplace`.
    pub id: String,
    pub version: String,
    /// `user`, `project` or `local`.
    pub scope: String,
    pub enabled: bool,
    pub install: PathBuf,
}

impl Plugin {
    pub fn name(&self) -> &str {
        self.id.split('@').next().unwrap_or(&self.id)
    }

    pub fn marketplace(&self) -> &str {
        self.id.split_once('@').map_or("", |(_, m)| m)
    }
}

/// Everything configured for one folder.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Config {
    /// The servers sessions in the folder get, one per name: a local server
    /// hides a project one of the same name, which hides a user one.
    pub servers: Vec<Server>,
    pub plugins: Vec<Plugin>,
    /// Files that exist but couldn't be read, in words to show as-is.
    pub errors: Vec<String>,
}

/// Where the CLI keeps its config: `~/.claude.json` and `~/.claude/`, or both
/// inside `CLAUDE_CONFIG_DIR` when that's set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    pub global: PathBuf,
    pub dir: PathBuf,
}

impl Paths {
    pub fn detect() -> Option<Self> {
        if let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR").filter(|d| !d.is_empty()) {
            let dir = PathBuf::from(dir);
            return Some(Self { global: dir.join(".claude.json"), dir });
        }
        let home = dirs::home_dir()?;
        Some(Self { global: home.join(".claude.json"), dir: home.join(".claude") })
    }
}

/// What's configured for sessions in `cwd`. Blocks reading a few files.
pub fn load(cwd: &Path) -> Config {
    match Paths::detect() {
        Some(paths) => load_from(&paths, cwd),
        None => Config { errors: vec!["Couldn't find your home folder.".into()], ..Config::default() },
    }
}

/// [`load`] with the config files at `paths`. Blocks.
pub fn load_from(paths: &Paths, cwd: &Path) -> Config {
    let mut errors = Vec::new();
    let mut read = |path: &Path| {
        read_json(path).unwrap_or_else(|e| {
            errors.push(e);
            Value::Null
        })
    };
    let global = read(&paths.global);
    let mcp_json = read(&cwd.join(".mcp.json"));
    // Lowest first: each layer overrides the one before.
    let settings = [
        read(&paths.dir.join("settings.json")),
        read(&cwd.join(".claude").join("settings.json")),
        read(&cwd.join(".claude").join("settings.local.json")),
    ];
    let installed = read(&paths.dir.join("plugins").join("installed_plugins.json"));

    let plugins = plugins(&installed, &settings, cwd);
    let mut plugin_servers = Vec::new();
    for p in plugins.iter().filter(|p| p.enabled) {
        let (found, error) = plugin_manifest_servers(&p.install);
        errors.extend(error);
        plugin_servers.extend(found.into_iter().map(|(name, transport)| Server {
            name,
            source: Source::Plugin(p.id.clone()),
            transport,
            approval: Approval::Approved,
        }));
    }
    let mut servers = servers(&global, &mcp_json, &settings, cwd);
    servers.extend(plugin_servers);
    Config { servers, plugins, errors }
}

/// A JSON file, `Null` when it doesn't exist.
pub(crate) fn read_json(path: &Path) -> Result<Value, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Value::Null),
        Err(e) => return Err(format!("Couldn't read {}: {e}", path.display())),
    };
    serde_json::from_str(&text).map_err(|e| format!("{} isn't valid JSON: {e}", path.display()))
}

/// The user, local and project servers for `cwd`, from `~/.claude.json` (`global`),
/// the folder's `.mcp.json` and the settings layers (lowest first), one per name.
pub fn servers(global: &Value, mcp_json: &Value, settings: &[Value], cwd: &Path) -> Vec<Server> {
    let entry = global["projects"].as_object().and_then(|p| project_entry(p, cwd));
    let ask = |name: &str| approval(name, entry, settings);
    let mut out: Vec<Server> = Vec::new();
    let mut add = |found: Vec<(String, Transport)>, source: Source, approve: &dyn Fn(&str) -> Approval| {
        for (name, transport) in found {
            if !out.iter().any(|s| s.name == name) {
                let approval = approve(&name);
                out.push(Server { name, source: source.clone(), transport, approval });
            }
        }
    };
    let approved = |_: &str| Approval::Approved;
    add(entry.map(|e| parse_servers(&e["mcpServers"])).unwrap_or_default(), Source::Local, &approved);
    add(parse_mcp_json(mcp_json), Source::Project, &ask);
    add(parse_servers(&global["mcpServers"]), Source::User, &approved);
    out
}

/// `server`'s environment variables as configured. They can hold secrets (a
/// token), which is why [`Server`] leaves them out.
pub fn server_env(global: &Value, mcp_json: &Value, cwd: &Path, server: &Server) -> Vec<(String, String)> {
    let name = server.name.as_str();
    let config = match server.source {
        Source::Local => {
            global["projects"].as_object().and_then(|p| project_entry(p, cwd)).map(|e| &e["mcpServers"][name])
        }
        Source::Project if mcp_json.get("mcpServers").is_some() => Some(&mcp_json["mcpServers"][name]),
        Source::Project => Some(&mcp_json[name]),
        Source::User => Some(&global["mcpServers"][name]),
        Source::Plugin(_) | Source::ClaudeAi | Source::Other => None,
    };
    config
        .and_then(|c| c["env"].as_object())
        .into_iter()
        .flatten()
        .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
        .collect()
}

/// The servers turned off for `cwd` (`/mcp` → Disable), by name.
pub fn disabled_servers(global: &Value, cwd: &Path) -> Vec<String> {
    let entry = global["projects"].as_object().and_then(|p| project_entry(p, cwd));
    let list = entry.and_then(|e| e["disabledMcpServers"].as_array());
    list.into_iter().flatten().filter_map(|n| n.as_str().map(Into::into)).collect()
}

/// A `.mcp.json`: `{ "mcpServers": { … } }`, or (in some plugins) the map bare.
pub fn parse_mcp_json(v: &Value) -> Vec<(String, Transport)> {
    if v.get("mcpServers").is_some() { parse_servers(&v["mcpServers"]) } else { parse_servers(v) }
}

/// An `mcpServers` map, in the file's order.
pub fn parse_servers(map: &Value) -> Vec<(String, Transport)> {
    map.as_object()
        .into_iter()
        .flatten()
        .filter(|(_, v)| v.is_object())
        .map(|(name, v)| (name.clone(), Transport::parse(v)))
        .collect()
}

/// `~/.claude.json`'s entry for `cwd`. Keys are paths with `/`; on Windows the
/// same folder can appear in two cases (`System32`, `system32`), so an exact
/// match wins over a case-blind one.
fn project_entry<'a>(projects: &'a Map<String, Value>, cwd: &Path) -> Option<&'a Value> {
    let want = norm(&cwd.to_string_lossy());
    let blind = cfg!(any(windows, target_os = "macos"));
    projects
        .iter()
        .find(|(k, _)| norm(k) == want)
        .or_else(|| projects.iter().find(|(k, _)| blind && norm(k).eq_ignore_ascii_case(&want)))
        .map(|(_, v)| v)
}

/// A path for comparing: `/` separators, no trailing one.
fn norm(p: &str) -> String {
    let p = p.replace('\\', "/");
    let trimmed = p.trim_end_matches('/');
    if trimmed.is_empty() || trimmed.ends_with(':') { p } else { trimmed.into() }
}

/// Whether project server `name` may start: a `disabledMcpjsonServers` entry
/// anywhere rejects it, `enabledMcpjsonServers` or `enableAllProjectMcpServers`
/// allows it, otherwise the CLI asks.
fn approval(name: &str, entry: Option<&Value>, settings: &[Value]) -> Approval {
    let layers = || entry.into_iter().chain(settings);
    let lists = |key: &str| layers().any(|l| l[key].as_array().is_some_and(|a| a.iter().any(|n| n == name)));
    if lists("disabledMcpjsonServers") {
        Approval::Rejected
    } else if lists("enabledMcpjsonServers") || layers().any(|l| l["enableAllProjectMcpServers"] == true) {
        Approval::Approved
    } else {
        Approval::Pending
    }
}

/// The plugins in `installed_plugins.json` that apply to `cwd`: user-scoped
/// ones, and project or local ones installed for this folder. A plugin is
/// enabled when the highest settings layer that names it in `enabledPlugins`
/// says `true`. Sorted by id.
pub fn plugins(installed: &Value, settings: &[Value], cwd: &Path) -> Vec<Plugin> {
    let want = norm(&cwd.to_string_lossy());
    let here = |p: &str| {
        let p = norm(p);
        p == want || (cfg!(any(windows, target_os = "macos")) && p.eq_ignore_ascii_case(&want))
    };
    let mut out = Vec::new();
    for (id, installs) in installed["plugins"].as_object().into_iter().flatten() {
        let applies = installs.as_array().into_iter().flatten().find(|i| match i["scope"].as_str() {
            Some("user") => true,
            _ => i["projectPath"].as_str().is_some_and(here),
        });
        let Some(i) = applies else { continue };
        let enabled = settings.iter().rev().find_map(|s| s["enabledPlugins"][id].as_bool()).unwrap_or(false);
        out.push(Plugin {
            id: id.clone(),
            version: i["version"].as_str().unwrap_or("").into(),
            scope: i["scope"].as_str().unwrap_or("").into(),
            enabled,
            install: PathBuf::from(i["installPath"].as_str().unwrap_or("")),
        });
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// The servers a plugin brings: `mcpServers` in its `.claude-plugin/plugin.json`
/// (inline, or a path to a file), else its `.mcp.json`. Blocks.
fn plugin_manifest_servers(install: &Path) -> (Vec<(String, Transport)>, Option<String>) {
    let manifest = match read_json(&install.join(".claude-plugin").join("plugin.json")) {
        Ok(v) => v,
        Err(e) => return (Vec::new(), Some(e)),
    };
    let file = match &manifest["mcpServers"] {
        Value::Object(_) => return (parse_servers(&manifest["mcpServers"]), None),
        Value::String(rel) => install.join(rel),
        _ => install.join(".mcp.json"),
    };
    match read_json(&file) {
        Ok(v) => (parse_mcp_json(&v), None),
        Err(e) => (Vec::new(), Some(e)),
    }
}

/// One line of the MCP list: a server, where it's from and, when a session
/// reported it, how it's doing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// As sessions name it (`plugin_figma_figma`, `claude_ai_Claude_Docs`).
    pub name: String,
    pub source: Source,
    pub transport: Option<Transport>,
    pub approval: Approval,
    /// `None` until a session in the folder reports it.
    pub status: Option<Status>,
}

impl Row {
    /// "Edge", "Claude Docs": the name people know it by.
    pub fn label(&self) -> String {
        mcp::server_label(&self.name)
    }
}

/// The configured servers with their live status, then those only a session
/// reported (claude.ai connectors, mostly).
pub fn rows(configured: &[Server], live: &[mcp::Server]) -> Vec<Row> {
    let status = |name: &str| live.iter().find(|l| l.name == name).map(|l| l.status.clone());
    let mut out: Vec<Row> = configured
        .iter()
        .map(|s| {
            let name = s.live_name();
            Row {
                status: status(&name),
                name,
                source: s.source.clone(),
                transport: Some(s.transport.clone()),
                approval: s.approval,
            }
        })
        .collect();
    for l in live {
        if !out.iter().any(|r| r.name == l.name) {
            let source = if l.name.starts_with("claude_ai_") { Source::ClaudeAi } else { Source::Other };
            out.push(Row {
                name: l.name.clone(),
                source,
                transport: None,
                approval: Approval::Approved,
                status: Some(l.status.clone()),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn stdio(command: &str, args: &[&str]) -> Transport {
        Transport::Stdio { command: command.into(), args: args.iter().map(|a| a.to_string()).collect() }
    }

    #[test]
    fn parses_transports() {
        let t = |v| Transport::parse(&v);
        assert_eq!(t(json!({ "type": "stdio", "command": "npx", "args": ["x"] })), stdio("npx", &["x"]));
        assert_eq!(t(json!({ "command": "node" })), stdio("node", &[]));
        assert_eq!(t(json!({ "type": "http", "url": "https://a" })), Transport::Http { url: "https://a".into() });
        assert_eq!(t(json!({ "url": "https://a" })), Transport::Http { url: "https://a".into() });
        assert_eq!(t(json!({ "type": "sse", "url": "https://a" })), Transport::Sse { url: "https://a".into() });
        assert_eq!(t(json!({ "type": "ws" })).label(), "ws");
        assert_eq!(t(json!({})).label(), "unknown");
        assert_eq!(
            stdio("npx", &["@playwright/mcp@latest", "--extension"]).detail(),
            "npx @playwright/mcp@latest --extension"
        );
    }

    #[test]
    fn reads_both_mcp_json_shapes() {
        let wrapped = json!({ "mcpServers": { "figma": { "type": "http", "url": "https://f" } } });
        let bare = json!({ "playwright": { "command": "npx" }, "junk": 3 });
        assert_eq!(parse_mcp_json(&wrapped)[0].0, "figma");
        assert_eq!(parse_mcp_json(&bare), vec![("playwright".into(), stdio("npx", &[]))]);
        assert!(parse_mcp_json(&Value::Null).is_empty());
    }

    #[test]
    fn local_hides_project_hides_user() {
        let cwd = Path::new("/work/app");
        let global = json!({
            "mcpServers": { "edge": { "command": "user-edge" }, "gh": { "command": "gh" } },
            "projects": {
                "/work/other": { "mcpServers": { "nope": { "command": "x" } } },
                "/work/app": { "mcpServers": { "edge": { "command": "local-edge" } } },
            },
        });
        let mcp_json = json!({ "mcpServers": { "edge": { "command": "repo-edge" }, "db": { "command": "db" } } });
        let list = servers(&global, &mcp_json, &[], cwd);
        let names: Vec<_> = list.iter().map(|s| (s.name.as_str(), s.source.clone())).collect();
        assert_eq!(names, [("edge", Source::Local), ("db", Source::Project), ("gh", Source::User)]);
        assert_eq!(list[0].transport, stdio("local-edge", &[]));
        assert_eq!(list[1].approval, Approval::Pending);
    }

    #[test]
    fn env_comes_from_the_entry_that_won_and_disabled_ones_are_listed() {
        let cwd = Path::new("/work/app");
        let global = json!({
            "mcpServers": { "edge": { "command": "npx", "env": { "TOKEN": "user" } } },
            "projects": { "/work/app": {
                "mcpServers": { "gh": { "command": "gh", "env": { "TOKEN": "local", "N": 3 } } },
                "disabledMcpServers": ["edge"],
            } },
        });
        let mcp_json = json!({ "db": { "command": "db", "env": { "URL": "x" } } });
        let list = servers(&global, &mcp_json, &[], cwd);
        let env = |name: &str| server_env(&global, &mcp_json, cwd, list.iter().find(|s| s.name == name).unwrap());
        assert_eq!(env("edge"), [("TOKEN".to_string(), "user".to_string())]);
        // Only string values are kept.
        assert_eq!(env("gh"), [("TOKEN".to_string(), "local".to_string())]);
        assert_eq!(env("db"), [("URL".to_string(), "x".to_string())]);
        assert_eq!(disabled_servers(&global, cwd), ["edge"]);
        assert!(disabled_servers(&global, Path::new("/elsewhere")).is_empty());
    }

    #[test]
    fn project_servers_need_approval() {
        let cwd = Path::new("/w");
        let mcp_json =
            json!({ "mcpServers": { "a": { "command": "a" }, "b": { "command": "b" }, "c": { "command": "c" } } });
        let global =
            json!({ "projects": { "/w": { "enabledMcpjsonServers": ["a"], "disabledMcpjsonServers": ["b"] } } });
        let of = |settings: &[Value]| {
            servers(&global, &mcp_json, settings, cwd).iter().map(|s| s.approval).collect::<Vec<_>>()
        };
        assert_eq!(of(&[]), [Approval::Approved, Approval::Rejected, Approval::Pending]);
        let all = json!({ "enableAllProjectMcpServers": true });
        assert_eq!(of(&[all]), [Approval::Approved, Approval::Rejected, Approval::Approved]);
    }

    #[test]
    fn matches_project_keys() {
        let projects = json!({ "C:/Windows/System32": 1, "C:/Windows/system32": 2, "C:/Users/me/repo": 3 });
        let p = projects.as_object().unwrap();
        assert_eq!(project_entry(p, Path::new("C:/Windows/system32")), Some(&json!(2)), "exact case wins");
        assert_eq!(project_entry(p, Path::new("C:\\Users\\me\\repo\\")), Some(&json!(3)));
        let blind = project_entry(p, Path::new("c:/users/ME/repo"));
        assert_eq!(blind.is_some(), cfg!(any(windows, target_os = "macos")));
        assert_eq!(norm("C:/"), "C:/");
        assert_eq!(norm("/"), "/");
    }

    #[test]
    fn lists_plugins_for_the_folder() {
        let installed = json!({ "version": 2, "plugins": {
            "playwright@official": [{ "scope": "user", "version": "f8c5", "installPath": "/c/playwright" }],
            "figma@official": [{ "scope": "project", "projectPath": "c:\\work\\otto", "installPath": "/c/figma" }],
            "far@official": [{ "scope": "local", "projectPath": "/elsewhere", "installPath": "/c/far" }],
        } });
        let user = json!({ "enabledPlugins": { "playwright@official": true, "figma@official": true } });
        let local = json!({ "enabledPlugins": { "figma@official": false } });
        let list = plugins(&installed, &[user.clone(), local], Path::new("c:/work/otto"));
        let got: Vec<_> = list.iter().map(|p| (p.name(), p.enabled)).collect();
        assert_eq!(got, [("figma", false), ("playwright", true)], "the higher layer wins; far is elsewhere");
        assert_eq!(list[1].marketplace(), "official");
        assert_eq!(list[1].version, "f8c5");
        assert_eq!(plugins(&installed, &[user], Path::new("/w")).len(), 1, "only user ones elsewhere");
    }

    #[test]
    fn joins_live_status() {
        let edge = Server {
            name: "edge".into(),
            source: Source::User,
            transport: stdio("npx", &[]),
            approval: Approval::Approved,
        };
        let figma = Server {
            name: "figma".into(),
            source: Source::Plugin("figma@official".into()),
            transport: Transport::Http { url: "https://f".into() },
            approval: Approval::Approved,
        };
        let live = [
            mcp::Server { name: "plugin_figma_figma".into(), status: Status::NeedsAuth },
            mcp::Server { name: "claude_ai_Claude_Docs".into(), status: Status::Connected },
            mcp::Server { name: "mystery".into(), status: Status::Failed },
        ];
        let rows = rows(&[edge, figma], &live);
        let got: Vec<_> = rows.iter().map(|r| (r.name.as_str(), r.source.label(), r.status.clone())).collect();
        assert_eq!(
            got,
            [
                ("edge", "User".into(), None),
                ("plugin_figma_figma", "Plugin figma".into(), Some(Status::NeedsAuth)),
                ("claude_ai_Claude_Docs", "claude.ai".into(), Some(Status::Connected)),
                ("mystery", "Other".into(), Some(Status::Failed)),
            ]
        );
        assert_eq!(rows[2].label(), "Claude Docs");
    }

    /// Your own config for this repo, printed: `cargo test -p kynex-core reads_your_config -- --ignored --nocapture`.
    #[test]
    #[ignore = "reads your real ~/.claude.json"]
    fn reads_your_config() {
        let config = load(&std::env::current_dir().unwrap());
        for s in &config.servers {
            println!("{:<24} {:<20} {:<6} {:?}", s.live_name(), s.source.label(), s.transport.label(), s.approval);
        }
        for p in &config.plugins {
            println!("plugin {} {} {} enabled={}", p.id, p.version, p.scope, p.enabled);
        }
        assert!(config.errors.is_empty(), "{:?}", config.errors);
    }

    /// The whole thing on disk, laid out like the CLI does it.
    #[test]
    fn loads_from_files() {
        let root = std::env::temp_dir().join(format!("kynex-ext-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (dir, cwd, plugin) = (root.join(".claude"), root.join("repo"), root.join("plug"));
        for d in [dir.join("plugins"), cwd.clone(), plugin.join(".claude-plugin")] {
            std::fs::create_dir_all(d).unwrap();
        }
        let write = |p: PathBuf, v: Value| std::fs::write(p, v.to_string()).unwrap();
        let paths = Paths { global: root.join(".claude.json"), dir: dir.clone() };
        write(paths.global.clone(), json!({ "mcpServers": { "edge": { "type": "stdio", "command": "npx" } } }));
        write(dir.join("settings.json"), json!({ "enabledPlugins": { "pw@m": true } }));
        write(
            dir.join("plugins").join("installed_plugins.json"),
            json!({ "plugins": { "pw@m": [{ "scope": "user", "installPath": plugin.to_string_lossy() }] } }),
        );
        write(plugin.join(".claude-plugin").join("plugin.json"), json!({ "name": "pw" }));
        write(plugin.join(".mcp.json"), json!({ "pw": { "command": "npx" } }));
        std::fs::write(cwd.join(".mcp.json"), "{ not json").unwrap();

        let config = load_from(&paths, &cwd);
        let names: Vec<_> = config.servers.iter().map(Server::live_name).collect();
        assert_eq!(names, ["edge", "plugin_pw_pw"]);
        assert_eq!(config.plugins.len(), 1);
        assert_eq!(config.errors.len(), 1, "{:?}", config.errors);
        assert!(config.errors[0].contains("isn't valid JSON"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
