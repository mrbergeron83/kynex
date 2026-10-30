//! Which branches are finished and safe to delete: merged into the default
//! branch for real or by squash / rebase, not protected, not checked out.
//! [`overview`] gathers what the Branches tab shows; [`cleanup`] picks what
//! "Clean up merged…" offers.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use super::branch::{
    Branch, branches, default_branch, merged_into, protected_overrides, restore_branch, unique_commits,
};
use super::github::{self, PrState, PullRequest};
use super::worktree::{Worktree, list_worktrees};

/// How a branch stands against the default branch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Merged {
    /// Every commit is in it (`git branch --merged`).
    Yes,
    /// Its pull request was merged, or each change is in it under another
    /// commit (`git cherry`): squash or rebase merged.
    Squashed,
    /// Has this many changes the default branch doesn't.
    No(usize),
    /// Couldn't tell (no default branch, or git failed).
    Unknown,
}

impl Merged {
    pub fn done(self) -> bool {
        matches!(self, Self::Yes | Self::Squashed)
    }
}

/// Names protected when GitHub can't say: the usual long-lived branches.
pub const PROTECTED_NAMES: [&str; 6] = ["main", "master", "develop", "dev", "trunk", "release/*"];

fn protected_by_name(name: &str) -> bool {
    PROTECTED_NAMES.iter().any(|p| match p.strip_suffix('*') {
        Some(prefix) => name.starts_with(prefix),
        None => name == *p,
    })
}

/// Whether `name` is protected: the default branch always is; otherwise
/// `branch.<name>.kynex-protected` decides if it's set, then GitHub when it
/// answered, then [`PROTECTED_NAMES`].
pub fn is_protected(
    name: &str,
    default: Option<&str>,
    github: Option<&HashSet<String>>,
    overrides: &HashMap<String, bool>,
) -> bool {
    if default == Some(name) {
        return true;
    }
    if let Some(&set) = overrides.get(name) {
        return set;
    }
    match github {
        Some(set) => set.contains(name),
        None => protected_by_name(name),
    }
}

/// A branch and what's known about it.
#[derive(Clone, Debug, PartialEq)]
pub struct BranchRow {
    pub branch: Branch,
    pub merged: Merged,
    pub pr: Option<PullRequest>,
    pub protected: bool,
    pub default: bool,
}

impl BranchRow {
    /// Deleting it is refused: protected, the default, or in use.
    pub fn locked(&self) -> Option<&'static str> {
        if self.default {
            Some("It's the default branch")
        } else if self.protected {
            Some("It's protected")
        } else if self.branch.current {
            Some("It's checked out here")
        } else {
            None
        }
    }
}

/// Everything the Branches tab shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Overview {
    pub default: Option<String>,
    /// Local branches, then remote-only ones, newest first.
    pub branches: Vec<BranchRow>,
    pub worktrees: Vec<Worktree>,
    /// Pull requests and protection came from GitHub; else from names only.
    pub github: bool,
}

/// Reads branches, worktrees and what's merged, with `github`'s pull requests
/// and protection ([`github_info`]) when it answered. Many git calls; blocks.
pub fn overview(root: &Path, github: Option<&GitHubInfo>) -> Result<Overview, String> {
    let list = branches(root)?;
    let default = default_branch(root);
    let merged = default.as_deref().map(|d| merged_into(root, d).unwrap_or_default());
    let overrides = protected_overrides(root);
    let rows = list
        .into_iter()
        .map(|branch| {
            let pr = github.and_then(|g| g.prs.get(branch.short_name()).cloned());
            let state = match (&default, &merged) {
                _ if branch.remote => Merged::Unknown,
                (Some(d), Some(set)) => {
                    if set.contains(&branch.name) {
                        Merged::Yes
                    } else if pr.as_ref().is_some_and(|p| p.state == PrState::Merged) {
                        Merged::Squashed
                    } else {
                        match unique_commits(root, d, &branch.name) {
                            Ok(0) => Merged::Squashed,
                            Ok(n) => Merged::No(n),
                            Err(_) => Merged::Unknown,
                        }
                    }
                }
                _ => Merged::Unknown,
            };
            let name = branch.short_name().to_string();
            BranchRow {
                protected: is_protected(&name, default.as_deref(), github.map(|g| &g.protected), &overrides),
                default: default.as_deref() == Some(name.as_str()),
                merged: state,
                pr,
                branch,
            }
        })
        .collect();
    Ok(Overview {
        default,
        branches: rows,
        worktrees: list_worktrees(root).unwrap_or_default(),
        github: github.is_some(),
    })
}

/// What GitHub said, kept a few minutes so a refresh doesn't ask again.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GitHubInfo {
    pub prs: HashMap<String, PullRequest>,
    pub protected: HashSet<String>,
}

/// Pull requests and protection for the repo's local branches, or `None` when
/// `gh` isn't there or can't see the repo. A few `gh` calls; blocks.
pub fn github_info(root: &Path) -> Option<GitHubInfo> {
    let prs = github::pull_requests(root).ok()?;
    let list = branches(root).ok()?;
    let names: Vec<String> = list.iter().filter(|b| !b.remote).map(|b| b.name.clone()).collect();
    let protected = github::protected(root, &names).ok()?;
    Some(GitHubInfo { prs, protected })
}

/// One thing "Clean up merged…" would do.
#[derive(Clone, Debug, PartialEq)]
pub struct Cleanup {
    pub branch: String,
    pub sha: String,
    /// Its remote branch, still there: deleted too if asked.
    pub remote: Option<String>,
    /// The branch it tracks (gone or not), set again by [`undo`].
    pub upstream: Option<String>,
    /// A clean worktree it's checked out in, removed first.
    pub worktree: Option<std::path::PathBuf>,
}

/// The local branches that are finished: merged or squash-merged, with their
/// remote branch gone or their pull request merged, not protected, and not
/// checked out (except in a clean worktree, which goes with them).
pub fn cleanup(overview: &Overview) -> Vec<Cleanup> {
    overview
        .branches
        .iter()
        .filter(|r| !r.branch.remote && r.merged.done() && r.locked().is_none())
        .filter(|r| r.branch.gone || r.pr.as_ref().is_some_and(|p| p.state == PrState::Merged))
        .filter_map(|r| {
            let worktree = match &r.branch.worktree {
                None => None,
                Some(path) => {
                    let w = overview.worktrees.iter().find(|w| &w.path == path)?;
                    // Only a clean, linked, unlocked one goes; else the branch stays.
                    if w.main || w.locked || w.changes != Some(0) {
                        return None;
                    }
                    Some(path.clone())
                }
            };
            Some(Cleanup {
                branch: r.branch.name.clone(),
                sha: r.branch.sha.clone(),
                remote: r.branch.upstream.clone().filter(|_| !r.branch.gone),
                upstream: r.branch.upstream.clone(),
                worktree,
            })
        })
        .collect()
}

/// A deleted branch, to put back with [`undo`].
#[derive(Clone, Debug, PartialEq)]
pub struct Deleted {
    pub branch: String,
    pub sha: String,
    /// Its remote branch, if that was deleted too.
    pub remote: Option<String>,
    /// The branch it tracked.
    pub upstream: Option<String>,
}

/// Deletes `items`: each one's worktree, then the branch, then its remote
/// branch if `with_remote`. Branches go even when git doesn't see them as
/// merged (a squash merge looks unmerged to it): whoever calls this has asked
/// first. Goes on past failures; returns what was deleted and what went
/// wrong. Blocks; the remote deletes talk to the network.
pub fn delete(root: &Path, items: &[Cleanup], with_remote: bool) -> (Vec<Deleted>, Vec<String>) {
    let (mut done, mut errors) = (Vec::new(), Vec::new());
    for c in items {
        if let Some(w) = &c.worktree
            && let Err(e) = super::worktree::remove_worktree(root, w, false)
        {
            errors.push(format!("{}: {e}", c.branch));
            continue;
        }
        if let Err(e) = super::branch::delete_branch(root, &c.branch, true) {
            errors.push(format!("{}: {e}", c.branch));
            continue;
        }
        let mut remote = None;
        if with_remote && let Some(r) = &c.remote {
            match super::branch::delete_remote_branch(root, r) {
                Ok(()) => remote = Some(r.clone()),
                Err(e) => errors.push(format!("{r}: {e}")),
            }
        }
        done.push(Deleted { branch: c.branch.clone(), sha: c.sha.clone(), remote, upstream: c.upstream.clone() });
    }
    (done, errors)
}

/// Puts `deleted` branches back where they pointed, remote ones too. Blocks.
pub fn undo(root: &Path, deleted: &[Deleted]) -> Result<(), String> {
    let errors: Vec<String> = deleted
        .iter()
        .filter_map(|d| {
            restore_branch(root, &d.branch, &d.sha, d.remote.as_deref(), d.upstream.as_deref())
                .err()
                .map(|e| format!("{}: {e}", d.branch))
        })
        .collect();
    if errors.is_empty() { Ok(()) } else { Err(errors.join("\n")) }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn row(name: &str, merged: Merged) -> BranchRow {
        BranchRow {
            branch: Branch { name: name.into(), sha: format!("{name}-sha"), ..Default::default() },
            merged,
            pr: None,
            protected: false,
            default: false,
        }
    }

    fn pr(state: PrState) -> Option<PullRequest> {
        Some(PullRequest { number: 1, state, checks: None, url: String::new() })
    }

    /// In a real repository: a branch whose change landed on main as another
    /// commit reads as squash merged, one with its own work as unmerged; a
    /// delete and its undo put the branch back where it was.
    #[test]
    fn squash_merged_branches_delete_and_come_back() {
        let dir = std::env::temp_dir().join(format!("kynex-cleanup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str]| super::super::git(&dir, args).unwrap();
        let write = |f: &str, t: &str| std::fs::write(dir.join(f), t).unwrap();
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.name", "Test"]);
        git(&["config", "user.email", "test@example.com"]);
        git(&["config", "commit.gpgsign", "false"]);
        write("a.txt", "a\n");
        git(&["add", "-A"]);
        git(&["commit", "-qm", "base"]);
        git(&["switch", "-qc", "done"]);
        write("b.txt", "b\n");
        git(&["add", "-A"]);
        git(&["commit", "-qm", "feature"]);
        git(&["switch", "-q", "main"]);
        // Main moves on first, so the picked change lands as a commit of its own.
        write("m.txt", "m\n");
        git(&["add", "-A"]);
        git(&["commit", "-qm", "main moves"]);
        git(&["cherry-pick", "done"]);
        git(&["switch", "-qc", "open"]);
        write("c.txt", "c\n");
        git(&["add", "-A"]);
        git(&["commit", "-qm", "work"]);
        git(&["switch", "-q", "main"]);

        let o = overview(&dir, Some(&GitHubInfo::default())).unwrap();
        let state = |n: &str| o.branches.iter().find(|r| r.branch.name == n).unwrap().merged;
        assert_eq!(o.default.as_deref(), Some("main"));
        assert_eq!((state("main"), state("done"), state("open")), (Merged::Yes, Merged::Squashed, Merged::No(1)));
        let main = o.branches.iter().find(|r| r.branch.name == "main").unwrap();
        assert!(main.default && main.locked().is_some());

        let done = o.branches.iter().find(|r| r.branch.name == "done").unwrap();
        let item = Cleanup {
            branch: "done".into(),
            sha: done.branch.sha.clone(),
            remote: None,
            upstream: Some("origin/done".into()),
            worktree: None,
        };
        let (deleted, errors) = delete(&dir, &[item], false);
        assert!(errors.is_empty(), "{errors:?}");
        assert!(branches(&dir).unwrap().iter().all(|b| b.name != "done"));
        undo(&dir, &deleted).unwrap();
        let back = branches(&dir).unwrap().into_iter().find(|b| b.name == "done").unwrap();
        assert_eq!(back.sha, done.branch.sha);
        // It tracks what it did, even though that remote branch doesn't exist.
        assert_eq!(git(&["config", "branch.done.merge"]).trim(), "refs/heads/done");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn protection_rules() {
        let none = HashMap::new();
        let gh = HashSet::from(["prod".to_string()]);
        // Without GitHub, names decide.
        assert!(is_protected("main", None, None, &none));
        assert!(is_protected("release/1.2", None, None, &none));
        assert!(!is_protected("feature", None, None, &none));
        // GitHub's answer replaces the names; the default always counts.
        assert!(!is_protected("develop", Some("main"), Some(&gh), &none));
        assert!(is_protected("prod", Some("main"), Some(&gh), &none));
        assert!(is_protected("main", Some("main"), Some(&HashSet::new()), &none));
        // git config wins over both, but not over being the default.
        let over =
            HashMap::from([("develop".to_string(), false), ("mine".to_string(), true), ("main".to_string(), false)]);
        assert!(!is_protected("develop", None, None, &over));
        assert!(is_protected("mine", None, Some(&gh), &over));
        assert!(is_protected("main", Some("main"), None, &over));
    }

    #[test]
    fn cleanup_picks_finished_branches_only() {
        let mut gone = row("gone", Merged::Squashed);
        gone.branch.gone = true;
        gone.branch.upstream = Some("origin/gone".into());
        let mut pr_merged = row("pr-merged", Merged::Squashed);
        pr_merged.pr = pr(PrState::Merged);
        pr_merged.branch.upstream = Some("origin/pr-merged".into());
        // Merged, but nothing says the work is done (remote still there, no PR).
        let mut live = row("live", Merged::Yes);
        live.branch.upstream = Some("origin/live".into());
        let mut unmerged = row("unmerged", Merged::No(3));
        unmerged.branch.gone = true;
        let mut protected = row("release/1", Merged::Yes);
        protected.protected = true;
        protected.branch.gone = true;
        let mut current = row("current", Merged::Yes);
        current.branch.current = true;
        current.branch.gone = true;
        let mut in_clean = row("in-clean", Merged::Yes);
        in_clean.branch.gone = true;
        in_clean.branch.worktree = Some(PathBuf::from("/w/clean"));
        let mut in_dirty = row("in-dirty", Merged::Yes);
        in_dirty.branch.gone = true;
        in_dirty.branch.worktree = Some(PathBuf::from("/w/dirty"));
        let wt =
            |path: &str, changes| Worktree { path: PathBuf::from(path), changes: Some(changes), ..Default::default() };
        let overview = Overview {
            branches: vec![gone, pr_merged, live, unmerged, protected, current, in_clean, in_dirty],
            worktrees: vec![wt("/w/clean", 0), wt("/w/dirty", 2)],
            ..Default::default()
        };
        let picked = cleanup(&overview);
        let names: Vec<_> = picked.iter().map(|c| c.branch.as_str()).collect();
        assert_eq!(names, ["gone", "pr-merged", "in-clean"]);
        // A gone remote has nothing left to delete; a merged PR's branch may.
        assert_eq!(picked[0].remote, None);
        assert_eq!(picked[1].remote.as_deref(), Some("origin/pr-merged"));
        assert_eq!(picked[2].worktree, Some(PathBuf::from("/w/clean")));
        assert_eq!(picked[0].sha, "gone-sha");
    }
}
