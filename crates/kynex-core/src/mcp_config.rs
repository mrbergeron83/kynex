//! Changing which MCP servers Claude gets, through the CLI so Kynex never edits
//! its files: `claude mcp add` / `remove`, the browser presets (Playwright on
//! your own Edge or Chrome), and `claude mcp list` for every server's health
//! when no session is open. [`crate::extensions`] reads what's configured.

use std::path::{Path, PathBuf};

use crate::cli;
use crate::extensions::{Source, Transport};
use crate::mcp::{self, Status};

/// The browser extension `@playwright/mcp --extension` talks to. Edge installs
/// it from the Chrome Web Store too.
pub const BROWSER_EXTENSION_URL: &str =
    "https://chromewebstore.google.com/detail/playwright-mcp-bridge/mmlmfjhmonkocbjadbfplnigmagldckm";

/// The extension's status page, which shows its token.
const BROWSER_EXTENSION_STATUS: &str = "chrome-extension://mmlmfjhmonkocbjadbfplnigmagldckm/status.html";

/// With the extension's token here, `@playwright/mcp --extension` connects
/// without the extension asking each time.
pub const BROWSER_TOKEN_ENV: &str = "PLAYWRIGHT_MCP_EXTENSION_TOKEN";

/// Where `claude mcp add` writes a server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// This folder, only you (`~/.claude.json`).
    Local,
    /// This folder, shared through the repo (`.mcp.json`).
    Project,
    /// Every folder (`~/.claude.json`).
    User,
}

impl Scope {
    pub const ALL: [Self; 3] = [Self::Local, Self::Project, Self::User];

    fn arg(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Project => "project",
            Self::User => "user",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Local => "This folder, just you",
            Self::Project => "This folder, shared in .mcp.json",
            Self::User => "Every folder",
        }
    }

    /// The scope a configured server lives in, when Kynex may remove it there.
    pub fn of(source: &Source) -> Option<Self> {
        match source {
            Source::Local => Some(Self::Local),
            Source::Project => Some(Self::Project),
            Source::User => Some(Self::User),
            Source::Plugin(_) | Source::ClaudeAi | Source::Other => None,
        }
    }
}

/// A server to add.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewServer {
    pub name: String,
    pub scope: Scope,
    pub transport: Transport,
    /// Environment variables the server runs with.
    pub env: Vec<(String, String)>,
}

impl NewServer {
    /// A server from what was typed: a URL makes an HTTP server, anything else
    /// is a command line to run.
    pub fn parse(name: &str, scope: Scope, what: &str) -> Result<Self, String> {
        let name = name.trim();
        check_name(name)?;
        let what = what.trim();
        let transport = if what.starts_with("https://") || what.starts_with("http://") {
            Transport::Http { url: what.into() }
        } else {
            let mut words = split_command(what)?.into_iter();
            let command = words.next().ok_or("Say what to run, or the server's URL.")?;
            Transport::Stdio { command, args: words.collect() }
        };
        Ok(Self { name: name.into(), scope, transport, env: Vec::new() })
    }

    /// The `claude` arguments that add it.
    pub fn add_args(&self) -> Result<Vec<String>, String> {
        let mut args: Vec<String> = ["mcp", "add", "--scope", self.scope.arg()].map(Into::into).into();
        for (k, v) in &self.env {
            args.extend(["-e".into(), format!("{k}={v}")]);
        }
        match &self.transport {
            Transport::Stdio { command, args: rest } => {
                args.extend([self.name.clone(), "--".into(), command.clone()]);
                args.extend(rest.iter().cloned());
            }
            Transport::Http { url } => {
                args.extend(["--transport".into(), "http".into(), self.name.clone(), url.clone()])
            }
            Transport::Sse { url } => args.extend(["--transport".into(), "sse".into(), self.name.clone(), url.clone()]),
            Transport::Other(t) => return Err(format!("Kynex can't add a {t} server.")),
        }
        Ok(args)
    }
}

/// Names the CLI takes: letters, digits, `-` and `_`.
pub fn check_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        Err("Give the server a name.".into())
    } else if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        Err("Names use letters, digits, - and _ only.".into())
    } else {
        Ok(())
    }
}

/// A typed command line as words: spaces split them, and single or double
/// quotes keep spaces in one (`"C:\Program Files\node.exe" server.js`).
pub fn split_command(line: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut word: Option<String> = None;
    let mut quote = None;
    for c in line.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => word.get_or_insert_default().push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                word.get_or_insert_default();
            }
            (None, c) if c.is_whitespace() => words.extend(word.take()),
            (None, c) => word.get_or_insert_default().push(c),
        }
    }
    if quote.is_some() {
        return Err("A quote isn't closed.".into());
    }
    words.extend(word);
    Ok(words)
}

/// Adds `server` for sessions in `cwd`. Blocks on `claude mcp add`.
pub fn add(cwd: &Path, server: &NewServer) -> Result<(), String> {
    run(cwd, &server.add_args()?)
}

/// Removes server `name` from `scope`. Blocks on `claude mcp remove`.
pub fn remove(cwd: &Path, name: &str, scope: Scope) -> Result<(), String> {
    run(cwd, &["mcp", "remove", "--scope", scope.arg(), name].map(Into::into))
}

/// Every server's health for sessions in `cwd`, as a session would name them.
/// Blocks on `claude mcp list`, which starts each server: seconds.
pub fn check(cwd: &Path) -> Result<Vec<mcp::Server>, String> {
    let out = cli::command(cli::program())
        .args(["mcp", "list"])
        .current_dir(cwd)
        .output()
        .map_err(|e| format!("Couldn't start `claude`: {e}"))?;
    let text = String::from_utf8_lossy(&out.stdout);
    if !out.status.success() {
        return Err(failure("claude mcp list", &out));
    }
    Ok(parse_list(&text))
}

fn run(cwd: &Path, args: &[String]) -> Result<(), String> {
    let out = cli::command(cli::program())
        .args(args)
        .current_dir(cwd)
        .output()
        .map_err(|e| format!("Couldn't start `claude`: {e}"))?;
    if out.status.success() { Ok(()) } else { Err(failure(&format!("claude {}", args[..2].join(" ")), &out)) }
}

fn failure(what: &str, out: &std::process::Output) -> String {
    let said = cli::last_line(&out.stderr).or_else(|| cli::last_line(&out.stdout));
    format!("`{what}` failed: {}", said.unwrap_or_else(|| "no message".into()))
}

/// `claude mcp list`'s lines, `<name>: <command or url> - <mark> <status>`,
/// with names as sessions report them (`claude.ai Notion` is
/// `claude_ai_Notion`, `plugin:pw:pw` is `plugin_pw_pw`). Servers waiting for
/// approval are left out: they haven't been tried.
pub fn parse_list(text: &str) -> Vec<mcp::Server> {
    text.lines()
        .filter_map(|line| {
            let (name, rest) =
                cli::strip_escapes(line).split_once(": ").map(|(n, r)| (n.to_string(), r.to_string()))?;
            let (_, status) = rest.rsplit_once(" - ")?;
            let status = status.trim_start_matches(|c: char| !c.is_alphabetic()).trim().to_ascii_lowercase();
            let status = match status.as_str() {
                "connected" => Status::Connected,
                "needs authentication" => Status::NeedsAuth,
                s if s.starts_with("failed") => Status::Failed,
                s if s.starts_with("pending") => return None,
                s if s.starts_with("disabled") => Status::Disabled,
                "" => return None,
                other => Status::Other(other.into()),
            };
            let name = name.trim().chars().map(|c| if c.is_alphanumeric() || c == '-' { c } else { '_' }).collect();
            Some(mcp::Server { name, status })
        })
        .collect()
}

/// A browser Playwright can drive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Browser {
    Edge,
    Chrome,
}

impl Browser {
    pub const ALL: [Self; 2] = [Self::Edge, Self::Chrome];

    pub fn label(self) -> &'static str {
        match self {
            Self::Edge => "Edge",
            Self::Chrome => "Chrome",
        }
    }

    /// `@playwright/mcp`'s `--browser` value.
    fn channel(self) -> &'static str {
        match self {
            Self::Edge => "msedge",
            Self::Chrome => "chrome",
        }
    }

    /// The preset: `@playwright/mcp` on this browser, for every folder. With
    /// `own_profile` it attaches to your running browser through its extension
    /// (your logins stay), and with the extension's `token` without it asking
    /// each time; without, it opens a fresh browser with none.
    pub fn preset(self, own_profile: bool, token: &str) -> NewServer {
        let mut args: Vec<String> = ["@playwright/mcp@latest", "--browser", self.channel()].map(Into::into).into();
        let mut env = Vec::new();
        if own_profile {
            args.push("--extension".into());
            let token = token.trim();
            if !token.is_empty() {
                env.push((BROWSER_TOKEN_ENV.into(), token.into()));
            }
        }
        NewServer {
            name: format!("playwright-{}", self.label().to_ascii_lowercase()),
            scope: Scope::User,
            transport: Transport::Stdio { command: "npx".into(), args },
            env,
        }
    }

    /// Opens the Playwright extension's status page, which shows its token, in
    /// this browser (an extension page only opens in its own browser).
    pub fn open_token_page(self) -> Result<(), String> {
        let place = self.places().into_iter().find(|p| p.exists());
        let place = place.ok_or_else(|| format!("{} isn't installed.", self.label()))?;
        #[cfg(target_os = "macos")]
        let mut cmd = {
            let mut c = std::process::Command::new("open");
            c.arg("-a").arg(&place).arg(BROWSER_EXTENSION_STATUS);
            c
        };
        #[cfg(not(target_os = "macos"))]
        let mut cmd = {
            let mut c = std::process::Command::new(&place);
            c.arg(BROWSER_EXTENSION_STATUS);
            c
        };
        crate::process::detach(&mut cmd).map(|_| ()).map_err(|e| format!("couldn't open {}: {e}", self.label()))
    }

    /// Whether it's installed, by where its installers put it.
    pub fn installed(self) -> bool {
        self.places().iter().any(|p| p.exists())
    }

    #[cfg(windows)]
    fn places(self) -> Vec<PathBuf> {
        let rel = match self {
            Self::Edge => ["Microsoft", "Edge", "Application", "msedge.exe"],
            Self::Chrome => ["Google", "Chrome", "Application", "chrome.exe"],
        };
        ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"]
            .iter()
            .filter_map(std::env::var_os)
            .map(|root| rel.iter().fold(PathBuf::from(root), |p, part| p.join(part)))
            .collect()
    }

    #[cfg(target_os = "macos")]
    fn places(self) -> Vec<PathBuf> {
        let app = match self {
            Self::Edge => "Microsoft Edge.app",
            Self::Chrome => "Google Chrome.app",
        };
        let mut out = vec![Path::new("/Applications").join(app)];
        out.extend(dirs::home_dir().map(|h| h.join("Applications").join(app)));
        out
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    fn places(self) -> Vec<PathBuf> {
        let names: &[&str] = match self {
            Self::Edge => &["microsoft-edge", "microsoft-edge-stable"],
            Self::Chrome => &["google-chrome", "google-chrome-stable"],
        };
        names.iter().filter_map(|n| on_path(n)).collect()
    }
}

/// Whether `npx` is on PATH; the browser presets run through it.
pub fn npx_found() -> bool {
    on_path(if cfg!(windows) { "npx.cmd" } else { "npx" }).is_some()
}

pub(crate) fn on_path(exe: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths).map(|dir| dir.join(exe)).find(|p| p.is_file())
}

/// The page a browser test opens, and what its title must say.
const TEST_PAGE: (&str, &str) = ("https://example.com", "Example Domain");

/// Tries browser server `server` (as sessions name it) for real: a one-off
/// `claude -p` on the smallest model that may only open a page with it, and
/// must report the page's title. `Ok` with what it read, or why it failed.
/// Blocks for the whole run (tens of seconds; a little quota).
pub fn test_browser(cwd: &Path, server: &str) -> Result<String, String> {
    let tool = format!("mcp__{}__browser_navigate", mcp::tool_spelling(server));
    let prompt = format!(
        "Open {} with the {tool} tool, then reply with only the page's title. \
         If the tool fails or isn't available, reply with ERROR: and what it said.",
        TEST_PAGE.0
    );
    let out = cli::command(cli::program())
        .args(["-p", &prompt, "--model", "haiku", "--output-format", "json", "--max-turns", "4"])
        // A test, not a conversation: nothing for the sessions list.
        .arg("--no-session-persistence")
        .args(["--allowedTools", &tool, "--disallowedTools", "Bash,Read,Write,Edit,WebFetch,WebSearch"])
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("Couldn't start `claude`: {e}"))?;
    let line = String::from_utf8_lossy(&out.stdout);
    let line = line.lines().rev().find(|l| l.trim_start().starts_with('{')).unwrap_or("");
    match serde_json::from_str::<serde_json::Value>(line) {
        Ok(v) => test_verdict(&v),
        Err(_) => Err(failure("claude -p", &out)),
    }
}

/// A browser test's `result` event: did the page's title come back?
pub(crate) fn test_verdict(v: &serde_json::Value) -> Result<String, String> {
    let said = v["result"].as_str().unwrap_or("").trim();
    if v["is_error"] != true && said.contains(TEST_PAGE.1) {
        return Ok(format!("Opened {} and read its title.", TEST_PAGE.0));
    }
    let said = said.strip_prefix("ERROR:").unwrap_or(said).trim();
    Err(if said.is_empty() { "The test run ended without an answer.".into() } else { said.chars().take(400).collect() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn judges_browser_tests() {
        use serde_json::json;
        assert!(test_verdict(&json!({ "result": "Example Domain", "is_error": false })).is_ok());
        assert_eq!(test_verdict(&json!({ "result": "ERROR: no browser" })), Err("no browser".into()));
        assert!(test_verdict(&json!({ "result": "Example Domain", "is_error": true })).is_err());
        assert!(test_verdict(&json!({})).is_err());
    }

    #[test]
    fn splits_command_lines() {
        assert_eq!(
            split_command("npx  @playwright/mcp@latest --extension").unwrap(),
            ["npx", "@playwright/mcp@latest", "--extension"]
        );
        assert_eq!(
            split_command(r#""C:\Program Files\node.exe" 'a b' """#).unwrap(),
            [r"C:\Program Files\node.exe", "a b", ""]
        );
        assert!(split_command("  ").unwrap().is_empty());
        assert!(split_command("node \"oops").is_err());
    }

    #[test]
    fn reads_what_was_typed() {
        let s = NewServer::parse(" gh ", Scope::Local, "https://api.example/mcp").unwrap();
        assert_eq!(s.transport, Transport::Http { url: "https://api.example/mcp".into() });
        assert_eq!(
            s.add_args().unwrap(),
            ["mcp", "add", "--scope", "local", "--transport", "http", "gh", "https://api.example/mcp"]
        );
        let s = NewServer::parse("db", Scope::Project, "node server.js --port 3").unwrap();
        assert_eq!(
            s.add_args().unwrap(),
            ["mcp", "add", "--scope", "project", "db", "--", "node", "server.js", "--port", "3"]
        );
        assert!(NewServer::parse("", Scope::User, "x").is_err());
        assert!(NewServer::parse("a b", Scope::User, "x").is_err());
        assert!(NewServer::parse("a", Scope::User, "  ").is_err());
    }

    #[test]
    fn browser_presets() {
        let edge = Browser::Edge.preset(true, "");
        assert_eq!(edge.name, "playwright-edge");
        assert_eq!(edge.scope, Scope::User);
        assert_eq!(edge.transport.detail(), "npx @playwright/mcp@latest --browser msedge --extension");
        assert!(edge.env.is_empty());
        assert_eq!(Browser::Chrome.preset(false, "").transport.detail(), "npx @playwright/mcp@latest --browser chrome");
        // The token goes in the server's environment, and only with your own browser.
        let edge = Browser::Edge.preset(true, " tok-1 ");
        assert_eq!(
            edge.add_args().unwrap(),
            [
                "mcp",
                "add",
                "--scope",
                "user",
                "-e",
                "PLAYWRIGHT_MCP_EXTENSION_TOKEN=tok-1",
                "playwright-edge",
                "--",
                "npx",
                "@playwright/mcp@latest",
                "--browser",
                "msedge",
                "--extension"
            ]
        );
        assert!(Browser::Chrome.preset(false, "tok-1").env.is_empty());
    }

    #[test]
    fn only_config_scopes_are_removable() {
        assert_eq!(Scope::of(&Source::Project), Some(Scope::Project));
        assert_eq!(Scope::of(&Source::Plugin("pw@m".into())), None);
        assert_eq!(Scope::of(&Source::ClaudeAi), None);
    }

    #[test]
    fn parses_mcp_list() {
        let text = "Checking MCP server health…\n\n\
            claude.ai Claude Docs: https://api.anthropic.com/v1/pages/mcp - √ Connected\n\
            claude.ai Notion: https://mcp.notion.com/mcp - ! Needs authentication\n\
            plugin:playwright:playwright: npx @playwright/mcp@latest - ✓ Connected\n\
            playwright-edge: npx a - b --extension - ✗ Failed to connect\n\
            repo-db: node db.js - ⏸ Pending approval\n\
            odd: x - ? Something new\n";
        let got: Vec<_> = parse_list(text).into_iter().map(|s| (s.name, s.status)).collect();
        assert_eq!(
            got,
            [
                ("claude_ai_Claude_Docs".to_string(), Status::Connected),
                ("claude_ai_Notion".into(), Status::NeedsAuth),
                ("plugin_playwright_playwright".into(), Status::Connected),
                ("playwright-edge".into(), Status::Failed),
                ("odd".into(), Status::Other("something new".into())),
            ]
        );
        assert!(parse_list("No MCP servers configured.").is_empty());
    }

    /// `cargo test -p kynex-core mcp_list_live -- --ignored --nocapture`
    #[test]
    #[ignore = "needs the claude CLI; starts your MCP servers"]
    fn mcp_list_live() {
        for s in check(&std::env::current_dir().unwrap()).unwrap() {
            println!("{:<32} {:?}", s.name, s.status);
        }
        println!("npx: {}, edge: {}, chrome: {}", npx_found(), Browser::Edge.installed(), Browser::Chrome.installed());
    }
}
