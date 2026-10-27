//! Git for one repository folder: status, diffs, history, branches, worktrees,
//! and the operations behind the Git view (stage, commit, switch, fetch, pull,
//! push). Everything here runs `git` and blocks; call it off the UI thread.
//!
//! Output is parsed from git's machine formats (`--porcelain=v2 -z`, custom
//! `--format`s with separator bytes), never from human-readable text.

pub(crate) mod branch;
pub(crate) mod diff;
pub(crate) mod log;
mod ops;
pub(crate) mod status;
pub(crate) mod worktree;

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

pub use branch::{Branch, branches, create_branch, switch_branch};
pub use diff::{DiffLine, FileDiff, Hunk, LineKind, commit_diff, diff_stat, file_diff};
pub use log::{Commit, CommitDetail, Ref, RefKind, commit_detail, log};
pub use ops::{Sync, commit, discard, last_message, stage, stage_all, sync, unstage, unstage_all};
pub use status::{Change, FileStatus, Head, Status, status};
pub use worktree::{Worktree, list_worktrees, worktree_in};

use crate::wsl;

/// `git` for `root`, set up to never prompt on a terminal (there is none) and
/// to print paths unquoted.
fn command(root: &Path) -> Command {
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
    // Reads shouldn't take the index lock a running Claude session may want.
    cmd.args(["--no-optional-locks", "-c", "core.quotepath=false", "-c", "color.ui=false"]);
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    cmd
}

/// Runs git and returns its stdout, or its stderr as the error.
fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    let out = command(root).args(args).output().map_err(|e| format!("Couldn't run git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(failure(&out.stderr, &out.stdout))
    }
}

/// Runs git with `input` on stdin.
fn git_stdin(root: &Path, args: &[&str], input: &str) -> Result<String, String> {
    let mut child = command(root)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Couldn't run git: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(input.as_bytes()).map_err(|e| format!("Couldn't talk to git: {e}"))?;
    }
    let out = child.wait_with_output().map_err(|e| format!("git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(failure(&out.stderr, &out.stdout))
    }
}

/// Git's own words for a failure: stderr, else stdout (`commit` reports
/// "nothing to commit" there), trimmed of hint lines.
fn failure(stderr: &[u8], stdout: &[u8]) -> String {
    let text = String::from_utf8_lossy(if stderr.iter().all(u8::is_ascii_whitespace) { stdout } else { stderr });
    let lines: Vec<&str> = text.lines().filter(|l| !l.starts_with("hint:") && !l.trim().is_empty()).collect();
    if lines.is_empty() { "git failed".into() } else { lines.join("\n") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failures_drop_hints() {
        let err = failure(b"error: failed to push some refs\nhint: Updates were rejected\n", b"");
        assert_eq!(err, "error: failed to push some refs");
        assert_eq!(failure(b"", b"nothing to commit, working tree clean\n"), "nothing to commit, working tree clean");
    }
}
