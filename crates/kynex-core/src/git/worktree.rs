//! A repository's working trees: `git worktree list`, plus each one's changes
//! and distance from the main branch.

use std::path::{Path, PathBuf};

use super::{Change, diff_stat, git, status};

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
    /// Lines `(added, removed)` against HEAD, untracked files included.
    pub lines: Option<(usize, usize)>,
    /// Commits `(ahead, behind)` the main working tree's branch.
    pub ahead_behind: Option<(usize, usize)>,
    /// Commits `(ahead, behind)` its upstream, when it has one.
    pub upstream: Option<(usize, usize)>,
}

impl Worktree {
    /// The branch name, or the short hash when detached.
    pub fn label(&self) -> &str {
        self.branch.as_deref().unwrap_or(&self.head)
    }
}

/// Every working tree of the repository `root` belongs to, main first, with
/// each one's changes (files and lines), distance from its upstream and from
/// the main branch. Runs a few `git` commands per worktree.
pub fn list_worktrees(root: &Path) -> Result<Vec<Worktree>, String> {
    let mut list = parse_worktrees(&git(root, &["worktree", "list", "--porcelain"])?);
    let base = list.first().and_then(|w| w.branch.clone());
    for w in &mut list {
        if w.prunable {
            continue;
        }
        if let Ok(st) = status(&w.path) {
            w.changes = Some(st.files.len());
            w.upstream = st.upstream.is_some().then_some((st.ahead, st.behind));
            let untracked: Vec<&str> =
                st.files.iter().filter(|f| f.unstaged == Some(Change::Untracked)).map(|f| f.path.as_str()).collect();
            w.lines = if st.files.is_empty() { Some((0, 0)) } else { diff_stat(&w.path, &untracked).ok() };
        }
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

/// The linked worktree (never the main one) that `text` points into, by its
/// full path (`C:\repos\kynex-terminal\src\a.rs`) or by its folder name after a
/// separator (`cd ../kynex-terminal`). For telling which worktree a session is
/// really working in from its tool calls, JSON-escaped or not.
pub fn worktree_in<'a>(text: &str, worktrees: &'a [Worktree]) -> Option<&'a Worktree> {
    let norm = |s: &str| {
        let mut out = s.to_lowercase().replace('\\', "/");
        while out.contains("//") {
            out = out.replace("//", "/");
        }
        out
    };
    let text = norm(text);
    let ends_path =
        |rest: &str| rest.chars().next().is_none_or(|c| matches!(c, '/' | '"' | '\'' | ' ' | ';' | ')' | '`' | '\n'));
    let linked = || worktrees.iter().filter(|w| !w.main && !w.prunable);
    let by_path = linked().find(|w| {
        let path = norm(&w.path.to_string_lossy());
        text.match_indices(&path).any(|(i, m)| ends_path(&text[i + m.len()..]))
    });
    by_path.or_else(|| {
        linked().find(|w| {
            let Some(name) = w.path.file_name().map(|n| format!("/{}", n.to_string_lossy().to_lowercase())) else {
                return false;
            };
            text.match_indices(&name).any(|(i, m)| ends_path(&text[i + m.len()..]))
        })
    })
}

/// Parses `git worktree list --porcelain`; bare entries are skipped.
pub(crate) fn parse_worktrees(s: &str) -> Vec<Worktree> {
    let mut list = Vec::new();
    for block in s.split("\n\n").filter(|b| !b.trim().is_empty()) {
        let mut w = Worktree::default();
        let mut bare = false;
        for line in block.lines() {
            let (key, value) = line.split_once(' ').unwrap_or((line, ""));
            match key {
                // git prints `/` everywhere; keep paths the way the OS writes them.
                "worktree" => {
                    w.path = PathBuf::from(if cfg!(windows) { value.replace('/', "\\") } else { value.to_string() })
                }
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
pub(crate) fn parse_left_right(s: &str) -> Option<(usize, usize)> {
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
    fn finds_the_worktree_a_tool_call_touches() {
        let wt = |path: &str, main| Worktree { path: PathBuf::from(path), main, ..Default::default() };
        let list = [wt("C:/repos/kynex", true), wt("C:/repos/kynex-terminal", false), wt("C:/repos/kynex-git", false)];
        let found = |text: &str| worktree_in(text, &list).map(|w| w.path.clone());
        let terminal = Some(PathBuf::from("C:/repos/kynex-terminal"));
        // A JSON-escaped Windows path in an Edit call.
        assert_eq!(found(r#"{"file_path":"C:\\Repos\\kynex-terminal\\src\\main.rs"}"#), terminal);
        // A relative cd in a shell command.
        assert_eq!(found(r#"{"command":"Set-Location ..\\kynex-terminal; cargo check"}"#), terminal);
        assert_eq!(found("cd ../kynex-terminal && git status"), terminal);
        // The main worktree, and names that only share a prefix, don't count.
        assert_eq!(found(r#"{"file_path":"C:\\repos\\kynex\\src\\main.rs"}"#), None);
        assert_eq!(found("cd ../kynex-terminally"), None);
        assert_eq!(found("ls"), None);
    }

    #[test]
    fn parses_left_right_counts() {
        assert_eq!(parse_left_right("3\t5\n"), Some((3, 5)));
        assert_eq!(parse_left_right("garbage"), None);
    }
}
