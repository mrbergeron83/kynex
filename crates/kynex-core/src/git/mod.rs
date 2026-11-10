//! Git for one repository folder: status, diffs, history, branches, worktrees,
//! and the operations behind the Git view (stage, commit, switch, fetch, pull,
//! push). Everything here runs `git` and blocks; call it off the UI thread.
//!
//! Output is parsed from git's machine formats (`--porcelain=v2 -z`, custom
//! `--format`s with separator bytes), never from human-readable text.

pub(crate) mod branch;
pub mod cleanup;
pub(crate) mod diff;
pub mod github;
pub(crate) mod log;
mod ops;
pub(crate) mod status;
pub(crate) mod worktree;

use std::ffi::{OsStr, OsString};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

pub use branch::{
    Branch, branches, create_branch, create_branch_at, default_branch, delete_branch, delete_remote_branch,
    publish_branch, rename_branch, switch_branch,
};
pub use diff::{DiffLine, FileDiff, Hunk, LineKind, commit_diff, compare_diff, diff_stat, file_diff};
pub use log::{Commit, CommitDetail, Ref, RefKind, commit_detail, log};
pub use ops::{Sync, commit, discard, last_message, stage, stage_all, sync, unstage, unstage_all};
pub use status::{Change, FileStatus, Head, Status, status};
pub use worktree::{
    Worktree, add_worktree, add_worktree_from, default_worktree_path, fill_worktrees, list_worktrees,
    list_worktrees_quick, prune_worktrees, remove_worktree, worktree_in,
};

/// `git` for `root`, set up to never prompt on a terminal (there is none) and
/// to print paths unquoted.
fn command(root: &Path) -> Command {
    command_with("git".as_ref(), root)
}

/// [`command`] running `program`.
fn command_with(program: &OsStr, root: &Path) -> Command {
    let mut cmd = Command::new(program);
    cmd.arg("-C").arg(root);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    // Reads shouldn't take the index lock a running Claude session may want.
    cmd.args(["--no-optional-locks", "-c", "core.quotepath=false", "-c", "color.ui=false"]);
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    cmd
}

/// Whether `args` are one of the local, read-only calls that are safe to run on the
/// real `git.exe` directly: they start no hooks, credential helpers or `ssh`, which
/// is what the `cmd\git.exe` wrapper's PATH set-up is for.
fn is_local_read(args: &[&str]) -> bool {
    match args {
        ["worktree", "list", ..] => true,
        [cmd, ..] => matches!(*cmd, "status" | "rev-list" | "rev-parse" | "for-each-ref" | "diff" | "log" | "ls-files"),
        [] => false,
    }
}

/// The program for [`is_local_read`] calls. On Windows, when the `git` on PATH is Git
/// for Windows' `cmd\git.exe` wrapper (which starts `mingw64\bin\git.exe` as a second
/// process: twice the process starts, and each can stall for half a second), that
/// `git.exe` itself; plain `git` everywhere else or if it isn't found.
fn read_program() -> &'static OsStr {
    static PROGRAM: OnceLock<OsString> = OnceLock::new();
    PROGRAM.get_or_init(|| {
        let path = std::env::var_os("PATH").unwrap_or_default();
        direct_git_in(&path).map_or_else(|| "git".into(), PathBuf::into_os_string)
    })
}

/// The `mingw64\bin\git.exe` behind the first `cmd\git.exe` on `path`.
fn direct_git_in(path: &OsStr) -> Option<PathBuf> {
    if !cfg!(windows) {
        return None;
    }
    // The first git on PATH is the one that runs.
    let dir = std::env::split_paths(path).find(|d| d.join("git.exe").is_file())?;
    if !dir.file_name()?.eq_ignore_ascii_case("cmd") {
        return None;
    }
    let real = dir.parent()?.join("mingw64").join("bin").join("git.exe");
    real.is_file().then_some(real)
}

/// Runs git and returns its stdout, or its stderr as the error.
fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    let program = if is_local_read(args) { read_program() } else { OsStr::new("git") };
    let out = command_with(program, root).args(args).output().map_err(|e| format!("Couldn't run git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(failure(&out.stderr, &out.stdout))
    }
}

/// [`git`] that stops when `cancel` is set, with "Cancelled" as the error.
/// Its output is read on threads meanwhile, so a chatty git never stalls on a full pipe.
fn git_until(root: &Path, args: &[&str], cancel: &AtomicBool) -> Result<String, String> {
    use std::io::Read;
    let mut child = command(root)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Couldn't run git: {e}"))?;
    let drain = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut p) = pipe {
                let _ = p.read_to_end(&mut buf);
            }
            buf
        })
    };
    let out = drain(child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let err = drain(child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let status = loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(CANCELLED.into());
        }
        match child.try_wait().map_err(|e| format!("git: {e}"))? {
            Some(status) => break status,
            None => std::thread::sleep(std::time::Duration::from_millis(50)),
        }
    };
    let (out, err) = (out.join().unwrap_or_default(), err.join().unwrap_or_default());
    if status.success() { Ok(String::from_utf8_lossy(&out).into_owned()) } else { Err(failure(&err, &out)) }
}

/// The error of an operation stopped with its cancel flag.
pub const CANCELLED: &str = "Cancelled";

/// The repository's files under `root`, relative to it with `/` separators:
/// tracked ones plus untracked ones `.gitignore` doesn't exclude. Blocks.
pub fn files(root: &Path) -> Result<Vec<String>, String> {
    let out = git(root, &["ls-files", "-z", "--cached", "--others", "--exclude-standard", "--deduplicate"])?;
    Ok(out.split('\0').filter(|p| !p.is_empty()).map(str::to_string).collect())
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

    #[test]
    fn only_local_reads_skip_the_wrapper() {
        for args in [&["status", "--porcelain=v2"][..], &["worktree", "list", "--porcelain"], &["rev-list", "a...b"]] {
            assert!(is_local_read(args), "{args:?}");
        }
        // Anything that may run a hook, a credential helper or ssh keeps the wrapper.
        for args in [
            &["push"][..],
            &["pull", "--ff-only"],
            &["fetch", "--all"],
            &["commit", "-m", "x"],
            &["worktree", "add", "p"],
            &["worktree", "remove", "p"],
            &["switch", "main"],
            &[],
        ] {
            assert!(!is_local_read(args), "{args:?}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn finds_the_real_git_behind_the_cmd_wrapper() {
        let root = std::env::temp_dir().join(format!("kynex-git-path-{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        let (cmd, real) = (root.join("Git").join("cmd"), root.join("Git").join("mingw64").join("bin"));
        std::fs::create_dir_all(&cmd).unwrap();
        std::fs::create_dir_all(&real).unwrap();
        std::fs::write(cmd.join("git.exe"), "").unwrap();
        let path = std::env::join_paths([root.join("nothing-here"), cmd.clone()]).unwrap();
        // No real git.exe yet: stay on plain `git`.
        assert_eq!(direct_git_in(&path), None);
        std::fs::write(real.join("git.exe"), "").unwrap();
        assert_eq!(direct_git_in(&path), Some(real.join("git.exe")));
        // A git that isn't in a `cmd` folder (a package manager's shim) is left alone.
        let shim = root.join("shims");
        std::fs::create_dir_all(&shim).unwrap();
        std::fs::write(shim.join("git.exe"), "").unwrap();
        let path = std::env::join_paths([shim, cmd]).unwrap();
        assert_eq!(direct_git_in(&path), None);
        std::fs::remove_dir_all(&root).ok();
    }
}
