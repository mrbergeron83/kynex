//! Managing the `claude` CLI from inside Kynex, so users never need a terminal:
//! find it, install it with the official installer, sign in, and keep it current
//! (compare the installed version with the newest release on the user's channel,
//! and run the CLI's own `claude update` when they accept). Everything here blocks;
//! call it off the UI thread.

use std::ffi::OsStr;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex, RwLock};

/// A `claude` picked in the app settings, for unusual installs.
static OVERRIDE: RwLock<Option<PathBuf>> = RwLock::new(None);

/// Points every `claude` Kynex runs at `path`, or with `None` back to [`found`].
pub fn set_program(path: Option<PathBuf>) {
    if let Ok(mut o) = OVERRIDE.write() {
        *o = path.filter(|p| !p.as_os_str().is_empty());
    }
}

/// The `claude` to run: the one picked with [`set_program`] while it exists,
/// else the one [`found`].
pub fn program() -> PathBuf {
    let picked = OVERRIDE.read().ok().and_then(|o| o.clone());
    picked.filter(|p| p.is_file()).unwrap_or_else(found)
}

/// The `claude` found on its own: the one on PATH, else the native installer's
/// location (`~/.local/bin`), which a fresh install puts it in before Kynex's PATH knows.
pub fn found() -> PathBuf {
    let exe = if cfg!(windows) { "claude.exe" } else { "claude" };
    let on_path = std::env::var_os("PATH")
        .and_then(|paths| std::env::split_paths(&paths).map(|dir| dir.join(exe)).find(|p| p.is_file()));
    let local = || dirs::home_dir().map(|h| h.join(".local").join("bin").join(exe)).filter(|p| p.is_file());
    on_path.or_else(local).unwrap_or_else(|| PathBuf::from("claude"))
}

/// Whether the CLI can run sessions yet.
#[derive(Clone, Copy, PartialEq)]
pub enum Status {
    Ready,
    Missing,
    LoggedOut,
}

/// Asks `claude auth status`. Anything unexpected (an older CLI without `auth`,
/// odd output) counts as ready, so this never locks out a working setup.
pub fn status() -> Status {
    let out = match command(program()).args(["auth", "status", "--json"]).output() {
        Ok(out) => out,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Status::Missing,
        Err(_) => return Status::Ready,
    };
    match serde_json::from_slice::<serde_json::Value>(&out.stdout) {
        Ok(v) if v["loggedIn"] == false && v["apiProvider"] == "firstParty" => Status::LoggedOut,
        _ => Status::Ready,
    }
}

/// Installs the native build with the official installer script.
pub fn install() -> Result<(), String> {
    let mut cmd = if cfg!(windows) {
        let mut c = command("powershell");
        c.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", "irm https://claude.ai/install.ps1 | iex"]);
        c
    } else {
        let mut c = command("bash");
        c.args(["-c", "curl -fsSL https://claude.ai/install.sh | bash"]);
        c
    };
    let out = cmd.output().map_err(|e| format!("couldn't start the installer: {e}"))?;
    if !out.status.success() || status() == Status::Missing {
        return Err(format!(
            "The Claude Code installer failed: {}",
            last_line(&out.stderr).or(last_line(&out.stdout)).unwrap_or_default()
        ));
    }
    Ok(())
}

/// A running `claude auth login`: it opens the browser itself, prints the sign-in
/// URL, and takes the code that page shows if the browser can't hand it back.
pub struct Login {
    child: Child,
    stdin: Option<ChildStdin>,
    output: Arc<Mutex<Vec<String>>>,
}

impl Login {
    /// `console` signs in with an Anthropic Console (API billing) account instead
    /// of a Claude subscription.
    pub fn start(console: bool) -> Result<Self, String> {
        let mut cmd = command(program());
        cmd.args(["auth", "login"]).stdin(Stdio::piped());
        if console {
            cmd.arg("--console");
        }
        let mut child = cmd.spawn().map_err(|e| format!("couldn't start `claude auth login`: {e}"))?;
        let output = Arc::new(Mutex::new(Vec::new()));
        let collect = |stream: Box<dyn std::io::Read + Send>| {
            let lines = output.clone();
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stream);
                let mut buf = Vec::new();
                while reader.read_until(b'\n', &mut buf).is_ok_and(|n| n > 0) {
                    lines.lock().unwrap().push(strip_escapes(&String::from_utf8_lossy(&buf)));
                    buf.clear();
                }
            });
        };
        collect(Box::new(child.stdout.take().unwrap()));
        collect(Box::new(child.stderr.take().unwrap()));
        Ok(Self { stdin: child.stdin.take(), child, output })
    }

    /// The sign-in page, once the CLI has printed it.
    pub fn url(&self) -> Option<String> {
        self.output.lock().unwrap().iter().find_map(|l| {
            let start = l.find("https://")?;
            Some(l[start..].split_whitespace().next()?.to_string())
        })
    }

    pub fn send_code(&mut self, code: &str) -> Result<(), String> {
        let stdin = self.stdin.as_mut().ok_or("sign-in already finished")?;
        writeln!(stdin, "{}", code.trim())
            .and_then(|_| stdin.flush())
            .map_err(|e| format!("couldn't send the code: {e}"))
    }

    /// `None` while it runs; then whether it signed in, or what it said last.
    pub fn finished(&mut self) -> Option<Result<(), String>> {
        let exit = self.child.try_wait().ok()??;
        if exit.success() {
            return Some(Ok(()));
        }
        let said = self.output.lock().unwrap().iter().rev().find(|l| !l.trim().is_empty()).cloned();
        Some(Err(said.map_or("Sign-in didn't complete.".into(), |s| s.trim().to_string())))
    }
}

impl Drop for Login {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Drops terminal escapes (colours, OSC 8 hyperlinks, which also repeat the URL).
pub(crate) fn strip_escapes(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        match chars.next() {
            // CSI: up to a final byte in @..~
            Some('[') => while chars.next().is_some_and(|c| !('@'..='~').contains(&c)) {},
            // OSC: up to BEL or ESC \, dropping the payload (the hyperlink target).
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\x07' || c == '\x1b' && chars.next_if_eq(&'\\').is_some() {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

pub(crate) fn last_line(bytes: &[u8]) -> Option<String> {
    let text = strip_escapes(&String::from_utf8_lossy(bytes));
    text.lines().rev().find(|l| !l.trim().is_empty()).map(|l| l.trim().to_string())
}

/// Where the update check stands; the composer shows a strip for every state but `Idle`.
#[derive(Clone, PartialEq)]
pub enum UpdateState {
    Idle,
    Available { installed: String, latest: String },
    Updating { latest: String },
    Updated { version: String },
    Failed(String),
}

/// npm publishes the CLI's release channels (`latest`, `stable`) as dist-tags.
const DIST_TAGS: &str = "https://registry.npmjs.org/-/package/@anthropic-ai/claude-code/dist-tags";

pub(crate) fn command(program: impl AsRef<OsStr>) -> Command {
    let mut cmd = Command::new(program);
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    crate::process::dies_with_parent(&mut cmd);
    cmd
}

/// The installed CLI's version, from `claude --version` ("2.1.310 (Claude Code)").
pub fn installed_version() -> Result<String, String> {
    version_of(&program())
}

/// The version `program` reports, from `<program> --version`.
pub fn version_of(program: &Path) -> Result<String, String> {
    let out = command(program).arg("--version").output().map_err(|e| format!("failed to start `claude`: {e}"))?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.split_whitespace()
        .next()
        .filter(|v| parse(v).is_some())
        .map(str::to_string)
        .ok_or_else(|| format!("unexpected `claude --version` output: {}", text.trim()))
}

/// The release channel the CLI follows: `autoUpdatesChannel` in `~/.claude/settings.json`,
/// `latest` unless it says `stable`.
fn channel() -> &'static str {
    let stable = dirs::home_dir()
        .and_then(|h| std::fs::read_to_string(h.join(".claude").join("settings.json")).ok())
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .is_some_and(|v| v["autoUpdatesChannel"] == "stable");
    if stable { "stable" } else { "latest" }
}

/// The newest release on the user's channel. Uses `curl`, which ships with
/// Windows 10+, macOS and every mainstream Linux, so Kynex needs no HTTP stack.
pub fn latest_version() -> Result<String, String> {
    let out = command("curl")
        .args(["-fsSL", "--max-time", "15", DIST_TAGS])
        .output()
        .map_err(|e| format!("failed to start `curl`: {e}"))?;
    if !out.status.success() {
        return Err(format!("couldn't fetch Claude Code releases: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    let tags: serde_json::Value =
        serde_json::from_slice(&out.stdout).map_err(|e| format!("bad Claude Code release list: {e}"))?;
    tags[channel()].as_str().map(str::to_string).ok_or_else(|| format!("no `{}` release listed", channel()))
}

/// The installed version and the newest on the user's channel; [`is_newer`]
/// says whether there's an update. Blocks on a process and a network request.
pub fn check() -> Result<(String, String), String> {
    Ok((installed_version()?, latest_version()?))
}

/// Runs `claude update` (which also handles package-manager installs) and
/// returns the version installed afterwards.
pub fn update() -> Result<String, String> {
    let out = command(program()).arg("update").output().map_err(|e| format!("failed to start `claude update`: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let msg = if err.trim().is_empty() { String::from_utf8_lossy(&out.stdout) } else { err };
        return Err(format!("`claude update` failed: {}", msg.trim()));
    }
    installed_version()
}

/// `major.minor.patch`, ignoring any pre-release or build suffix.
pub(crate) fn parse(v: &str) -> Option<(u64, u64, u64)> {
    let core = v.trim().split(['-', '+']).next()?;
    let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
    let version = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(version)
}

pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse(candidate), parse(current)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions() {
        assert!(is_newer("2.1.311", "2.1.310"));
        assert!(is_newer("2.10.0", "2.9.99"));
        assert!(is_newer("3.0.0", "2.99.99"));
        assert!(!is_newer("2.1.310", "2.1.310"));
        assert!(!is_newer("2.1.308", "2.1.310"));
        assert!(!is_newer("garbage", "2.1.310"));
        assert!(is_newer("2.1.311-beta.1", "2.1.310"));
        assert_eq!(parse("2.1"), None);
        assert_eq!(parse("2.1.3.4"), None);
    }

    #[test]
    fn strips_escapes() {
        let url = "https://claude.com/cai/oauth/authorize?code=true";
        let line = format!("If the browser didn't open, visit: \x1b]8;;{url}\x1b\\{url}\x1b]8;;\x1b\\\n");
        assert_eq!(strip_escapes(&line), format!("If the browser didn't open, visit: {url}\n"));
        assert_eq!(strip_escapes("\x1b[1;32mok\x1b[0m"), "ok");
        assert_eq!(strip_escapes("\x1b]8;;x\x07y\x1b]8;;\x07"), "y");
    }

    #[test]
    fn a_picked_program_wins_while_it_exists() {
        let picked = std::env::temp_dir().join(format!("kynex-claude-{}", std::process::id()));
        std::fs::write(&picked, "").unwrap();
        set_program(Some(picked.clone()));
        assert_eq!(program(), picked);
        std::fs::remove_file(&picked).unwrap();
        assert_eq!(program(), found(), "a picked program that's gone falls back");
        set_program(Some(PathBuf::new()));
        assert_eq!(program(), found());
        set_program(None);
    }

    #[test]
    #[ignore = "needs the claude CLI and network"]
    fn checks_live() {
        let installed = installed_version().unwrap();
        let latest = latest_version().unwrap();
        assert!(parse(&installed).is_some() && parse(&latest).is_some());
    }
}
