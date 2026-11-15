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
    let mut list = list_worktrees_quick(root)?;
    fill_worktrees(root, &mut list);
    Ok(list)
}

/// The working trees as `git worktree list` names them, without their changes or
/// distances: for showing the list at once while [`fill_worktrees`] works out the
/// rest. Read in-process; when that can't answer, one `git` call.
pub fn list_worktrees_quick(root: &Path) -> Result<Vec<Worktree>, String> {
    let _t = crate::trace::span("list_worktrees_quick (background)");
    match super::native::worktrees(root) {
        Ok(Some(list)) => Ok(list),
        Ok(None) => Err("Not a git repository".into()),
        Err(_) => Ok(parse_worktrees(&git(root, &["worktree", "list", "--porcelain"])?)),
    }
}

/// Works out each worktree's changes and distances, one thread per worktree (each
/// is several `git` calls, and starting a process can take a while). Blocking.
pub fn fill_worktrees(root: &Path, list: &mut [Worktree]) {
    let _t = crate::trace::span("fill_worktrees (background)");
    let base = list.first().and_then(|w| w.branch.clone());
    std::thread::scope(|scope| {
        for w in list.iter_mut() {
            let base = base.as_deref();
            // One worktree that can't be read (or a panic in what reads it) leaves its numbers
            // empty; it must not take the others, or the caller, down with it.
            scope.spawn(move || {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| fill_worktree(root, base, w)));
            });
        }
    });
}

fn fill_worktree(root: &Path, base: Option<&str>, w: &mut Worktree) {
    if w.prunable {
        return;
    }
    // In-process first; the CLI answers when that can't.
    let summary = super::native::summary(&w.path).ok().or_else(|| {
        let st = status(&w.path).ok()?;
        Some(super::native::Summary {
            files: st.files.len(),
            untracked: st
                .files
                .iter()
                .filter(|f| f.unstaged == Some(Change::Untracked))
                .map(|f| f.path.clone())
                .collect(),
            upstream: st.upstream.is_some().then_some((st.ahead, st.behind)),
        })
    });
    if let Some(s) = summary {
        w.changes = Some(s.files);
        w.upstream = s.upstream;
        // The lines (`+42 −7`) are `git diff`'s, and only asked for when something changed.
        let untracked: Vec<&str> = s.untracked.iter().map(String::as_str).collect();
        w.lines = if s.files == 0 { Some((0, 0)) } else { diff_stat(&w.path, &untracked).ok() };
    }
    if let (Some(base), Some(branch)) = (base, &w.branch)
        && !w.main
    {
        // `base...branch`: commits only on base are how far behind, only on branch how far ahead.
        w.ahead_behind = super::native::ahead_behind(root, base, branch)
            .ok()
            .or_else(|| {
                git(root, &["rev-list", "--left-right", "--count", &format!("{base}...{branch}")])
                    .ok()
                    .and_then(|s| parse_left_right(&s))
            })
            .map(|(behind, ahead)| (ahead, behind));
    }
}

/// Where a new worktree for `branch` goes by default: beside the repository,
/// as `<repo>-<branch>` (`/` in the branch name becomes `-`).
pub fn default_worktree_path(root: &Path, branch: &str) -> PathBuf {
    let repo = root.file_name().map_or_else(|| "repo".into(), |n| n.to_string_lossy().into_owned());
    let name = format!("{repo}-{}", branch.replace(['/', '\\'], "-"));
    root.parent().map_or_else(|| PathBuf::from(&name), |p| p.join(&name))
}

/// Adds a worktree at `path` on `branch`: the existing local branch, or a new
/// one from HEAD when there's none by that name.
pub fn add_worktree(root: &Path, path: &Path, branch: &str) -> Result<(), String> {
    add_worktree_from(root, path, branch, None)
}

/// [`add_worktree`], where a new branch starts from `base` (a branch, tag or
/// commit) instead of HEAD. `base` means nothing for a branch that exists.
pub fn add_worktree_from(root: &Path, path: &Path, branch: &str, base: Option<&str>) -> Result<(), String> {
    let branch = branch.trim();
    let path = path.to_string_lossy();
    if git(root, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{branch}")]).is_ok() {
        git(root, &["worktree", "add", &path, branch]).map(drop)
    } else {
        git(root, &["check-ref-format", "--branch", branch])
            .map_err(|_| format!("\"{branch}\" isn't a valid branch name"))?;
        let mut args = vec!["worktree", "add", "-b", branch, &*path];
        args.extend(base.filter(|b| !b.trim().is_empty()));
        git(root, &args).map(drop)
    }
}

/// Removes the worktree at `path`. Git refuses when it has changes, unless `force`.
pub fn remove_worktree(root: &Path, path: &Path, force: bool) -> Result<(), String> {
    let path = path.to_string_lossy();
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.push(&path);
    git(root, &args).map(drop)
}

/// Forgets worktrees whose folders are gone.
pub fn prune_worktrees(root: &Path) -> Result<(), String> {
    git(root, &["worktree", "prune"]).map(drop)
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

/// Whether tool call `name` with `input` (its JSON) works somewhere, rather than just
/// looking: file edits do, and shell commands unless every part of them only reads
/// (`cd x && git log`, `ls`, `cat`, `grep`…). Reads (Read, Grep, Glob) and everything else
/// don't. For [`worktree_in`]: a session that peeks into another worktree isn't working there.
pub fn works_in(name: &str, input: &str) -> bool {
    match name {
        "Edit" | "MultiEdit" | "Write" | "NotebookEdit" => true,
        "Bash" | "PowerShell" => {
            let command = serde_json::from_str::<serde_json::Value>(input)
                .ok()
                .and_then(|v| v.get("command").and_then(|c| c.as_str()).map(str::to_string))
                .unwrap_or_else(|| input.to_string());
            !only_reads(&command)
        }
        _ => false,
    }
}

/// Whether every part of shell `command` (split on `&&`, `||`, `;`, `|` and lines) is a
/// known read or a `cd`, with nothing redirected into a file.
fn only_reads(command: &str) -> bool {
    const READS: &[&str] = &[
        "cd",
        "set-location",
        "pushd",
        "popd",
        "ls",
        "dir",
        "cat",
        "type",
        "head",
        "tail",
        "less",
        "grep",
        "rg",
        "find",
        "wc",
        "echo",
        "pwd",
        "stat",
        "file",
        "which",
        "where",
        "get-content",
        "get-childitem",
        "gci",
        "select-string",
        "test-path",
        "get-item",
        "get-location",
        "select-object",
        "sort",
        "uniq",
        "tr",
        "cut",
        "jq",
        "true",
    ];
    const GIT_READS: &[&str] = &[
        "log",
        "reflog",
        "status",
        "diff",
        "show",
        "blame",
        "grep",
        "ls-files",
        "ls-tree",
        "rev-parse",
        "describe",
        "shortlog",
        "cat-file",
        "rev-list",
        "merge-base",
        "for-each-ref",
    ];
    // Output thrown away is fine; anything else redirected is a write.
    let mut command = command.to_string();
    for quiet in ["2>&1", "2>/dev/null", "2>$null", ">/dev/null", "> /dev/null", ">$null", "> $null"] {
        command = command.replace(quiet, " ");
    }
    if command.contains('>') {
        return false;
    }
    command.split(['\n', ';', '|', '&']).map(str::trim).filter(|p| !p.is_empty()).all(|part| {
        let mut words = part.split_whitespace().map(|w| w.trim_matches(['"', '\'', '(', ')']).to_lowercase());
        let Some(first) = words.next() else { return true };
        match first.as_str() {
            "git" => {
                // Past `-C <dir>` and other options, to the subcommand.
                let mut sub = None;
                while let Some(w) = words.next() {
                    // `-C <dir>` (lowercased here) and `-c <key=value>` take a value.
                    if w == "-c" {
                        words.next();
                    } else if !w.starts_with('-') {
                        sub = Some(w);
                        break;
                    }
                }
                match sub.as_deref() {
                    Some("worktree") | Some("remote") | Some("branch") | Some("stash") => {
                        matches!(
                            words.next().as_deref(),
                            Some("list") | Some("-v") | Some("-a") | Some("--list") | None
                        )
                    }
                    Some(s) => GIT_READS.contains(&s),
                    None => true,
                }
            }
            // `sed -n` prints; other sed may write in place.
            "sed" => words.any(|w| w == "-n"),
            w => READS.contains(&w),
        }
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
    fn edits_and_commands_that_change_things_work_in_a_folder_reads_dont() {
        let bash = |c: &str| works_in("Bash", &serde_json::json!({ "command": c }).to_string());
        // Looking around another worktree is no work there.
        assert!(!bash("cd /c/wt/main && git reflog -8 --date=iso; cd /c/repos/kynex && git reflog -5"));
        assert!(!bash("git -C ../wt log --oneline | head -5"));
        assert!(!bash("ls ../wt 2>/dev/null; cat ../wt/Cargo.toml | grep version"));
        assert!(!bash("git worktree list --porcelain"));
        assert!(!bash("sed -n 1,40p ../wt/src/main.rs"));
        let ps = serde_json::json!({ "command": r"Get-ChildItem ..\wt | Select-Object Name" }).to_string();
        assert!(!works_in("PowerShell", &ps));
        // Building, committing, switching or writing a file is.
        assert!(bash("cd ../wt && cargo check"));
        assert!(bash("cd ../wt && git commit -m x"));
        assert!(bash("git -C ../wt switch main"));
        assert!(bash("git branch new-feature"));
        assert!(bash("echo hi > ../wt/notes.txt"));
        assert!(bash("sed -i s/a/b/ ../wt/x.rs"));
        // Edits always; reads and other tools never.
        assert!(works_in("Edit", r#"{"file_path":"C:\\wt\\a.rs"}"#));
        assert!(works_in("Write", "{}"));
        assert!(!works_in("Read", r#"{"file_path":"C:\\wt\\a.rs"}"#));
        assert!(!works_in("Grep", r#"{"path":"C:\\wt"}"#));
        // A command that isn't JSON is read as it is.
        assert!(!works_in("Bash", "cd ../wt && git status"));
    }

    #[test]
    fn new_worktrees_go_beside_the_repo() {
        let root = Path::new("/repos/kynex");
        assert_eq!(default_worktree_path(root, "feat/tabs"), Path::new("/repos/kynex-feat-tabs"));
    }

    #[test]
    fn parses_left_right_counts() {
        assert_eq!(parse_left_right("3\t5\n"), Some((3, 5)));
        assert_eq!(parse_left_right("garbage"), None);
    }
}
