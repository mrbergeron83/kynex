//! Branches: listing (local and remote, newest first), switching, creating,
//! renaming, publishing and deleting, and what a branch has that its base doesn't.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use super::git;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Branch {
    /// e.g. `main`, or `origin/main` for a remote one.
    pub name: String,
    pub remote: bool,
    /// Checked out here.
    pub current: bool,
    pub upstream: Option<String>,
    pub ahead: usize,
    pub behind: usize,
    /// The upstream was deleted on the remote.
    pub gone: bool,
    /// Last commit time, seconds since the Unix epoch.
    pub time: i64,
    /// The commit it points to (full hash), to bring it back after a delete.
    pub sha: String,
    /// The last commit's subject line.
    pub subject: String,
    /// The worktree it's checked out in, when that's another one than here.
    pub worktree: Option<PathBuf>,
}

impl Branch {
    /// A remote branch's name without its remote: `origin/feature` → `feature`.
    pub fn short_name(&self) -> &str {
        if self.remote { self.name.split_once('/').map_or(self.name.as_str(), |(_, n)| n) } else { &self.name }
    }
}

/// Local branches, then remote ones that have no local branch of the same
/// name, each newest first.
pub fn branches(root: &Path) -> Result<Vec<Branch>, String> {
    let s = git(
        root,
        &[
            "for-each-ref",
            "--sort=-committerdate",
            "--format=%(refname)%1f%(HEAD)%1f%(upstream:short)%1f%(upstream:track,nobracket)%1f%(committerdate:unix)\
             %1f%(objectname)%1f%(worktreepath)%1f%(contents:subject)",
            "refs/heads",
            "refs/remotes",
        ],
    )?;
    Ok(parse(&s))
}

pub(crate) fn parse(s: &str) -> Vec<Branch> {
    let mut local = Vec::new();
    let mut remote = Vec::new();
    for line in s.lines() {
        let f: Vec<&str> = line.splitn(8, '\x1f').collect();
        if f.len() < 5 {
            continue;
        }
        let field = |i: usize| f.get(i).copied().unwrap_or("");
        let current = f[1] == "*";
        let mut b = Branch {
            current,
            upstream: Some(f[2]).filter(|u| !u.is_empty()).map(str::to_string),
            time: f[4].parse().unwrap_or(0),
            sha: field(5).to_string(),
            // git prints `/` everywhere; keep paths the way the OS writes them.
            worktree: Some(field(6))
                .filter(|w| !w.is_empty() && !current)
                .map(|w| PathBuf::from(if cfg!(windows) { w.replace('/', "\\") } else { w.to_string() })),
            subject: field(7).to_string(),
            ..Default::default()
        };
        for part in f[3].split(", ") {
            if let Some(n) = part.strip_prefix("ahead ") {
                b.ahead = n.parse().unwrap_or(0);
            } else if let Some(n) = part.strip_prefix("behind ") {
                b.behind = n.parse().unwrap_or(0);
            } else if part == "gone" {
                b.gone = true;
            }
        }
        if let Some(name) = f[0].strip_prefix("refs/heads/") {
            b.name = name.to_string();
            local.push(b);
        } else if let Some(name) = f[0].strip_prefix("refs/remotes/") {
            if name.ends_with("/HEAD") {
                continue;
            }
            b.name = name.to_string();
            b.remote = true;
            remote.push(b);
        }
    }
    // A remote branch that's already tracked (or has a local twin) is noise in the picker.
    remote.retain(|r: &Branch| {
        let short = r.short_name();
        !local.iter().any(|l| l.upstream.as_deref() == Some(&r.name) || l.name == short)
    });
    local.extend(remote);
    local
}

/// Checks out a branch. A remote one (`origin/feature`) gets a local branch
/// tracking it.
pub fn switch_branch(root: &Path, branch: &Branch) -> Result<(), String> {
    if branch.remote {
        git(root, &["switch", "--track", &branch.name]).map(drop)
    } else {
        git(root, &["switch", &branch.name]).map(drop)
    }
}

fn check_name(root: &Path, name: &str) -> Result<(), String> {
    git(root, &["check-ref-format", "--branch", name])
        .map(drop)
        .map_err(|_| format!("\"{name}\" isn't a valid branch name"))
}

/// Creates a branch from HEAD and checks it out.
pub fn create_branch(root: &Path, name: &str) -> Result<(), String> {
    let name = name.trim();
    check_name(root, name)?;
    git(root, &["switch", "-c", name]).map(drop)
}

/// Creates a branch at `start` (a branch or commit) and checks it out.
pub fn create_branch_at(root: &Path, name: &str, start: &str) -> Result<(), String> {
    let name = name.trim();
    check_name(root, name)?;
    git(root, &["switch", "-c", name, start]).map(drop)
}

/// Renames local branch `old` to `new`.
pub fn rename_branch(root: &Path, old: &str, new: &str) -> Result<(), String> {
    let new = new.trim();
    check_name(root, new)?;
    git(root, &["branch", "-m", old, new]).map(drop)
}

/// Pushes local branch `name` to the remote (`origin`, or the only one) and
/// tracks it there.
pub fn publish_branch(root: &Path, name: &str) -> Result<(), String> {
    let remote = default_remote(root)?;
    git(root, &["push", "--set-upstream", &remote, &format!("refs/heads/{name}:refs/heads/{name}")]).map(drop)
}

/// Deletes local branch `name`: only if it's merged, unless `force`.
pub fn delete_branch(root: &Path, name: &str, force: bool) -> Result<(), String> {
    git(root, &["branch", if force { "-D" } else { "-d" }, name]).map(drop)
}

/// Deletes `remote/branch` (as `origin/feature`) on the remote.
pub fn delete_remote_branch(root: &Path, remote_branch: &str) -> Result<(), String> {
    let (remote, branch) =
        remote_branch.split_once('/').ok_or_else(|| format!("\"{remote_branch}\" isn't a remote branch"))?;
    git(root, &["push", remote, "--delete", branch]).map(drop)
}

/// Puts local branch `name` back at `sha` after a delete, and its remote
/// branch too when `remote_branch` (as `origin/feature`) was deleted with it.
/// It tracks `upstream` again, even one that's gone from the remote.
pub fn restore_branch(
    root: &Path,
    name: &str,
    sha: &str,
    remote_branch: Option<&str>,
    upstream: Option<&str>,
) -> Result<(), String> {
    git(root, &["branch", name, sha])?;
    if let Some((remote, branch)) = remote_branch.and_then(|r| r.split_once('/')) {
        git(root, &["push", remote, &format!("{sha}:refs/heads/{branch}")])?;
    }
    // Written as config: `--set-upstream-to` refuses a remote branch that's gone.
    if let Some((remote, branch)) = upstream.and_then(|u| u.split_once('/')) {
        git(root, &["config", &format!("branch.{name}.remote"), remote])?;
        git(root, &["config", &format!("branch.{name}.merge"), &format!("refs/heads/{branch}")])?;
    }
    Ok(())
}

/// The remote to push to: `origin`, or the only one there is.
fn default_remote(root: &Path) -> Result<String, String> {
    let remotes = git(root, &["remote"])?;
    let remotes: Vec<&str> = remotes.lines().collect();
    match remotes.as_slice() {
        [] => Err("This repository has no remote to push to".into()),
        [only] => Ok(only.to_string()),
        many => Ok(many.iter().find(|r| **r == "origin").unwrap_or(&many[0]).to_string()),
    }
}

/// The remote's default branch (`origin/HEAD`), else a local `main` or `master`.
pub fn default_branch(root: &Path) -> Option<String> {
    if let Ok(s) = git(root, &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"])
        && let Some((_, name)) = s.trim().split_once('/')
    {
        return Some(name.to_string());
    }
    ["main", "master"]
        .into_iter()
        .find(|b| git(root, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{b}")]).is_ok())
        .map(Into::into)
}

/// Local branches whose commits are all in `base` (`git branch --merged`).
pub fn merged_into(root: &Path, base: &str) -> Result<HashSet<String>, String> {
    let s = git(root, &["branch", "--merged", base, "--format=%(refname:short)"])?;
    Ok(s.lines().map(str::trim).filter(|l| !l.is_empty()).map(Into::into).collect())
}

/// How many of `branch`'s commits have no equivalent change in `base`
/// (`git cherry`): 0 when it was merged by rebase, or squashed from one commit.
pub fn unique_commits(root: &Path, base: &str, branch: &str) -> Result<usize, String> {
    Ok(parse_cherry(&git(root, &["cherry", base, branch])?))
}

pub(crate) fn parse_cherry(s: &str) -> usize {
    s.lines().filter(|l| l.starts_with('+')).count()
}

/// `branch.<name>.kynex-protected` from git config: a branch marked protected
/// or not by hand, over what GitHub or the name says.
pub fn protected_overrides(root: &Path) -> HashMap<String, bool> {
    // Exit code 1 means none are set.
    let s = git(root, &["config", "--get-regexp", r"^branch\..*\.kynex-protected$"]).unwrap_or_default();
    parse_overrides(&s)
}

pub(crate) fn parse_overrides(s: &str) -> HashMap<String, bool> {
    s.lines()
        .filter_map(|l| {
            let (key, value) = l.split_once(' ')?;
            let name = key.strip_prefix("branch.")?.strip_suffix(".kynex-protected")?;
            let on = matches!(value.trim().to_ascii_lowercase().as_str(), "true" | "yes" | "on" | "1");
            Some((name.to_string(), on))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_local_and_remote() {
        let s = "refs/heads/main\x1f*\x1forigin/main\x1fahead 1, behind 2\x1f1700000000\x1fabc\x1fC:/repos/k\x1fFix it\n\
                 refs/heads/old\x1f \x1forigin/old\x1fgone\x1f1600000000\x1fdef\x1fC:/repos/k-old\x1fOld: a\x1fb\n\
                 refs/remotes/origin/HEAD\x1f \x1f\x1f\x1f1700000000\n\
                 refs/remotes/origin/main\x1f \x1f\x1f\x1f1700000000\n\
                 refs/remotes/origin/feature\x1f \x1f\x1f\x1f1650000000\x1f123\x1f\x1fFeature\n";
        let list = parse(s);
        let names: Vec<_> = list.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names, ["main", "old", "origin/feature"]);
        assert!(list[0].current);
        assert_eq!((list[0].ahead, list[0].behind), (1, 2));
        // The worktree it's checked out in only counts when it's another one.
        assert_eq!(list[0].worktree, None);
        assert_eq!(list[1].worktree, Some(PathBuf::from("C:/repos/k-old")));
        // A separator in the subject stays in it.
        assert_eq!(list[1].subject, "Old: a\x1fb");
        assert!(list[1].gone);
        assert!(list[2].remote);
        assert_eq!((list[2].short_name(), list[2].sha.as_str()), ("feature", "123"));
    }

    #[test]
    fn rejects_bad_names() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        assert!(create_branch(root, "bad..name").unwrap_err().contains("isn't a valid branch name"));
        assert!(rename_branch(root, "x", "bad..name").unwrap_err().contains("isn't a valid branch name"));
    }

    #[test]
    fn reads_cherry_and_overrides() {
        assert_eq!(parse_cherry("- abc\n+ def\n+ 123\n"), 2);
        assert_eq!(parse_cherry(""), 0);
        let o = parse_overrides("branch.main.kynex-protected false\nbranch.release/x.kynex-protected true\nnoise\n");
        assert_eq!(o.get("main"), Some(&false));
        assert_eq!(o.get("release/x"), Some(&true));
        assert_eq!(o.len(), 2);
    }
}
