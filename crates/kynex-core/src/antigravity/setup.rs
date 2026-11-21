//! Setting Antigravity up from Kynex, so nobody needs a terminal: installing
//! `agy` with Google's script, and saying where it stands. There's no sign-in:
//! it runs on the OpenRouter key ([`super::command`]).

use crate::cli::{command, last_line};

/// Base URL of Antigravity CLI's official release manifests (same endpoint as `install.ps1` / `install.sh`).
const MANIFEST_BASE_URL: &str = "https://antigravity-cli-auto-updater-974169037036.us-central1.run.app/manifests";

/// Where Antigravity stands on this machine.
#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    /// No `agy` found.
    Missing,
    /// Installed, this version.
    Ready(String),
}

impl Status {
    /// In words, for Settings: "Antigravity CLI 1.5.12".
    pub fn describe(&self) -> String {
        match self {
            Status::Missing => "The Antigravity CLI isn't installed.".into(),
            Status::Ready(v) => format!("Antigravity CLI {v}"),
        }
    }
}

/// The installed `agy` version, from `agy --version`. Blocks.
pub fn installed_version() -> Result<String, String> {
    let program = super::program();
    if !program.is_file() {
        return Err("Antigravity CLI isn't installed.".into());
    }
    let out = command(&program).arg("--version").output().map_err(|e| format!("failed to start `agy`: {e}"))?;
    let text = String::from_utf8_lossy(&out.stdout);
    version_in(&text).ok_or_else(|| format!("unexpected `agy --version` output: {}", text.trim()))
}

fn manifest_platform(os: &str, arch: &str, musl: bool) -> Result<&'static str, String> {
    match (os, arch, musl) {
        ("windows", "x86_64", _) => Ok("windows_amd64"),
        ("windows", "aarch64", _) => Ok("windows_arm64"),
        ("macos", "x86_64", _) => Ok("darwin_amd64"),
        ("macos", "aarch64", _) => Ok("darwin_arm64"),
        ("linux", "x86_64", true) => Ok("linux_amd64_musl"),
        ("linux", "x86_64", false) => Ok("linux_amd64"),
        ("linux", "aarch64", true) => Ok("linux_arm64_musl"),
        ("linux", "aarch64", false) => Ok("linux_arm64"),
        _ => Err(format!("unsupported platform for Antigravity CLI updates: {os} {arch}")),
    }
}

fn parse_manifest_version(json: &[u8]) -> Result<String, String> {
    let v: serde_json::Value =
        serde_json::from_slice(json).map_err(|e| format!("bad Antigravity release manifest: {e}"))?;
    v["version"]
        .as_str()
        .map(str::trim)
        .filter(|s| crate::cli::parse(s).is_some())
        .map(str::to_string)
        .ok_or_else(|| "no valid `version` in Antigravity release manifest".into())
}

/// The newest Antigravity CLI release from its official manifest. Uses `curl`, same as `cli.rs`.
pub fn latest_version() -> Result<String, String> {
    let platform = manifest_platform(std::env::consts::OS, std::env::consts::ARCH, cfg!(target_env = "musl"))?;
    let url = format!("{MANIFEST_BASE_URL}/{platform}.json");
    let out = command("curl")
        .args(["-fsSL", "--max-time", "15", &url])
        .output()
        .map_err(|e| format!("failed to start `curl`: {e}"))?;
    if !out.status.success() {
        return Err(format!("couldn't fetch Antigravity releases: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    parse_manifest_version(&out.stdout)
}

/// The installed `agy` version and the newest release from Google's manifest. Blocks.
pub fn check() -> Result<(String, String), String> {
    Ok((installed_version()?, latest_version()?))
}

/// Runs `agy update` and returns the version installed afterwards. Blocks.
pub fn update() -> Result<String, String> {
    let out =
        command(super::program()).arg("update").output().map_err(|e| format!("couldn't start `agy update`: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "`agy update` failed: {}",
            last_line(&out.stderr).or(last_line(&out.stdout)).unwrap_or_default()
        ));
    }
    match status() {
        Status::Ready(v) => Ok(v),
        Status::Missing => Err("Antigravity CLI wasn't found after updating.".into()),
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
        assert_eq!(Status::Ready("1.5.12".into()).describe(), "Antigravity CLI 1.5.12");
        assert!(Status::Missing.describe().contains("isn't installed"));
    }

    #[test]
    fn resolves_manifest_platforms_and_parses_version() {
        assert_eq!(manifest_platform("windows", "x86_64", false), Ok("windows_amd64"));
        assert_eq!(manifest_platform("macos", "aarch64", false), Ok("darwin_arm64"));
        assert_eq!(manifest_platform("linux", "x86_64", false), Ok("linux_amd64"));
        assert_eq!(manifest_platform("linux", "aarch64", true), Ok("linux_arm64_musl"));
        assert!(manifest_platform("freebsd", "x86_64", false).is_err());

        let manifest = br#"{"version":"1.5.14","url":"https://example.invalid/agy","sha512":"abc"}"#;
        assert_eq!(parse_manifest_version(manifest), Ok("1.5.14".into()));
        assert!(parse_manifest_version(br#"{"version":"not-a-version"}"#).is_err());
        assert!(parse_manifest_version(b"not json").is_err());
    }
}
