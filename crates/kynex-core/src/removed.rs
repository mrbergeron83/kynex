//! Workspaces removed through Kynex. Sessions are stored by folder and a worktree is short
//! lived, so removing one keeps its conversations in sight: the project remembers the
//! folder (with its name and branch) and the sidebar lists its sessions under "Removed
//! workspaces". They open to read; "Continue in" copies a log into a live workspace's
//! transcripts folder ([`continue_in`]); "Forget" deletes them all ([`delete_sessions`]).
//!
//! `claude --resume` finds a log by the folder it runs in, and the removed folder is gone:
//! Kynex still reads the log (the lookup falls back to every project folder) but can't start
//! a process there.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::groups::SessionGroups;
use crate::project::normalize;
use crate::session;
use crate::workspace::{Workspace, Workspaces, same_folder};

/// A workspace that was removed, remembered for its sessions.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Removed {
    pub path: PathBuf,
    /// What the sidebar called it: the folder's name.
    pub label: String,
    #[serde(default)]
    pub branch: Option<String>,
}

impl Workspaces {
    /// Remembers that the workspace at `path` was removed. One remembered already is replaced
    /// (and moves last). Returns whether anything changed.
    pub fn remember_removed(&mut self, path: &Path, label: &str, branch: Option<&str>) -> bool {
        let new = Removed { path: path.to_path_buf(), label: label.to_string(), branch: branch.map(str::to_string) };
        if self.removed.last() == Some(&new) {
            return false;
        }
        self.removed.retain(|r| !same_folder(&r.path, path));
        self.removed.push(new);
        true
    }

    /// Drops the removed workspace at `path` from the list. Returns whether it was there.
    pub fn forget_removed(&mut self, path: &Path) -> bool {
        let before = self.removed.len();
        self.removed.retain(|r| !same_folder(&r.path, path));
        self.removed.len() != before
    }

    /// The removed workspaces to show, newest removal first, given the workspaces there are
    /// now: a folder that is a workspace again (a worktree made anew, a folder added back)
    /// is that, not a removed one, so nothing is listed twice.
    pub fn removed_to_show(&self, current: &[Workspace]) -> Vec<&Removed> {
        let mut out: Vec<&Removed> =
            self.removed.iter().rev().filter(|r| !current.iter().any(|c| c.at(&r.path))).collect();
        // The same folder can't be listed twice (the file may have been edited by hand).
        let mut seen = Vec::new();
        out.retain(|r| {
            let key = normalize(&r.path);
            let new = !seen.contains(&key);
            seen.push(key);
            new
        });
        out
    }

    /// The removed workspace at `path`.
    pub fn removed_at(&self, path: &Path) -> Option<&Removed> {
        self.removed.iter().find(|r| same_folder(&r.path, path))
    }
}

/// What [`delete_sessions`] did.
#[derive(Debug, Default, PartialEq)]
pub struct Purged {
    /// The sessions that are gone.
    pub deleted: Vec<String>,
    /// Why the others couldn't be.
    pub errors: Vec<String>,
}

/// Deletes every session of the folder `cwd`: their transcripts, and their places in its
/// groups and archive. Blocking (reads every transcript's ends, then deletes); call it off
/// the UI thread.
pub fn delete_sessions(cwd: &Path) -> Purged {
    let mut out = Purged::default();
    for s in session::list_sessions(cwd) {
        match session::delete_session(cwd, &s.id) {
            Ok(()) => out.deleted.push(s.id),
            Err(e) => out.errors.push(e),
        }
    }
    if !out.deleted.is_empty() {
        let mut groups = SessionGroups::load(cwd);
        for id in &out.deleted {
            groups.forget(id);
        }
        if let Err(e) = groups.save(cwd) {
            out.errors.push(e);
        }
    }
    out
}

/// Makes session `id` of the folder `from` continuable in `to`: its log (and the folder with
/// the same name beside it, if Claude Code kept one: subagent logs) is copied into `to`'s
/// transcripts folder, so `claude --resume <id>` run in `to` finds it. The original stays.
/// A log `to` already has is left alone. Blocking: file copies. A Grok session can't be moved.
pub fn continue_in(from: &Path, id: &str, to: &Path) -> Result<(), String> {
    let src = session::find_transcript(from, id)
        .ok_or_else(|| "Only Claude Code sessions can be continued in another workspace".to_string())?;
    let dir = session::transcripts_dir(to)
        .or_else(|| Some(session::projects_dir()?.join(session::transcripts_name(to))))
        .ok_or_else(|| "Couldn't find the transcripts folder".to_string())?;
    copy_session(&src, &dir)
}

/// Copies the log `src` (`<id>.jsonl`) and its sibling folder `<id>/` into `dir`.
fn copy_session(src: &Path, dir: &Path) -> Result<(), String> {
    let name = src.file_name().ok_or("bad transcript path")?;
    let dest = dir.join(name);
    if dest.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("Couldn't create {}: {e}", dir.display()))?;
    // The folder first: the log appearing is what makes the session exist.
    let side = src.with_extension("");
    if side.is_dir() {
        copy_dir(&side, &dir.join(side.file_name().ok_or("bad transcript path")?))?;
    }
    std::fs::copy(src, &dest).map_err(|e| format!("Couldn't copy {}: {e}", src.display()))?;
    Ok(())
}

fn copy_dir(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|e| format!("Couldn't create {}: {e}", to.display()))?;
    let entries = std::fs::read_dir(from).map_err(|e| format!("Couldn't read {}: {e}", from.display()))?;
    for e in entries.filter_map(Result::ok) {
        let (src, dest) = (e.path(), to.join(e.file_name()));
        if src.is_dir() {
            copy_dir(&src, &dest)?;
        } else {
            std::fs::copy(&src, &dest).map_err(|e| format!("Couldn't copy {}: {e}", src.display()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ws(path: &str) -> Workspace {
        Workspace { path: path.into(), branch: None, main: false, label: "x".into(), folder: false }
    }

    #[test]
    fn remembers_and_forgets() {
        let mut w = Workspaces::default();
        let p = Path::new("/data/wt/feat-x");
        assert!(w.remember_removed(p, "feat-x", Some("feat/x")));
        assert!(!w.remember_removed(p, "feat-x", Some("feat/x")));
        assert_eq!(w.removed_at(p).unwrap().branch.as_deref(), Some("feat/x"));
        // Removed again with another branch: replaced, not doubled.
        assert!(w.remember_removed(p, "feat-x", None));
        assert_eq!(w.removed.len(), 1);
        assert!(w.forget_removed(p));
        assert!(!w.forget_removed(p));
        assert_eq!(w, Workspaces::default());
    }

    #[test]
    fn lists_newest_first_without_doubles() {
        let mut w = Workspaces::default();
        w.remember_removed(Path::new("/wt/a"), "a", None);
        w.remember_removed(Path::new("/wt/b"), "b", None);
        w.remember_removed(Path::new("/wt/c"), "c", None);
        // A workspace again: listed as a workspace only.
        let current = [ws("/wt/b")];
        let labels: Vec<_> = w.removed_to_show(&current).iter().map(|r| r.label.as_str()).collect();
        assert_eq!(labels, ["c", "a"]);
        // A hand-edited file with the same folder twice.
        w.removed.push(Removed { path: "/wt/a".into(), label: "a2".into(), branch: None });
        assert_eq!(w.removed_to_show(&[]).len(), 3);
    }

    #[test]
    fn slashes_do_not_make_a_second_row() {
        let mut w = Workspaces::default();
        w.remember_removed(Path::new("/wt/a"), "a", None);
        w.remember_removed(Path::new("/wt/a/"), "a", None);
        assert_eq!(w.removed.len(), 1);
        assert!(w.removed_to_show(&[ws("/wt/a/")]).is_empty());
    }

    #[test]
    fn an_old_file_loads_unchanged_and_keeps_what_it_has() {
        let old =
            r#"{"selected":"/work/app","sessions":{"/work/app":"s1"},"order":["/work/app"],"folders":["/notes"]}"#;
        let w: Workspaces = serde_json::from_str(old).unwrap();
        assert!(w.removed.is_empty());
        assert_eq!(w.sessions.get("/work/app").map(String::as_str), Some("s1"));
        // A saved file without a branch reads too, and a round trip loses nothing.
        let new = r#"{"removed":[{"path":"/wt/a","label":"a"}]}"#;
        let mut w: Workspaces = serde_json::from_str(new).unwrap();
        assert_eq!(w.removed[0].branch, None);
        w.remember_removed(Path::new("/wt/b"), "b", Some("b"));
        let back: Workspaces = serde_json::from_str(&serde_json::to_string(&w).unwrap()).unwrap();
        assert_eq!(back, w);
    }

    #[test]
    fn copies_a_log_and_its_folder() {
        let base = std::env::temp_dir().join(format!("kynex-continue-{}", std::process::id()));
        let (from, to) = (base.join("from"), base.join("to"));
        std::fs::create_dir_all(from.join("abc").join("subagents")).unwrap();
        std::fs::write(from.join("abc.jsonl"), "log").unwrap();
        std::fs::write(from.join("abc").join("subagents").join("a.jsonl"), "sub").unwrap();
        std::fs::write(from.join("other.jsonl"), "other").unwrap();
        copy_session(&from.join("abc.jsonl"), &to).unwrap();
        assert_eq!(std::fs::read_to_string(to.join("abc.jsonl")).unwrap(), "log");
        assert_eq!(std::fs::read_to_string(to.join("abc").join("subagents").join("a.jsonl")).unwrap(), "sub");
        assert!(!to.join("other.jsonl").exists());
        assert!(from.join("abc.jsonl").exists());
        // A second time, and with a log already there, leaves it alone.
        std::fs::write(to.join("abc.jsonl"), "newer").unwrap();
        copy_session(&from.join("abc.jsonl"), &to).unwrap();
        assert_eq!(std::fs::read_to_string(to.join("abc.jsonl")).unwrap(), "newer");
        std::fs::remove_dir_all(&base).ok();
    }
}
