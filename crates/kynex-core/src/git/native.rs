//! Reading git state in-process with `gix`: no `git` process to start, which on
//! Windows costs 40-60 ms a call and now and then stalls for half a second.
//!
//! Only reads live here: the worktree list, `git status`, how far a branch is from its
//! upstream or another branch, the branch list and the default branch: what Kynex asks
//! again and again. Each is tested against the CLI's answer on real repositories.
//!
//! Writes (stage, commit, switch, push, pull, fetch, worktree add / remove) stay on the
//! `git` CLI: it runs hooks, signs, applies `.gitattributes` and line-ending filters and
//! talks to the credential manager, none of which `gix` does as git does. Reads that
//! are rare or whose output is fiddly (history with its ref decorations, diffs, `git
//! cherry`) stay there too. Every function here returns an error when `gix` can't answer
//! (a sparse checkout, submodules, a repository owned by someone else…), and callers
//! fall back to the CLI's answer.

use std::collections::BTreeMap;
use std::path::Path;

use gix::bstr::{BString, ByteSlice};

use super::{Branch, Change, FileStatus, Head, Status, Worktree};

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

/// A branch's upstream (`origin/main`) and the commits `(ahead, behind)` it is.
type Upstream = (String, (usize, usize));

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
    // Git reports the real path (`/private/var/…` for a `/var/…` that's a symlink on macOS),
    // so the rest of Kynex, which compares these with paths git gave before, sees the same.
    let path = if cfg!(windows) {
        std::path::PathBuf::from(path.to_string_lossy().replace('/', "\\"))
    } else {
        path.canonicalize().unwrap_or(path)
    };
    Worktree { path, branch, head, main, locked, prunable, ..Default::default() }
}

/// The remote's default branch (`origin/HEAD`), else a local `main` or `master`: as
/// [`super::default_branch`] finds it, without a process.
pub fn default_branch(root: &Path) -> Result<Option<String>, String> {
    let repo = open(root)?;
    if let Some(origin_head) = repo.try_find_reference("refs/remotes/origin/HEAD").map_err(err)?
        && let gix::refs::TargetRef::Symbolic(target) = origin_head.target()
        && let Some(name) = target.as_bstr().to_str().ok().and_then(|t| t.strip_prefix("refs/remotes/origin/"))
    {
        return Ok(Some(name.to_string()));
    }
    for branch in ["main", "master"] {
        if repo.try_find_reference(format!("refs/heads/{branch}").as_str()).map_err(err)?.is_some() {
            return Ok(Some(branch.to_string()));
        }
    }
    Ok(None)
}

/// Local branches, then remote ones nobody tracks, newest first: the `git for-each-ref`
/// listing of [`super::branches`], without a process. A local branch whose upstream
/// `gix` can't follow (a local one) makes this an error, so the CLI answers it.
pub fn branches(root: &Path) -> Result<Vec<Branch>, String> {
    let repo = open(root)?;
    let head = repo.head_name().map_err(err)?.map(|name| name.to_string());
    // Where each branch is checked out, for the ones that are in another worktree.
    let checked_out: BTreeMap<String, std::path::PathBuf> =
        worktrees(root)?.unwrap_or_default().into_iter().filter_map(|w| Some((w.branch?, w.path))).collect();
    let direction = gix::remote::Direction::Fetch;
    let mut local = Vec::new();
    let mut remote = Vec::new();
    let platform = repo.references().map_err(err)?;
    // Git spells a name out (`heads/x`) when a tag has the same one; `gix` doesn't: ask git.
    let tags: std::collections::HashSet<String> =
        platform.tags().map_err(err)?.filter_map(Result::ok).map(|t| t.name().shorten().to_string()).collect();
    for (is_remote, refs) in
        [(false, platform.local_branches().map_err(err)?), (true, platform.remote_branches().map_err(err)?)]
    {
        for reference in refs {
            let mut reference = reference.map_err(err)?;
            let full = reference.name().as_bstr().to_string();
            if is_remote && full.ends_with("/HEAD") {
                continue;
            }
            let name = reference.name().shorten().to_string();
            if tags.contains(&name) {
                return Err(err("a branch and a tag share a name"));
            }
            let id = reference.peel_to_id().map_err(err)?.detach();
            let commit = repo.find_commit(id).map_err(err)?;
            let message = commit.message().map_err(err)?;
            let current = head.as_deref() == Some(full.as_str());
            let mut b = Branch {
                name: name.clone(),
                remote: is_remote,
                current,
                time: commit.time().map_err(err)?.seconds,
                sha: id.to_string(),
                // `%(contents:subject)`: the first paragraph's lines, joined by a space.
                subject: message.title.to_str_lossy().lines().map(str::trim).collect::<Vec<_>>().join(" "),
                worktree: checked_out.get(&name).filter(|_| !current).cloned(),
                ..Default::default()
            };
            if !is_remote {
                match repo.branch_remote_tracking_ref_name(reference.name(), direction) {
                    Some(tracking) => {
                        let tracking = tracking.map_err(err)?;
                        b.upstream = Some(tracking.shorten().to_string());
                        match repo.try_find_reference(tracking.as_ref()).map_err(err)? {
                            Some(mut theirs) => {
                                let theirs = theirs.peel_to_id().map_err(err)?.detach();
                                (b.ahead, b.behind) = count(&repo, id, theirs).ok_or_else(|| err("history"))?;
                            }
                            None => b.gone = true,
                        }
                    }
                    None if repo.branch_remote_ref_name(reference.name(), direction).is_some() => {
                        return Err(err("an upstream without a remote-tracking branch"));
                    }
                    None => {}
                }
            }
            if is_remote { remote.push((full, b)) } else { local.push((full, b)) }
        }
    }
    // `--sort=-committerdate`: newest first, and by name where the times are the same.
    let sorted = |mut list: Vec<(String, Branch)>| -> Vec<Branch> {
        list.sort_by(|(an, a), (bn, b)| b.time.cmp(&a.time).then_with(|| an.cmp(bn)));
        list.into_iter().map(|(_, b)| b).collect()
    };
    Ok(super::branch::assemble(sorted(local), sorted(remote)))
}

/// What the sidebar shows of the working tree at `path`: [`status`], boiled down.
pub fn summary(path: &Path) -> Result<Summary, String> {
    let st = status(path)?;
    let mut untracked: Vec<String> =
        st.files.iter().filter(|f| f.unstaged == Some(Change::Untracked)).map(|f| f.path.clone()).collect();
    untracked.sort();
    Ok(Summary { files: st.files.len(), untracked, upstream: st.upstream.is_some().then_some((st.ahead, st.behind)) })
}

/// `git status --porcelain=v2 --branch --untracked-files=all` for the working tree at
/// `path`, read in-process: the branch and where it stands against its upstream, and
/// every changed file split into what's staged and what isn't. Renames are found in the
/// index only (as git does); worktree-side renames show as a delete and an untracked file.
pub fn status(path: &Path) -> Result<Status, String> {
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
    // One entry per path: the index side (what's staged) and the working tree side.
    let mut files: BTreeMap<String, FileStatus> = BTreeMap::new();
    fn entry(files: &mut BTreeMap<String, FileStatus>, path: String) -> &mut FileStatus {
        files.entry(path.clone()).or_insert_with(|| FileStatus { path, orig_path: None, staged: None, unstaged: None })
    }
    for item in iter {
        match item.map_err(err)? {
            gix::status::Item::IndexWorktree(item) => {
                use gix::status::index_worktree::iter::Summary as S;
                let Some(summary) = item.summary() else { continue };
                let change = match summary {
                    S::Conflict => Change::Conflicted,
                    S::Removed => Change::Deleted,
                    S::Added => Change::Untracked,
                    S::IntentToAdd => Change::Added,
                    S::Modified => Change::Modified,
                    S::TypeChange => Change::TypeChanged,
                    S::Renamed => Change::Renamed,
                    S::Copied => Change::Copied,
                };
                let mut path = item.rela_path().to_str_lossy().into_owned();
                // Git names an untracked folder with a trailing slash (a nested repository, say).
                if let gix::status::index_worktree::Item::DirectoryContents { entry, .. } = &item
                    && matches!(
                        entry.disk_kind,
                        Some(gix::dir::entry::Kind::Directory | gix::dir::entry::Kind::Repository)
                    )
                {
                    path.push('/');
                }
                entry(&mut files, path).unstaged = Some(change);
            }
            gix::status::Item::TreeIndex(change) => {
                use gix::diff::index::Change as C;
                let (path, orig, staged) = match &change {
                    C::Addition { location, .. } => (location, None, Change::Added),
                    C::Deletion { location, .. } => (location, None, Change::Deleted),
                    C::Modification { location, previous_entry_mode, entry_mode, .. } => {
                        let kind = |m: &gix::index::entry::Mode| m.bits() & 0o170_000;
                        let kind_changed = kind(previous_entry_mode) != kind(entry_mode);
                        (location, None, if kind_changed { Change::TypeChanged } else { Change::Modified })
                    }
                    C::Rewrite { source_location, location, copy, .. } => {
                        let orig = Some(source_location.to_str_lossy().into_owned());
                        (location, orig, if *copy { Change::Copied } else { Change::Renamed })
                    }
                };
                let file = entry(&mut files, path.to_str_lossy().into_owned());
                file.staged = Some(staged);
                file.orig_path = orig;
            }
        }
    }
    let mut files: Vec<FileStatus> = files.into_values().collect();
    // A conflicted path is listed once, as git does: nothing staged, the conflict unstaged.
    for f in files.iter_mut().filter(|f| f.unstaged == Some(Change::Conflicted)) {
        f.staged = None;
        f.orig_path = None;
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));

    let head = repo.head().map_err(err)?;
    let oid = repo.head_id().map(|id| id.to_string()).unwrap_or_default();
    let head_state = match head.referent_name() {
        None => Head::Detached(oid.chars().take(7).collect()),
        Some(name) if oid.is_empty() => Head::Unborn(name.shorten().to_string()),
        Some(name) => Head::Branch(name.shorten().to_string()),
    };
    let (upstream, ahead, behind) = match upstream(&repo)? {
        Some((name, (ahead, behind))) => (Some(name), ahead, behind),
        None => (None, 0, 0),
    };
    Ok(Status { head: head_state, oid, upstream, ahead, behind, files })
}

/// The upstream of the branch checked out in `repo` (`origin/main`) and how many commits
/// `(ahead, behind)` it is; `None` without a branch (detached, unborn) or without an
/// upstream configured. An upstream that's set but that `gix` can't follow (a local one, a
/// remote branch that's gone) is an error, so the CLI answers it.
fn upstream(repo: &gix::Repository) -> Result<Option<Upstream>, String> {
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
    let Ok(ours) = repo.head_id() else { return Ok(None) };
    let counts = count(repo, ours.detach(), theirs).ok_or_else(|| err("couldn't walk the history"))?;
    Ok(Some((tracking.shorten().to_string(), counts)))
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
    use crate::git::status::status_cli as status;

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
        // The whole status, not just its counts: which side each change is on, renames, the branch.
        assert_eq!(super::status(path).unwrap(), status(path).unwrap(), "{step} (full status)");
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

    /// A commit dated `unix` seconds, so the sort order isn't down to the clock.
    fn commit_at(dir: &Path, msg: &str, unix: i64) {
        write(dir, "f.txt", msg);
        git_in(dir, &["add", "."]);
        let date = format!("{unix} +0000");
        let out = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@example.com", "commit", "-q", "-m", msg])
            .env("GIT_COMMITTER_DATE", &date)
            .env("GIT_AUTHOR_DATE", &date)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    }

    #[test]
    fn branches_and_the_default_branch_match_the_cli() {
        let dir = scratch("branches");
        let origin = dir.join("origin.git");
        std::fs::create_dir_all(&origin).unwrap();
        git_in(&origin, &["init", "-q", "--bare", "-b", "main"]);
        git_in(&dir, &["clone", "-q", origin.to_str().unwrap(), "a"]);
        let a = dir.join("a");
        commit_at(&a, "one", 1_700_000_000);
        git_in(&a, &["push", "-q", "-u", "origin", "main"]);
        assert_eq!(default_branch(&a).unwrap(), super::super::branch::default_branch_cli(&a));
        git_in(&a, &["remote", "set-head", "origin", "main"]);
        assert_eq!(default_branch(&a).unwrap(), Some("main".to_string()));
        assert_eq!(default_branch(&a).unwrap(), super::super::branch::default_branch_cli(&a));

        // Two branches with the same time (tie broken by name), a slash name, one tracked
        // and ahead, one whose upstream is deleted on the remote, one with no upstream.
        git_in(&a, &["branch", "zeta"]);
        git_in(&a, &["branch", "alpha"]);
        git_in(&a, &["branch", "feat/slash"]);
        git_in(&a, &["checkout", "-q", "-b", "gone-soon"]);
        commit_at(&a, "gone soon", 1_700_000_100);
        git_in(&a, &["push", "-q", "-u", "origin", "gone-soon"]);
        git_in(&a, &["checkout", "-q", "-b", "tracked"]);
        commit_at(&a, "tracked", 1_700_000_200);
        git_in(&a, &["push", "-q", "-u", "origin", "tracked"]);
        commit_at(&a, "tracked, ahead", 1_700_000_300);
        // A remote-only branch (someone else's), and a linked worktree holding a branch.
        git_in(&a, &["push", "-q", "origin", "main:their-work"]);
        let linked = dir.join("linked");
        git_in(&a, &["worktree", "add", "-q", "-b", "in-worktree", linked.to_str().unwrap()]);
        git_in(&a, &["push", "-q", "origin", "--delete", "gone-soon"]);
        git_in(&a, &["fetch", "-q", "--prune"]);
        for from in [&a, &linked] {
            let got = branches(from).unwrap();
            let want = super::super::branch::branches_cli(from).unwrap();
            assert_eq!(got, want, "from {}", from.display());
        }
        let got = branches(&a).unwrap();
        assert!(got.iter().any(|b| b.gone && b.name == "gone-soon"), "{got:?}");
        assert!(got.iter().any(|b| b.name == "tracked" && b.ahead == 1));
        assert!(got.iter().any(|b| b.name == "origin/their-work" && b.remote));
        assert!(got.iter().any(|b| b.name == "in-worktree" && b.worktree.is_some()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn untracked_folders_and_nested_repos_match_the_cli() {
        let repo = scratch("nested");
        git_in(&repo, &["init", "-q", "-b", "main"]);
        write(&repo, "a.txt", "a\n");
        git_in(&repo, &["add", "."]);
        git_in(&repo, &["commit", "-q", "-m", "first"]);
        std::fs::create_dir_all(repo.join("empty-dir/inner")).unwrap();
        parity(&repo, "an empty untracked folder");
        write(&repo, "plain/deep/x.txt", "x\n");
        parity(&repo, "an untracked folder with a file");
        let nested = repo.join("nested-repo");
        std::fs::create_dir_all(&nested).unwrap();
        git_in(&nested, &["init", "-q", "-b", "main"]);
        write(&nested, "n.txt", "n\n");
        parity(&repo, "an untracked repository inside");
        write(&repo, "name with spaces.txt", "s\n");
        write(&repo, "ünïcode.txt", "u\n");
        parity(&repo, "spaces and non-ASCII names");
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn ahead_behind_matches_with_skewed_commit_dates() {
        let repo = scratch("skew");
        git_in(&repo, &["init", "-q", "-b", "main"]);
        commit_at(&repo, "base", 1_700_000_000);
        git_in(&repo, &["branch", "side"]);
        // `main` gets commits dated *before* its base, `side` after: the clocks disagree.
        for (n, t) in [(1, 1_600_000_000), (2, 1_800_000_000), (3, 1_500_000_000), (4, 1_900_000_000)] {
            commit_at(&repo, &format!("main {n}"), t);
        }
        git_in(&repo, &["checkout", "-q", "side"]);
        for (n, t) in [(1, 1_750_000_000), (2, 1_450_000_000)] {
            commit_at(&repo, &format!("side {n}"), t);
        }
        for (a, b) in [("main", "side"), ("side", "main")] {
            let out = git_in(&repo, &["rev-list", "--left-right", "--count", &format!("{a}...{b}")]);
            let want: Vec<usize> = out.split_whitespace().map(|n| n.parse().unwrap()).collect();
            assert_eq!(ahead_behind(&repo, a, b).unwrap(), (want[0], want[1]), "{a}...{b}");
        }
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn conflicts_and_mode_changes_match_the_cli() {
        let repo = scratch("conflict");
        git_in(&repo, &["init", "-q", "-b", "main"]);
        write(&repo, "c.txt", "base\n");
        write(&repo, "x.sh", "#!/bin/sh\n");
        git_in(&repo, &["add", "."]);
        git_in(&repo, &["commit", "-q", "-m", "base"]);
        git_in(&repo, &["update-index", "--chmod=+x", "x.sh"]);
        parity(&repo, "a staged mode change");
        git_in(&repo, &["commit", "-q", "-m", "mode"]);
        git_in(&repo, &["checkout", "-q", "-b", "other"]);
        write(&repo, "c.txt", "other\n");
        git_in(&repo, &["commit", "-q", "-am", "other"]);
        git_in(&repo, &["checkout", "-q", "main"]);
        write(&repo, "c.txt", "main\n");
        git_in(&repo, &["commit", "-q", "-am", "main"]);
        // The merge stops on a conflict: git exits non-zero, which is the point.
        // (With an identity: CI machines have none, and a merge needs one.)
        let _ = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@example.com", "merge", "other"])
            .current_dir(&repo)
            .output()
            .unwrap();
        let st = status(&repo).unwrap();
        assert!(st.has_conflicts(), "{st:?}");
        parity(&repo, "a merge conflict");
        write(&repo, "c.txt", "resolved\n");
        git_in(&repo, &["add", "c.txt"]);
        parity(&repo, "the conflict resolved and staged");
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
