//! Changing things: staging, discarding, committing, and syncing with the remote.

use std::path::Path;
use std::sync::atomic::AtomicBool;

use super::status::{Change, FileStatus};
use super::{git, git_stdin, git_until};

pub fn stage(root: &Path, paths: &[String]) -> Result<(), String> {
    let mut args = vec!["add", "-A", "--"];
    args.extend(paths.iter().map(String::as_str));
    git(root, &args).map(drop)
}

pub fn stage_all(root: &Path) -> Result<(), String> {
    git(root, &["add", "-A"]).map(drop)
}

pub fn unstage(root: &Path, paths: &[String]) -> Result<(), String> {
    let mut args = vec!["restore", "--staged", "--"];
    args.extend(paths.iter().map(String::as_str));
    match git(root, &args) {
        // No HEAD to restore from yet (nothing committed): just take them out of the index.
        Err(e) if no_head(&e) => {
            let mut args = vec!["rm", "--cached", "-r", "-q", "--"];
            args.extend(paths.iter().map(String::as_str));
            git(root, &args).map(drop)
        }
        r => r.map(drop),
    }
}

pub fn unstage_all(root: &Path) -> Result<(), String> {
    match git(root, &["reset", "-q"]) {
        Err(e) if no_head(&e) => git(root, &["rm", "--cached", "-r", "-q", "."]).map(drop),
        r => r.map(drop),
    }
}

fn no_head(err: &str) -> bool {
    err.contains("could not resolve HEAD") || err.contains("ambiguous argument 'HEAD'") || err.contains("bad revision")
}

/// Throws away a file's unstaged changes; an untracked file is deleted.
/// Staged changes stay.
pub fn discard(root: &Path, file: &FileStatus) -> Result<(), String> {
    match file.unstaged {
        Some(Change::Untracked) => {
            std::fs::remove_file(root.join(&file.path)).map_err(|e| format!("Couldn't delete {}: {e}", file.path))
        }
        Some(_) => git(root, &["restore", "--worktree", "--", &file.path]).map(drop),
        None => Ok(()),
    }
}

/// Commits what's staged with `message`; `amend` rewrites the last commit
/// instead. Returns git's one-line summary.
pub fn commit(root: &Path, message: &str, amend: bool) -> Result<String, String> {
    if message.trim().is_empty() {
        return Err("Write a commit message first".into());
    }
    let mut args = vec!["commit", "--cleanup=strip", "-F", "-"];
    if amend {
        args.push("--amend");
    }
    let out = git_stdin(root, &args, message)?;
    Ok(out.lines().next().unwrap_or("").trim().to_string())
}

/// The last commit's message, to edit when amending.
pub fn last_message(root: &Path) -> Result<String, String> {
    git(root, &["log", "-1", "--format=%B"]).map(|s| s.trim_end().to_string())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sync {
    Fetch,
    Pull,
    Push,
}

impl Sync {
    pub fn label(self) -> &'static str {
        match self {
            Sync::Fetch => "Fetching",
            Sync::Pull => "Pulling",
            Sync::Push => "Pushing",
        }
    }
}

/// Talks to the remote. Pushing a branch with no upstream publishes it to
/// `origin` (or the only remote) and sets the upstream. Can take a while:
/// setting `cancel` stops it, with [`super::CANCELLED`] as the error. Blocks.
pub fn sync(root: &Path, op: Sync, cancel: &AtomicBool) -> Result<(), String> {
    match op {
        Sync::Fetch => git_until(root, &["fetch", "--all", "--prune"], cancel).map(drop),
        Sync::Pull => git_until(root, &["pull"], cancel).map(drop),
        Sync::Push => {
            let has_upstream = git(root, &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"]).is_ok();
            if has_upstream {
                return git_until(root, &["push"], cancel).map(drop);
            }
            let remotes = git(root, &["remote"])?;
            let remotes: Vec<&str> = remotes.lines().collect();
            let remote = match remotes.as_slice() {
                [] => return Err("This repository has no remote to push to".into()),
                [only] => *only,
                many => many.iter().copied().find(|r| *r == "origin").unwrap_or(many[0]),
            };
            git_until(root, &["push", "--set-upstream", remote, "HEAD"], cancel).map(drop)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::super::status::status;
    use super::*;

    /// A throwaway repo in the temp dir, removed on drop.
    struct Repo(PathBuf);

    impl Repo {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("kynex-git-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let repo = Repo(dir);
            repo.git(&["init", "-q", "-b", "main"]);
            repo.git(&["config", "user.name", "Test"]);
            repo.git(&["config", "user.email", "test@example.com"]);
            repo.git(&["config", "commit.gpgsign", "false"]);
            repo
        }

        fn git(&self, args: &[&str]) -> String {
            git(&self.0, args).unwrap()
        }

        fn write(&self, file: &str, text: &str) {
            std::fs::write(self.0.join(file), text).unwrap();
        }
    }

    impl Drop for Repo {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn stage_commit_discard_round_trip() {
        let repo = Repo::new("ops");
        let root = &repo.0;
        repo.write("a.txt", "one\n");

        // Before the first commit: staging and unstaging still work.
        stage_all(root).unwrap();
        assert_eq!(status(root).unwrap().staged().count(), 1);
        unstage_all(root).unwrap();
        assert_eq!(status(root).unwrap().staged().count(), 0);

        stage(root, &["a.txt".into()]).unwrap();
        assert!(commit(root, "  ", false).is_err());
        commit(root, "First\n\nBody", false).unwrap();
        assert_eq!(last_message(root).unwrap(), "First\n\nBody");
        assert!(status(root).unwrap().files.is_empty());

        repo.write("a.txt", "two\n");
        repo.write("new.txt", "new\n");
        let st = status(root).unwrap();
        assert_eq!(st.unstaged().count(), 2);
        for f in st.unstaged() {
            discard(root, f).unwrap();
        }
        assert!(status(root).unwrap().files.is_empty());
        assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap().trim(), "one");

        repo.write("a.txt", "three\n");
        stage(root, &["a.txt".into()]).unwrap();
        unstage(root, &["a.txt".into()]).unwrap();
        assert_eq!(status(root).unwrap().staged().count(), 0);
        stage_all(root).unwrap();
        commit(root, "Amended", true).unwrap();
        assert_eq!(super::super::log(root, 10).unwrap().len(), 1);
    }

    #[test]
    fn the_tree_stages_and_unstages_every_kind_of_change() {
        use super::super::tree::{Staged, Target, TreeFile, stage_paths, tree_files};
        let repo = Repo::new("tree-stage");
        let root = &repo.0;
        // Contents of their own, or git would pair the deletion with the rename.
        for f in ["gone.txt", "old.txt", "edit.txt"] {
            repo.write(f, &format!("{f}\none\ntwo\n"));
        }
        stage_all(root).unwrap();
        commit(root, "base", false).unwrap();
        std::fs::remove_file(root.join("gone.txt")).unwrap();
        repo.git(&["mv", "old.txt", "renamed.txt"]);
        std::fs::create_dir_all(root.join("fresh/deeper")).unwrap();
        repo.write("fresh/deeper/n.txt", "n\n");
        repo.write("untracked.txt", "u\n");
        repo.write("edit.txt", "2\n");
        stage(root, &["edit.txt".into()]).unwrap();
        repo.write("edit.txt", "3\n");

        let files = || tree_files(&status(root).unwrap(), &std::collections::HashMap::new());
        let state = |p: &str| files().into_iter().find(|f| f.path == p).map(|f| f.staged());
        let at = |f: &[TreeFile], p: &str| Target::File(f.iter().position(|x| x.path == p).unwrap());
        assert_eq!(state("edit.txt"), Some(Staged::Partly));
        assert_eq!(state("renamed.txt"), Some(Staged::All));

        // The rest of a partly staged file, a deletion, an untracked file, a new folder.
        for p in ["edit.txt", "gone.txt", "untracked.txt"] {
            let f = files();
            stage(root, &stage_paths(&f, at(&f, p), false)).unwrap();
            assert_eq!(state(p), Some(Staged::All), "{p}");
        }
        let f = files();
        stage(root, &stage_paths(&f, Target::Folder("fresh/deeper"), false)).unwrap();
        assert_eq!(state("fresh/deeper/n.txt"), Some(Staged::All));

        // And all of them back.
        for p in ["edit.txt", "gone.txt", "untracked.txt", "renamed.txt"] {
            let f = files();
            unstage(root, &stage_paths(&f, at(&f, p), true)).unwrap();
            assert_eq!(state(p), Some(Staged::No), "{p}");
        }
        assert_eq!(state("old.txt"), Some(Staged::No), "the rename's old path isn't left staged as deleted");
        let f = files();
        unstage(root, &stage_paths(&f, Target::Folder("fresh/deeper"), true)).unwrap();
        assert_eq!(state("fresh/deeper/n.txt"), Some(Staged::No));
        assert_eq!(status(root).unwrap().staged().count(), 0);
    }

    #[test]
    fn a_partly_staged_file_shows_each_side_whole() {
        use super::super::diff::{DiffSide, LineKind, whole_file_diff_side};
        let repo = Repo::new("sides");
        let root = &repo.0;
        repo.write("f.txt", "one\ntwo\nthree\n");
        stage_all(root).unwrap();
        commit(root, "base", false).unwrap();
        repo.write("f.txt", "ONE\ntwo\nthree\n");
        stage_all(root).unwrap();
        repo.write("f.txt", "ONE\ntwo\nTHREE\n");
        let added = |side| -> Vec<String> {
            let d = whole_file_diff_side(root, "f.txt", false, side).unwrap();
            let lines = d.hunks.iter().flat_map(|h| &h.lines);
            lines.filter(|l| l.kind == LineKind::Added).map(|l| l.text.clone()).collect()
        };
        assert_eq!(added(DiffSide::All), ["ONE", "THREE"]);
        assert_eq!(added(DiffSide::Staged), ["ONE"]);
        assert_eq!(added(DiffSide::Unstaged), ["THREE"]);
        // Whole: every line of the file is there, not only the hunks.
        let d = whole_file_diff_side(root, "f.txt", false, DiffSide::Unstaged).unwrap();
        assert_eq!(
            d.hunks.iter().map(|h| h.lines.iter().filter(|l| l.kind != LineKind::Removed).count()).sum::<usize>(),
            3
        );
    }

    #[test]
    fn push_without_a_remote_says_so() {
        let repo = Repo::new("push");
        repo.write("a.txt", "x");
        stage_all(&repo.0).unwrap();
        commit(&repo.0, "x", false).unwrap();
        let go = AtomicBool::new(false);
        assert_eq!(sync(&repo.0, Sync::Push, &go).unwrap_err(), "This repository has no remote to push to");
    }

    #[test]
    fn a_sync_stops_when_cancelled() {
        let repo = Repo::new("cancel");
        assert_eq!(sync(&repo.0, Sync::Fetch, &AtomicBool::new(false)), Ok(()), "nothing to fetch from");
        assert_eq!(sync(&repo.0, Sync::Fetch, &AtomicBool::new(true)).unwrap_err(), super::super::CANCELLED);
    }
}
