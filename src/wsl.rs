//! Projects stored inside WSL (`\\wsl.localhost\<distro>\...`) run their
//! tools in that distro, so Claude Code gets a real Linux shell, Linux paths
//! and native filesystem speed, while Kynex itself stays a Windows app.

use std::path::{Path, PathBuf};
use std::process::Command;

pub struct WslPath {
    pub distro: String,
    /// The Linux path, e.g. `/home/me/repos/kynex`.
    pub path: String,
}

/// Recognises `\\wsl.localhost\<distro>\...` and `\\wsl$\<distro>\...`.
pub fn parse(p: &Path) -> Option<WslPath> {
    let s = p.to_str()?;
    let s = s.strip_prefix(r"\\?\UNC\").map(|r| format!(r"\\{r}")).unwrap_or_else(|| s.to_string());
    let lower = s.to_ascii_lowercase();
    let prefix = [r"\\wsl.localhost\", r"\\wsl$\"].into_iter().find(|p| lower.starts_with(p))?;
    let rest = &s[prefix.len()..];
    let (distro, path) = rest.split_once('\\').unwrap_or((rest, ""));
    if distro.is_empty() {
        return None;
    }
    Some(WslPath { distro: distro.to_string(), path: format!("/{}", path.replace('\\', "/").trim_end_matches('/')) })
}

impl WslPath {
    /// Runs `program` inside the distro, in this directory. Goes through a login
    /// shell so `PATH` picks up `~/.local/bin`, `~/.cargo/bin` and the like.
    pub fn command(&self, program: &str) -> Command {
        let mut cmd = Command::new("wsl.exe");
        cmd.args(["-d", &self.distro, "--cd", &self.path, "--exec", "bash", "-lc", r#"exec "$0" "$@""#, program]);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000);
        }
        cmd
    }

    /// The distro user's home directory as a Windows path, e.g. `\\wsl.localhost\Ubuntu\home\me`.
    pub fn home(&self) -> Option<PathBuf> {
        let out = self.command("sh").args(["-c", "printf %s \"$HOME\""]).output().ok()?;
        let home = String::from_utf8(out.stdout).ok()?;
        home.starts_with('/').then(|| PathBuf::from(format!(r"\\wsl.localhost\{}{}", self.distro, home.replace('/', r"\"))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_unc_paths() {
        let w = parse(Path::new(r"\\wsl.localhost\Ubuntu\home\me\repos\kynex")).unwrap();
        assert_eq!((w.distro.as_str(), w.path.as_str()), ("Ubuntu", "/home/me/repos/kynex"));
        let w = parse(Path::new(r"\\wsl$\Ubuntu\home\me\")).unwrap();
        assert_eq!((w.distro.as_str(), w.path.as_str()), ("Ubuntu", "/home/me"));
        let w = parse(Path::new(r"\\?\UNC\wsl.localhost\Ubuntu\srv")).unwrap();
        assert_eq!((w.distro.as_str(), w.path.as_str()), ("Ubuntu", "/srv"));
        assert!(parse(Path::new(r"C:\Users\me")).is_none());
    }

    /// Needs WSL with an `Ubuntu` distro. Run with `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn passes_args_through_intact() {
        let w = parse(Path::new(r"\\wsl.localhost\Ubuntu\tmp")).unwrap();
        let out = w.command("printf").args(["[%s]", "a", "", "b c", "$HOME", "\"q\""]).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), "[a][][b c][$HOME][\"q\"]");
        let out = w.command("pwd").output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "/tmp");
        assert!(w.home().is_some());
    }
}
