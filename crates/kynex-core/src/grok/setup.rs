//! Setting Grok up from Kynex, so nobody needs a terminal: installing Grok
//! Build with xAI's script, signing in (`grok login`, in the browser), signing
//! out, and saying where it stands.

use std::path::PathBuf;

use crate::cli::{Login, command, last_line};

/// Where Grok stands on this machine.
#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    /// No `grok` found.
    Missing,
    /// Installed (this version), nobody signed in.
    SignedOut(String),
    /// Installed and signed in (or an `XAI_API_KEY` in the environment).
    Ready(String),
}

impl Status {
    /// In words, for Settings: "Grok Build 1.0.70 · signed in".
    pub fn describe(&self) -> String {
        match self {
            Status::Missing => "Grok Build isn't installed.".into(),
            Status::SignedOut(v) => format!("Grok Build {v} · not signed in"),
            Status::Ready(v) => format!("Grok Build {v} · signed in"),
        }
    }
}

/// `~/.grok/auth.json`, where `grok login` keeps its credentials.
fn auth_file() -> Option<PathBuf> {
    Some(dirs::home_dir()?.join(".grok").join("auth.json"))
}

/// Where Grok stands. Blocks on `grok --version`.
pub fn status() -> Status {
    let program = super::program();
    if !program.is_file() {
        return Status::Missing;
    }
    let version = command(&program)
        .arg("--version")
        .output()
        .ok()
        .and_then(|o| version_in(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or_else(|| "(unknown version)".into());
    let key = std::env::var_os("XAI_API_KEY").is_some_and(|k| !k.is_empty());
    let signed_in = auth_file().and_then(|p| std::fs::metadata(p).ok()).is_some_and(|m| m.len() > 2);
    if key || signed_in { Status::Ready(version) } else { Status::SignedOut(version) }
}

/// The version in `grok --version`'s output: "grok 1.0.70 (f50904eaee6c) [stable]".
fn version_in(text: &str) -> Option<String> {
    let mut words = text.split_whitespace();
    let first = words.next()?;
    let v = if first.eq_ignore_ascii_case("grok") { words.next()? } else { first };
    v.chars().next().is_some_and(|c| c.is_ascii_digit()).then(|| v.to_string())
}

/// Installs Grok Build with xAI's script (PowerShell on Windows, bash elsewhere).
/// Blocks until it's done.
pub fn install() -> Result<(), String> {
    let mut cmd = if cfg!(windows) {
        let mut c = command("powershell");
        c.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", "irm https://x.ai/cli/install.ps1 | iex"]);
        c
    } else {
        let mut c = command("bash");
        c.args(["-c", "curl -fsSL https://x.ai/cli/install.sh | bash"]);
        c
    };
    let out = cmd.output().map_err(|e| format!("couldn't start the installer: {e}"))?;
    super::recheck();
    if !out.status.success() || !super::installed() {
        return Err(format!(
            "The Grok Build installer failed: {}",
            last_line(&out.stderr).or(last_line(&out.stdout)).unwrap_or_default()
        ));
    }
    Ok(())
}

/// Starts `grok login` in the browser; watch it with [`Login::finished`].
pub fn login() -> Result<Login, String> {
    let mut cmd = command(super::program());
    cmd.args(["login", "--oauth"]);
    Login::run(cmd, "`grok login`")
}

/// Signs Grok out (`grok logout`). Blocks.
pub fn logout() -> Result<(), String> {
    let out = command(super::program()).arg("logout").output().map_err(|e| format!("failed to start `grok`: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    Err(format!("Couldn't sign Grok out: {}", last_line(&out.stderr).or(last_line(&out.stdout)).unwrap_or_default()))
}

/// The installed Grok Build version, from `grok --version`. Blocks.
pub fn installed_version() -> Result<String, String> {
    let program = super::program();
    if !program.is_file() {
        return Err("Grok Build isn't installed.".into());
    }
    let out = command(&program).arg("--version").output().map_err(|e| format!("failed to start `grok`: {e}"))?;
    let text = String::from_utf8_lossy(&out.stdout);
    version_in(&text).ok_or_else(|| format!("unexpected `grok --version` output: {}", text.trim()))
}

/// Checks for a newer Grok Build via `grok update --check --json`, returning
/// `(installed, latest)`. Blocks on the CLI and network.
pub fn check() -> Result<(String, String), String> {
    let program = super::program();
    if !program.is_file() {
        return Err("Grok Build isn't installed.".into());
    }
    let out = command(&program)
        .args(["update", "--check", "--json"])
        .output()
        .map_err(|e| format!("failed to start `grok update --check`: {e}"))?;
    if !out.status.success() && out.stdout.is_empty() {
        return Err(format!(
            "`grok update --check` failed: {}",
            last_line(&out.stderr).or(last_line(&out.stdout)).unwrap_or_default()
        ));
    }
    parse_update_check(&out.stdout)
}

fn parse_update_check(json: &[u8]) -> Result<(String, String), String> {
    let v: serde_json::Value =
        serde_json::from_slice(json).map_err(|e| format!("bad `grok update --check` output: {e}"))?;
    if let Some(err) = v["error"].as_str().map(str::trim).filter(|s| !s.is_empty()) {
        return Err(err.to_string());
    }
    let current = v["currentVersion"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or("`grok update --check` gave no currentVersion")?
        .to_string();
    let latest = v["latestVersion"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map_or_else(|| current.clone(), str::to_string);
    Ok((current, latest))
}

/// Runs `grok update` and returns the version installed afterwards. Blocks.
pub fn update() -> Result<String, String> {
    let out =
        command(super::program()).arg("update").output().map_err(|e| format!("failed to start `grok update`: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "`grok update` failed: {}",
            last_line(&out.stderr).or(last_line(&out.stdout)).unwrap_or_default()
        ));
    }
    match status() {
        Status::Ready(v) | Status::SignedOut(v) => Ok(v),
        Status::Missing => Err("Grok Build wasn't found after updating.".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_version() {
        assert_eq!(version_in("grok 1.0.70 (f50904eaee6c) [stable]\n").as_deref(), Some("1.0.70"));
        assert_eq!(version_in("1.2.3").as_deref(), Some("1.2.3"));
        assert_eq!(version_in("error: nope"), None);
        assert_eq!(version_in(""), None);
    }

    #[test]
    fn says_where_it_stands() {
        assert_eq!(Status::Ready("1.0.70".into()).describe(), "Grok Build 1.0.70 · signed in");
        assert!(Status::SignedOut("1".into()).describe().contains("not signed in"));
        assert!(Status::Missing.describe().contains("isn't installed"));
    }

    #[test]
    fn parses_update_check_json() {
        let avail = br#"{"currentVersion":"1.0.77","latestVersion":"1.0.92","updateAvailable":true,"error":null}"#;
        assert_eq!(parse_update_check(avail), Ok(("1.0.77".into(), "1.0.92".into())));

        let up_to_date = br#"{"currentVersion":"1.0.92","latestVersion":null,"updateAvailable":false,"error":null}"#;
        assert_eq!(parse_update_check(up_to_date), Ok(("1.0.92".into(), "1.0.92".into())));

        let failed = br#"{"currentVersion":"1.0.77","latestVersion":null,"updateAvailable":false,"error":"offline"}"#;
        assert_eq!(parse_update_check(failed), Err("offline".into()));

        assert!(parse_update_check(b"not json").is_err());
    }
}
