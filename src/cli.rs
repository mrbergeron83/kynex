//! Keeping the `claude` CLI current from inside Kynex: compare the installed
//! version with the newest release on the user's channel, and run the CLI's own
//! `claude update` when they accept. Everything here blocks; call it off the UI thread.

use std::process::{Command, Stdio};

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

fn command(program: &str) -> Command {
    let mut cmd = Command::new(program);
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd
}

/// The installed CLI's version, from `claude --version` ("2.1.310 (Claude Code)").
pub fn installed_version() -> Result<String, String> {
    let out = command("claude").arg("--version").output().map_err(|e| format!("failed to start `claude`: {e}"))?;
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

/// The installed version and the newest one, if the newest is ahead.
pub fn check() -> Result<Option<(String, String)>, String> {
    let installed = installed_version()?;
    let latest = latest_version()?;
    Ok(is_newer(&latest, &installed).then_some((installed, latest)))
}

/// Runs `claude update` (which also handles package-manager installs) and
/// returns the version installed afterwards.
pub fn update() -> Result<String, String> {
    let out = command("claude").arg("update").output().map_err(|e| format!("failed to start `claude update`: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let msg = if err.trim().is_empty() { String::from_utf8_lossy(&out.stdout) } else { err };
        return Err(format!("`claude update` failed: {}", msg.trim()));
    }
    installed_version()
}

/// `major.minor.patch`, ignoring any pre-release or build suffix.
fn parse(v: &str) -> Option<(u64, u64, u64)> {
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
    #[ignore = "needs the claude CLI and network"]
    fn checks_live() {
        let installed = installed_version().unwrap();
        let latest = latest_version().unwrap();
        assert!(parse(&installed).is_some() && parse(&latest).is_some());
    }
}
