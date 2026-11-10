//! Reading git state in-process with `gix`: no `git` process to start, which on
//! Windows costs 40-60 ms a call and now and then stalls for half a second.
//!
//! Only reads live here, and only the ones Kynex repeats (the sidebar's changed-file
//! counts and how far a branch is from its upstream or the default branch). Writes
//! (commit, push, worktree add / remove) stay on the `git` CLI, so hooks and the
//! credential manager behave as they always did. Every function returns an error when
//! `gix` can't answer, and callers fall back to the CLI's answer.

use std::collections::HashSet;
use std::path::Path;

use gix::bstr::{BString, ByteSlice};

use super::Worktree;

/// What the sidebar shows of `git status`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    /// Files with a change (staged, unstaged, conflicted or untracked), each counted once.
    pub files: usize,
    /// The untracked ones, relative to the worktree: their lines count as added.
    pub untracked: Vec<String>,
    /// Commits `(ahead, behind)` the branch's upstream; `None` without one.
    pub upstream: Option<(usize, usize)>,
}

fn err(e: impl std::fmt::Display) -> String {
    format!("gix: {e}")
}

/// The repository at or above `path`, when `gix` can be trusted to read it the way the
/// CLI would: a folder owned by someone else is opened with its own config ignored
/// (the CLI refuses it outright), so that's left to the CLI.
fn open(path: &Path) -> Result<gix::Repository, String> {
    let repo = gix::discover(path).map_err(err)?;
    if repo.git_dir_trust() != gix::sec::Trust::Full {
        return Err(err("the repository isn't fully trusted"));
    }
    Ok(repo)
}

/// The working trees of the repository `root` belongs to, main first, as `git worktree
/// list --porcelain` names them (no changes or distances yet). `Ok(None)`: `root` isn't
/// in a repository, and the CLI wouldn't say otherwise. `Err`: `gix` can't tell (a bare
/// repository, a worktree whose folder is gone…): ask the CLI.
pub fn worktrees(root: &Path) -> Result<Option<Vec<Worktree>>, String> {
    // A `.git` (a folder, or a file in a linked worktree) in `root` or a parent is what
    // makes a repository; without one there's nothing to ask.
    if !root.ancestors().any(|dir| dir.join(".git").exists()) {
        return Ok(None);
    }
    let repo = open(root)?;
    let main = repo.main_repo().map_err(err)?;
    let main_path = main.workdir().ok_or_else(|| err("a bare repository"))?.to_path_buf();
    let mut out = vec![worktree(&main, main_path, true, false, false)];
    for proxy in main.worktrees().map_err(err)? {
        let path = proxy.base().map_err(err)?;
        let (locked, prunable) = (proxy.is_locked(), proxy.is_prunable());
        let linked = proxy.into_repo_with_possibly_inaccessible_worktree().map_err(err)?;
        out.push(worktree(&linked, path, false, locked, prunable));
    }
    Ok(Some(out))
}

fn worktree(repo: &gix::Repository, path: std::path::PathBuf, main: bool, locked: bool, prunable: bool) -> Worktree {
    // As the CLI's output is read: the short commit is seven characters, an unborn branch has all zeros.
    let head = repo.head_id().map_or_else(|_| "0000000".to_string(), |id| id.to_string().chars().take(7).collect());
    let branch = repo.head_name().ok().flatten().map(|name| name.shorten().to_string());
    // Git prints `/` everywhere; the rest of Kynex has paths the way the OS writes them.
    let path = if cfg!(windows) { std::path::PathBuf::from(path.to_string_lossy().replace('/', "\\")) } else { path };
    Worktree { path, branch, head, main, locked, prunable, ..Default::default() }
}

/// The changed files of the working tree at `path` and its distance from its upstream,
/// as `git status --porcelain=v2 --branch --untracked-files=all` would count them.
/// (Renames count once, as git's do; worktree-side renames aren't detected, as git's aren't.)
pub fn summary(path: &Path) -> Result<Summary, String> {
    let repo = open(path)?;
    // What `gix` doesn't do the way git does yet (its crate status): leave these to the CLI.
    let sparse = repo.config_snapshot().boolean("core.sparseCheckout").unwrap_or(false);
    if sparse || repo.workdir().is_some_and(|dir| dir.join(".gitmodules").exists()) {
        return Err(err("sparse checkouts and submodules are left to git"));
    }
    let platform = repo
        .status(gix::progress::Discard)
        .map_err(err)?
        .untracked_files(gix::status::UntrackedFiles::Files)
        .index_worktree_rewrites(None);
    let iter = platform.into_iter(Vec::<BString>::new()).map_err(err)?;
    let mut files = HashSet::new();
    let mut untracked = HashSet::new();
    for item in iter {
        match item.map_err(err)? {
            gix::status::Item::IndexWorktree(item) => {
                let Some(summary) = item.summary() else { continue };
                let path = item.rela_path().to_str_lossy().into_owned();
                if summary == gix::status::index_worktree::iter::Summary::Added {
                    untracked.insert(path.clone());
                }
                files.insert(path);
            }
            gix::status::Item::TreeIndex(change) => {
                files.insert(change.location().to_str_lossy().into_owned());
            }
        }
    }
    let mut untracked: Vec<String> = untracked.into_iter().collect();
    untracked.sort();
    Ok(Summary { files: files.len(), untracked, upstream: upstream(&repo)? })
}

/// Commits `(ahead, behind)` the upstream of the branch checked out in `repo`; `None`
/// without a branch (detached, unborn) or without an upstream configured. An upstream
/// that's set but that `gix` can't follow (a local one, a remote branch that's gone) is
/// an error, so the CLI answers it.
fn upstream(repo: &gix::Repository) -> Result<Option<(usize, usize)>, String> {
    let head = repo.head().map_err(err)?;
    let Some(name) = head.referent_name() else { return Ok(None) };
    let direction = gix::remote::Direction::Fetch;
    let Some(tracking) = repo.branch_remote_tracking_ref_name(name, direction) else {
        return if repo.branch_remote_ref_name(name, direction).is_some() {
            Err(err("an upstream without a remote-tracking branch"))
        } else {
            Ok(None)
        };
    };
    let tracking = tracking.map_err(err)?;
    let theirs = repo.find_reference(tracking.as_ref()).map_err(err)?.into_fully_peeled_id().map_err(err)?.detach();
    let ours = repo.head_id().map_err(err)?.detach();
    count(repo, ours, theirs).map(Some).ok_or_else(|| err("couldn't walk the history"))
}

/// Commits `(only in `a`, only in `b`)`: `git rev-list --left-right --count a...b`.
pub fn ahead_behind(root: &Path, a: &str, b: &str) -> Result<(usize, usize), String> {
    let repo = open(root)?;
    let id = |rev: &str| -> Result<gix::ObjectId, String> {
        Ok(repo.rev_parse_single(rev).map_err(err)?.object().map_err(err)?.peel_to_commit().map_err(err)?.id)
    };
    count(&repo, id(a)?, id(b)?).ok_or_else(|| err("couldn't walk the history"))
}

/// Commits reachable from `a` but not `b`, and from `b` but not `a`.
fn count(repo: &gix::Repository, a: gix::ObjectId, b: gix::ObjectId) -> Option<(usize, usize)> {
    let only_in = |from: gix::ObjectId, not: gix::ObjectId| -> Option<usize> {
        let mut n = 0;
        for commit in repo.rev_walk([from]).with_hidden([not]).all().ok()? {
            commit.ok()?;
            n += 1;
        }
        Some(n)
    };
    Some((only_in(a, b)?, only_in(b, a)?))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::process::Command;

    use super::*;
    use crate::git::{Change, status};

    fn git_in(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kynex-native-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(dir: &Path, file: &str, text: &str) {
        let path = dir.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    /// What the CLI path says, in the shape of [`Summary`].
    fn cli(path: &Path) -> Summary {
        let st = status(path).unwrap();
        let mut untracked: Vec<String> =
            st.files.iter().filter(|f| f.unstaged == Some(Change::Untracked)).map(|f| f.path.clone()).collect();
        untracked.sort();
        Summary { files: st.files.len(), untracked, upstream: st.upstream.is_some().then_some((st.ahead, st.behind)) }
    }

    fn parity(path: &Path, step: &str) {
        assert_eq!(summary(path).unwrap(), cli(path), "{step}");
    }

    #[test]
    fn status_counts_match_the_cli_at_every_step() {
        let repo = scratch("status");
        git_in(&repo, &["init", "-q", "-b", "main"]);
        write(&repo, "a.txt", "one\ntwo\n");
        write(&repo, "src/b.txt", "b\n");
        write(&repo, "gone.txt", "bye\n");
        write(&repo, "old-name.txt", "a rename needs a few lines\nto be recognised\nas the same file\nwhen it moves\n");
        git_in(&repo, &["add", "."]);
        git_in(&repo, &["commit", "-q", "-m", "first"]);
        parity(&repo, "clean");
        assert_eq!(summary(&repo).unwrap().files, 0);

        write(&repo, "a.txt", "one\ntwo\nthree\n");
        parity(&repo, "unstaged edit");
        git_in(&repo, &["add", "a.txt"]);
        parity(&repo, "staged edit");
        write(&repo, "a.txt", "one\ntwo\nthree\nfour\n");
        parity(&repo, "staged and edited again (one file)");
        write(&repo, "new/deep/c.txt", "c\n");
        write(&repo, "top.txt", "t\n");
        parity(&repo, "untracked files, one in a new folder");
        git_in(&repo, &["add", "top.txt"]);
        parity(&repo, "a staged new file");
        std::fs::remove_file(repo.join("gone.txt")).unwrap();
        parity(&repo, "an unstaged delete");
        git_in(&repo, &["rm", "-q", "src/b.txt"]);
        parity(&repo, "a staged delete");
        git_in(&repo, &["mv", "old-name.txt", "new-name.txt"]);
        parity(&repo, "a staged rename");
        write(&repo, ".gitignore", "*.log\n");
        write(&repo, "debug.log", "ignored\n");
        parity(&repo, "an ignored file and a new .gitignore");
        let s = summary(&repo).unwrap();
        assert!(!s.untracked.iter().any(|f| f.ends_with(".log")));
        assert!(s.untracked.contains(&"new/deep/c.txt".to_string()), "{s:?}");
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn line_endings_and_intent_to_add_match_the_cli() {
        for autocrlf in ["true", "false", "input"] {
            let repo = scratch(&format!("crlf-{autocrlf}"));
            git_in(&repo, &["init", "-q", "-b", "main"]);
            git_in(&repo, &["config", "core.autocrlf", autocrlf]);
            write(&repo, "w.txt", "one\r\ntwo\r\n");
            write(&repo, "u.txt", "one\ntwo\n");
            git_in(&repo, &["add", "."]);
            git_in(&repo, &["commit", "-q", "-m", "first"]);
            parity(&repo, &format!("autocrlf={autocrlf}: clean after commit"));
            write(&repo, "w.txt", "one\r\ntwo\r\nthree\r\n");
            parity(&repo, &format!("autocrlf={autocrlf}: one CRLF file edited"));
            write(&repo, "later.txt", "x\r\n");
            git_in(&repo, &["add", "-N", "later.txt"]);
            parity(&repo, &format!("autocrlf={autocrlf}: intent to add"));
            let _ = std::fs::remove_dir_all(&repo);
        }
    }

    #[test]
    fn what_gix_cant_follow_is_left_to_the_cli() {
        let dir = scratch("fallback");
        let repo = dir.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git_in(&repo, &["init", "-q", "-b", "main"]);
        write(&repo, "a.txt", "a\n");
        git_in(&repo, &["add", "."]);
        git_in(&repo, &["commit", "-q", "-m", "first"]);
        assert!(summary(&repo).is_ok());
        // An upstream set to a local branch: gix has no remote-tracking ref to follow.
        git_in(&repo, &["branch", "side"]);
        git_in(&repo, &["branch", "--set-upstream-to=side", "main"]);
        assert!(summary(&repo).is_err());
        let _ = std::fs::remove_dir_all(&dir);

        // A sparse checkout.
        let sparse = scratch("sparse");
        git_in(&sparse, &["init", "-q", "-b", "main"]);
        git_in(&sparse, &["config", "core.sparseCheckout", "true"]);
        assert!(summary(&sparse).is_err());
        let _ = std::fs::remove_dir_all(&sparse);
    }

    #[test]
    fn a_linked_worktree_and_a_detached_head_match_too() {
        let dir = scratch("linked");
        let repo = dir.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git_in(&repo, &["init", "-q", "-b", "main"]);
        write(&repo, "a.txt", "a\n");
        git_in(&repo, &["add", "."]);
        git_in(&repo, &["commit", "-q", "-m", "first"]);
        let linked = dir.join("linked");
        git_in(&repo, &["worktree", "add", "-q", "-b", "feat", linked.to_str().unwrap()]);
        write(&linked, "a.txt", "changed\n");
        write(&linked, "only-here.txt", "x\n");
        parity(&linked, "a linked worktree with changes");
        parity(&repo, "the main tree, clean");
        git_in(&repo, &["checkout", "-q", "--detach"]);
        write(&repo, "d.txt", "d\n");
        parity(&repo, "a detached HEAD");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn upstream_distance_matches_the_cli() {
        let dir = scratch("upstream");
        let origin = dir.join("origin.git");
        std::fs::create_dir_all(&origin).unwrap();
        git_in(&origin, &["init", "-q", "--bare", "-b", "main"]);
        let (a, b) = (dir.join("a"), dir.join("b"));
        git_in(&dir, &["clone", "-q", origin.to_str().unwrap(), "a"]);
        write(&a, "f.txt", "1\n");
        git_in(&a, &["add", "."]);
        git_in(&a, &["commit", "-q", "-m", "one"]);
        git_in(&a, &["push", "-q", "-u", "origin", "main"]);
        parity(&a, "in sync");
        assert_eq!(summary(&a).unwrap().upstream, Some((0, 0)));

        git_in(&dir, &["clone", "-q", origin.to_str().unwrap(), "b"]);
        for n in 0..2 {
            write(&b, "g.txt", &format!("{n}\n"));
            git_in(&b, &["add", "."]);
            git_in(&b, &["commit", "-q", "-m", &format!("remote {n}")]);
        }
        git_in(&b, &["push", "-q"]);
        for n in 0..3 {
            write(&a, "h.txt", &format!("{n}\n"));
            git_in(&a, &["add", "."]);
            git_in(&a, &["commit", "-q", "-m", &format!("local {n}")]);
        }
        parity(&a, "ahead, and not yet fetched");
        git_in(&a, &["fetch", "-q"]);
        parity(&a, "ahead 3, behind 2");
        assert_eq!(summary(&a).unwrap().upstream, Some((3, 2)));

        // A branch with no upstream has none.
        git_in(&a, &["checkout", "-q", "-b", "lonely"]);
        parity(&a, "no upstream");
        assert_eq!(summary(&a).unwrap().upstream, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `cargo test -p kynex-core -- --ignored --nocapture speed`: this repository, `gix` against the CLI.
    #[test]
    #[ignore = "a timing, not a check"]
    fn speed_against_the_cli_on_this_repo() {
        let here = Path::new(env!("CARGO_MANIFEST_DIR"));
        let time = |f: &dyn Fn()| {
            f(); // warm up
            let start = std::time::Instant::now();
            for _ in 0..10 {
                f();
            }
            start.elapsed() / 10
        };
        println!("gix status summary: {:?}", time(&|| drop(summary(here).unwrap())));
        println!("cli status:         {:?}", time(&|| drop(status(here).unwrap())));
        println!("gix ahead/behind:   {:?}", time(&|| _ = ahead_behind(here, "HEAD", "HEAD~1").unwrap()));
        println!(
            "cli rev-list:       {:?}",
            time(&|| drop(crate::git::git(here, &["rev-list", "--left-right", "--count", "HEAD...HEAD~1"]).unwrap()))
        );
    }

    #[test]
    fn the_worktree_list_matches_the_cli() {
        let dir = scratch("list");
        let repo = dir.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        // An unborn branch first: the CLI lists it with an all-zero HEAD.
        git_in(&repo, &["init", "-q", "-b", "main"]);
        let unborn = worktrees(&repo).unwrap().unwrap();
        assert_eq!(unborn, crate::git::worktree::parse_worktrees(&git_in(&repo, &["worktree", "list", "--porcelain"])));

        write(&repo, "a.txt", "a\n");
        git_in(&repo, &["add", "."]);
        git_in(&repo, &["commit", "-q", "-m", "first"]);
        for (n, branch) in ["feat/one", "two", "three"].iter().enumerate() {
            let at = dir.join(format!("wt-{n}"));
            git_in(&repo, &["worktree", "add", "-q", "-b", branch, at.to_str().unwrap()]);
        }
        let detached = dir.join("wt-detached");
        git_in(&repo, &["worktree", "add", "-q", "--detach", detached.to_str().unwrap()]);
        git_in(&repo, &["worktree", "lock", dir.join("wt-1").to_str().unwrap()]);
        let want = crate::git::worktree::parse_worktrees(&git_in(&repo, &["worktree", "list", "--porcelain"]));
        // From the main tree and from inside a linked one.
        for from in [repo.clone(), dir.join("wt-0")] {
            let got = worktrees(&from).unwrap().unwrap();
            assert_eq!(got, want, "from {}", from.display());
        }
        assert!(want.iter().any(|w| w.locked) && want.iter().any(|w| w.branch.is_none()));

        // A worktree whose folder was deleted: the same answer, or a fall back to the CLI.
        std::fs::remove_dir_all(dir.join("wt-2")).unwrap();
        let want = crate::git::worktree::parse_worktrees(&git_in(&repo, &["worktree", "list", "--porcelain"]));
        if let Ok(got) = worktrees(&repo) {
            assert_eq!(got.unwrap(), want);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn outside_a_repository_there_is_nothing_to_list() {
        let dir = scratch("norepo");
        assert_eq!(worktrees(&dir), Ok(None));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ahead_behind_matches_rev_list() {
        let repo = scratch("revlist");
        git_in(&repo, &["init", "-q", "-b", "main"]);
        let commit = |msg: &str| {
            write(&repo, "f.txt", msg);
            git_in(&repo, &["add", "."]);
            git_in(&repo, &["commit", "-q", "-m", msg]);
        };
        commit("base");
        git_in(&repo, &["branch", "feat"]);
        commit("main 1");
        commit("main 2");
        git_in(&repo, &["checkout", "-q", "feat"]);
        commit("feat 1");
        for (a, b) in [("main", "feat"), ("feat", "main"), ("main", "main"), ("HEAD", "main")] {
            let out = git_in(&repo, &["rev-list", "--left-right", "--count", &format!("{a}...{b}")]);
            let want: Vec<usize> = out.split_whitespace().map(|n| n.parse().unwrap()).collect();
            assert_eq!(ahead_behind(&repo, a, b).unwrap(), (want[0], want[1]), "{a}...{b}");
        }
        assert!(ahead_behind(&repo, "main", "no-such-branch").is_err());
        let _ = std::fs::remove_dir_all(&repo);
    }
}
