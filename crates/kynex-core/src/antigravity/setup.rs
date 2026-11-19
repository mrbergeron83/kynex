//! Setting Antigravity up from Kynex, so nobody needs a terminal: installing
//! `agy` with Google's script, and saying where it stands. There's no sign-in:
//! it runs on the OpenRouter key ([`super::command`]).

use crate::cli::{command, last_line};

/// Where Antigravity stands on this machine.
#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    /// No `agy` found.
    Missing,
    /// Installed, this version.
    Ready(String),
}

impl Status {
    /// In words, for Settings: "Antigravity CLI 1.5.12 · on your OpenRouter key".
    pub fn describe(&self) -> String {
        match self {
            Status::Missing => "The Antigravity CLI isn't installed.".into(),
            Status::Ready(v) => format!("Antigravity CLI {v} · on your OpenRouter key"),
        }
    }
}

/// Where Antigravity stands. Blocks on `agy --version`.
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
    Status::Ready(version)
}

/// The version in `agy --version`'s output: "1.5.12".
fn version_in(text: &str) -> Option<String> {
    let mut words = text.split_whitespace();
    let first = words.next()?;
    let v = if first.eq_ignore_ascii_case("agy") { words.next()? } else { first };
    v.chars().next().is_some_and(|c| c.is_ascii_digit()).then(|| v.to_string())
}

/// Installs `agy` with Google's script (PowerShell on Windows, bash elsewhere).
/// The script checks the binary's SHA-512 against Google's manifest. Blocks until it's done.
pub fn install() -> Result<(), String> {
    let mut cmd = if cfg!(windows) {
        let mut c = command("powershell");
        c.args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            "irm https://antigravity.google/cli/install.ps1 | iex",
        ]);
        c
    } else {
        let mut c = command("bash");
        c.args(["-c", "curl -fsSL https://antigravity.google/cli/install.sh | bash"]);
        c
    };
    let out = cmd.output().map_err(|e| format!("couldn't start the installer: {e}"))?;
    super::recheck();
    if !out.status.success() || !super::installed() {
        return Err(format!(
            "The Antigravity CLI installer failed: {}",
            last_line(&out.stderr).or(last_line(&out.stdout)).unwrap_or_default()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_version() {
        assert_eq!(version_in("1.5.12\n").as_deref(), Some("1.5.12"));
        assert_eq!(version_in("agy 1.5.12").as_deref(), Some("1.5.12"));
        assert_eq!(version_in("error: nope"), None);
        assert_eq!(version_in(""), None);
    }

    #[test]
    fn says_where_it_stands() {
        assert_eq!(Status::Ready("1.5.12".into()).describe(), "Antigravity CLI 1.5.12 · on your OpenRouter key");
        assert!(Status::Missing.describe().contains("isn't installed"));
    }
}
