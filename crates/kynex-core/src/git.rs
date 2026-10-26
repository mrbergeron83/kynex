//! Reading a project's git state: branch, changed files, history and diffs.
//! Everything here runs `git` and blocks; call it off the UI thread.

use std::path::Path;
use std::process::Command;

use crate::wsl;

#[derive(Default, Debug, PartialEq)]
pub struct Status {
    /// The porcelain branch line, e.g. `main...origin/main [ahead 1]`.
    pub branch: String,
    /// `(XY status, path)`; renames read `old -> new`.
    pub files: Vec<(String, String)>,
    /// `git log --oneline` lines, newest first.
    pub log: Vec<String>,
    pub error: Option<String>,
}

fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    let mut cmd = match wsl::parse(root) {
        // Linux git in the distro: fast, and agrees with Claude about line endings and file modes.
        Some(w) => w.command("git"),
        None => {
            let mut c = Command::new("git");
            c.arg("-C").arg(root);
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                c.creation_flags(0x0800_0000);
            }
            c
        }
    };
    cmd.args(args);
    let out = cmd.output().map_err(|e| format!("git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

pub fn read_status(root: &Path) -> Status {
    match git(root, &["status", "--porcelain=v1", "-b", "-uall"]) {
        Ok(s) => {
            let log = git(root, &["log", "--oneline", "--decorate", "-n", "50"])
                .map(|s| s.lines().map(str::to_string).collect())
                .unwrap_or_default();
            Status { log, ..parse_status(&s) }
        }
        Err(e) => Status { error: Some(e), ..Default::default() },
    }
}

/// Parses `git status --porcelain=v1 -b`.
fn parse_status(s: &str) -> Status {
    let mut lines = s.lines();
    let branch = lines.next().unwrap_or("").trim_start_matches("## ").to_string();
    let files = lines.filter(|l| l.len() > 3).map(|l| (l[..2].to_string(), l[3..].to_string())).collect();
    Status { branch, files, ..Default::default() }
}

/// The diff of one changed file against HEAD, as lines. Untracked files read as
/// all added.
pub fn read_diff(root: &Path, file: &str) -> Vec<String> {
    let path = file.rsplit(" -> ").next().unwrap_or(file);
    let diff = git(root, &["diff", "HEAD", "--", path]).unwrap_or_default();
    if !diff.is_empty() {
        return diff.lines().map(str::to_string).collect();
    }
    match std::fs::read_to_string(root.join(path)) {
        Ok(c) => c.lines().map(|l| format!("+{l}")).collect(),
        Err(_) => vec!["(binary or unreadable file)".into()],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_porcelain() {
        let s = parse_status("## main...origin/main\n M src/app.rs\n?? new.rs\nR  a.rs -> b.rs\n");
        assert_eq!(s.branch, "main...origin/main");
        assert_eq!(
            s.files,
            [(" M", "src/app.rs"), ("??", "new.rs"), ("R ", "a.rs -> b.rs")].map(|(a, b)| (a.to_string(), b.to_string()))
        );
    }
}
