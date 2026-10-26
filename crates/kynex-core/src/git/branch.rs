//! Branches: listing (local and remote, newest first), switching, creating.

use std::path::Path;

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
}

/// Local branches, then remote ones that have no local branch of the same
/// name, each newest first.
pub fn branches(root: &Path) -> Result<Vec<Branch>, String> {
    let s = git(
        root,
        &[
            "for-each-ref",
            "--sort=-committerdate",
            "--format=%(refname)%1f%(HEAD)%1f%(upstream:short)%1f%(upstream:track,nobracket)%1f%(committerdate:unix)",
            "refs/heads",
            "refs/remotes",
        ],
    )?;
    Ok(parse(&s))
}

fn parse(s: &str) -> Vec<Branch> {
    let mut local = Vec::new();
    let mut remote = Vec::new();
    for line in s.lines() {
        let f: Vec<&str> = line.split('\x1f').collect();
        if f.len() < 5 {
            continue;
        }
        let mut b = Branch {
            current: f[1] == "*",
            upstream: Some(f[2]).filter(|u| !u.is_empty()).map(str::to_string),
            time: f[4].parse().unwrap_or(0),
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
        let short = r.name.split_once('/').map_or(r.name.as_str(), |(_, n)| n);
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

/// Creates a branch from HEAD and checks it out.
pub fn create_branch(root: &Path, name: &str) -> Result<(), String> {
    let name = name.trim();
    git(root, &["check-ref-format", "--branch", name]).map_err(|_| format!("\"{name}\" isn't a valid branch name"))?;
    git(root, &["switch", "-c", name]).map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_local_and_remote() {
        let s = "refs/heads/main\x1f*\x1forigin/main\x1fahead 1, behind 2\x1f1700000000\n\
                 refs/heads/old\x1f \x1forigin/old\x1fgone\x1f1600000000\n\
                 refs/remotes/origin/HEAD\x1f \x1f\x1f\x1f1700000000\n\
                 refs/remotes/origin/main\x1f \x1f\x1f\x1f1700000000\n\
                 refs/remotes/origin/feature\x1f \x1f\x1f\x1f1650000000\n";
        let list = parse(s);
        let names: Vec<_> = list.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names, ["main", "old", "origin/feature"]);
        assert!(list[0].current);
        assert_eq!((list[0].ahead, list[0].behind), (1, 2));
        assert!(list[1].gone);
        assert!(list[2].remote);
    }

    #[test]
    fn rejects_bad_names() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        assert!(create_branch(root, "bad..name").unwrap_err().contains("isn't a valid branch name"));
    }
}
