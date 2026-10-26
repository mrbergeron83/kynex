//! Reading a project's git state: branch, changed files, history, diffs and
//! worktrees. Everything here runs `git` and blocks; call it off the UI thread.

use std::path::{Path, PathBuf};
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

/// One working tree of a repository, as `git worktree list` sees it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Worktree {
    pub path: PathBuf,
    /// Short branch name; `None` when the HEAD is detached.
    pub branch: Option<String>,
    /// Short commit hash of the HEAD.
    pub head: String,
    /// The repository's main working tree (listed first by git).
    pub main: bool,
    pub locked: bool,
    /// Its folder is gone; `git worktree prune` would drop it.
    pub prunable: bool,
    /// Changed files (staged, unstaged or untracked), if it could be read.
    pub changes: Option<usize>,
    /// Commits `(ahead, behind)` the main working tree's branch.
    pub ahead_behind: Option<(usize, usize)>,
}

impl Worktree {
    /// The branch name, or the short hash when detached.
    pub fn label(&self) -> &str {
        self.branch.as_deref().unwrap_or(&self.head)
    }

    /// What's notable about it, e.g. `["merged", "3 changed"]`: "merged" means its
    /// branch has nothing main lacks, so it's safe to remove once it's clean.
    pub fn details(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.main {
            out.push("main worktree".into());
        }
        match self.ahead_behind {
            Some((0, _)) => out.push("merged".into()),
            Some((ahead, 0)) => out.push(format!("{ahead} ahead")),
            Some((ahead, behind)) => out.push(format!("{ahead} ahead, {behind} behind")),
            None => {}
        }
        if let Some(n) = self.changes.filter(|&n| n > 0) {
            out.push(format!("{n} changed"));
        }
        if self.locked {
            out.push("locked".into());
        }
        if self.prunable {
            out.push("folder missing".into());
        }
        out
    }
}

/// Every working tree of the repository `root` belongs to, main first, with
/// each one's changed-file count and how far it is from the main branch.
/// Runs a few `git` commands per worktree.
pub fn list_worktrees(root: &Path) -> Result<Vec<Worktree>, String> {
    let mut list = parse_worktrees(&git(root, &["worktree", "list", "--porcelain"])?);
    let base = list.first().and_then(|w| w.branch.clone());
    for w in &mut list {
        if w.prunable {
            continue;
        }
        w.changes = git(&w.path, &["status", "--porcelain"]).ok().map(|s| s.lines().count());
        if let (Some(base), Some(branch)) = (&base, &w.branch)
            && !w.main
        {
            w.ahead_behind = git(root, &["rev-list", "--left-right", "--count", &format!("{base}...{branch}")])
                .ok()
                .and_then(|s| parse_left_right(&s))
                .map(|(behind, ahead)| (ahead, behind));
        }
    }
    Ok(list)
}

/// Parses `git worktree list --porcelain`; bare entries are skipped.
fn parse_worktrees(s: &str) -> Vec<Worktree> {
    let mut list = Vec::new();
    for block in s.split("\n\n").filter(|b| !b.trim().is_empty()) {
        let mut w = Worktree::default();
        let mut bare = false;
        for line in block.lines() {
            let (key, value) = line.split_once(' ').unwrap_or((line, ""));
            match key {
                "worktree" => w.path = PathBuf::from(value),
                "HEAD" => w.head = value.chars().take(7).collect(),
                "branch" => w.branch = Some(value.trim_start_matches("refs/heads/").to_string()),
                "bare" => bare = true,
                "locked" => w.locked = true,
                "prunable" => w.prunable = true,
                _ => {}
            }
        }
        if !bare {
            w.main = list.is_empty();
            list.push(w);
        }
    }
    list
}

/// Parses `git rev-list --left-right --count`: `"<left>\t<right>"`.
fn parse_left_right(s: &str) -> Option<(usize, usize)> {
    let mut it = s.split_whitespace().map(str::parse::<usize>);
    Some((it.next()?.ok()?, it.next()?.ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_worktree_list() {
        let s = "worktree C:/repos/kynex\nHEAD cc450cc644cecaa5f6871f1fae3b84102351caaf\nbranch refs/heads/main\n\n\
                 worktree C:/repos/kynex-select\nHEAD bf0f8bef6b93d1c47621f4a17f1922f35959fdec\nbranch refs/heads/selectable-text\nlocked\n\n\
                 worktree C:/repos/gone\nHEAD bf0f8bef6b93d1c47621f4a17f1922f35959fdec\ndetached\nprunable gitdir file points to non-existent location\n";
        let list = parse_worktrees(s);
        assert_eq!(list.len(), 3);
        assert!(list[0].main && !list[1].main);
        assert_eq!(list[0].label(), "main");
        assert_eq!(list[1].branch.as_deref(), Some("selectable-text"));
        assert!(list[1].locked);
        assert_eq!(list[2].label(), "bf0f8be");
        assert!(list[2].prunable);
        // `/` and `\` are both separators on Windows, so this matches a saved project path.
        assert_eq!(list[1].path, PathBuf::from("C:/repos/kynex-select"));
    }

    #[test]
    fn skips_bare_repos() {
        let list =
            parse_worktrees("worktree /srv/repo.git\nbare\n\nworktree /srv/wt\nHEAD abcdef0123\nbranch refs/heads/x\n");
        assert_eq!(list.len(), 1);
        assert!(list[0].main);
    }

    #[test]
    fn lists_this_repos_worktrees() {
        let list = list_worktrees(Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap();
        assert!(list[0].main);
        assert!(list.iter().all(|w| w.prunable || w.changes.is_some()));
    }

    #[test]
    fn worktree_details() {
        let w = |ahead_behind, changes| Worktree { ahead_behind, changes, ..Default::default() };
        assert_eq!(w(Some((0, 5)), Some(0)).details(), ["merged"]);
        assert_eq!(w(Some((2, 0)), Some(3)).details(), ["2 ahead", "3 changed"]);
        assert_eq!(w(Some((2, 1)), None).details(), ["2 ahead, 1 behind"]);
        assert_eq!(Worktree { main: true, ..Default::default() }.details(), ["main worktree"]);
    }

    #[test]
    fn parses_left_right_counts() {
        assert_eq!(parse_left_right("3\t5\n"), Some((3, 5)));
        assert_eq!(parse_left_right("garbage"), None);
    }

    #[test]
    fn parses_porcelain() {
        let s = parse_status("## main...origin/main\n M src/app.rs\n?? new.rs\nR  a.rs -> b.rs\n");
        assert_eq!(s.branch, "main...origin/main");
        assert_eq!(
            s.files,
            [(" M", "src/app.rs"), ("??", "new.rs"), ("R ", "a.rs -> b.rs")]
                .map(|(a, b)| (a.to_string(), b.to_string()))
        );
    }
}
