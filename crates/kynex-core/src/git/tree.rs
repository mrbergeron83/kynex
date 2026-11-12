//! Changed files as a collapsible folder tree: one status per file, line counts, and
//! the rows to draw for the folders that are open.

use std::collections::{HashMap, HashSet};

use super::status::{Change, Status};

/// What happened to a file, staged and unstaged together.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TreeStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
    Untracked,
    Conflicted,
}

/// One changed file in the tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeFile {
    /// Repo-relative, with `/` separators.
    pub path: String,
    /// Where a rename came from.
    pub orig_path: Option<String>,
    pub status: TreeStatus,
    pub added: usize,
    pub removed: usize,
}

/// A row of the tree, top to bottom.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TreeRow {
    /// A folder (or a chain of single-child folders, `src/app`). `path` is the deepest
    /// folder's full path, which is what `closed` holds.
    Folder { path: String, name: String, depth: usize, open: bool, files: usize, added: usize, removed: usize },
    /// `files[index]`, shown under its folder's `depth`.
    File { index: usize, name: String, depth: usize },
}

/// The changed files of `status` with their line counts from `stats` (see
/// [`super::file_stats`]; missing means 0), one entry per file, in status order.
///
/// A file's status is the first that fits, over its staged and unstaged changes:
/// conflicted, untracked, renamed or copied, deleted, added, else modified.
pub fn tree_files(status: &Status, stats: &HashMap<String, (usize, usize)>) -> Vec<TreeFile> {
    status
        .files
        .iter()
        .map(|f| {
            let has = |c: &[Change]| [f.staged, f.unstaged].iter().flatten().any(|x| c.contains(x));
            let kind = if has(&[Change::Conflicted]) {
                TreeStatus::Conflicted
            } else if has(&[Change::Untracked]) {
                TreeStatus::Untracked
            } else if has(&[Change::Renamed, Change::Copied]) {
                TreeStatus::Renamed
            } else if has(&[Change::Deleted]) {
                TreeStatus::Deleted
            } else if has(&[Change::Added]) {
                TreeStatus::Added
            } else {
                TreeStatus::Modified
            };
            let (added, removed) = stats.get(&f.path).copied().unwrap_or((0, 0));
            TreeFile { path: f.path.clone(), orig_path: f.orig_path.clone(), status: kind, added, removed }
        })
        .collect()
}

#[derive(Default)]
struct Node {
    name: String,
    path: String,
    folders: Vec<Node>,
    files: Vec<usize>,
}

impl Node {
    /// Files, added and removed lines below this folder.
    fn totals(&self, files: &[TreeFile]) -> (usize, usize, usize) {
        let mut t = (self.files.len(), 0, 0);
        for &i in &self.files {
            t.1 += files[i].added;
            t.2 += files[i].removed;
        }
        for f in &self.folders {
            let s = f.totals(files);
            (t.0, t.1, t.2) = (t.0 + s.0, t.1 + s.1, t.2 + s.2);
        }
        t
    }
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn sort_key(name: &str) -> (String, &str) {
    (name.to_lowercase(), name)
}

/// The rows for `files` as a tree: folders before files at each level, both sorted
/// without regard to case. A folder whose full path is in `closed` shows but hides what
/// is below it. Folder totals count every file below. A folder holding only one folder
/// joins it into one row (`src/app`), whose path is the deepest one's.
pub fn tree_rows(files: &[TreeFile], closed: &HashSet<String>) -> Vec<TreeRow> {
    let mut root = Node::default();
    for (i, f) in files.iter().enumerate() {
        let parts: Vec<&str> = f.path.split('/').collect();
        let mut node = &mut root;
        let mut path = String::new();
        for part in &parts[..parts.len() - 1] {
            if !path.is_empty() {
                path.push('/');
            }
            path.push_str(part);
            let at = match node.folders.iter().position(|n| n.name == *part) {
                Some(at) => at,
                None => {
                    node.folders.push(Node { name: part.to_string(), path: path.clone(), ..Default::default() });
                    node.folders.len() - 1
                }
            };
            node = &mut node.folders[at];
        }
        node.files.push(i);
    }
    let mut rows = Vec::new();
    emit(&root, files, closed, 0, &mut rows);
    rows
}

fn emit(node: &Node, files: &[TreeFile], closed: &HashSet<String>, depth: usize, rows: &mut Vec<TreeRow>) {
    let mut folders: Vec<&Node> = node.folders.iter().collect();
    folders.sort_by_key(|f| sort_key(&f.name));
    for folder in folders {
        let (mut deepest, mut name) = (folder, folder.name.clone());
        while deepest.files.is_empty() && deepest.folders.len() == 1 {
            deepest = &deepest.folders[0];
            name = format!("{name}/{}", deepest.name);
        }
        let (count, added, removed) = deepest.totals(files);
        let open = !closed.contains(&deepest.path);
        rows.push(TreeRow::Folder { path: deepest.path.clone(), name, depth, open, files: count, added, removed });
        if open {
            emit(deepest, files, closed, depth + 1, rows);
        }
    }
    let mut own: Vec<usize> = node.files.clone();
    own.sort_by_key(|&i| sort_key(file_name(&files[i].path)));
    for index in own {
        rows.push(TreeRow::File { index, name: file_name(&files[index].path).to_string(), depth });
    }
}

#[cfg(test)]
mod tests {
    use super::super::status::FileStatus;
    use super::*;

    fn file(path: &str, added: usize, removed: usize) -> TreeFile {
        TreeFile { path: path.into(), orig_path: None, status: TreeStatus::Modified, added, removed }
    }

    fn fs(path: &str, staged: Option<Change>, unstaged: Option<Change>) -> FileStatus {
        FileStatus { path: path.into(), orig_path: None, staged, unstaged }
    }

    fn names(rows: &[TreeRow]) -> Vec<String> {
        rows.iter()
            .map(|r| match r {
                TreeRow::Folder { name, depth, open, .. } => {
                    format!("{}{name}/{}", " ".repeat(*depth), if *open { "" } else { " (closed)" })
                }
                TreeRow::File { name, depth, .. } => format!("{}{name}", " ".repeat(*depth)),
            })
            .collect()
    }

    #[test]
    fn maps_statuses_with_precedence() {
        use Change::*;
        let status = Status {
            files: vec![
                fs("a", Some(Modified), None),
                fs("b", Some(Added), Some(Modified)),
                fs("c", None, Some(Deleted)),
                fs("d", Some(Renamed), Some(Modified)),
                fs("e", None, Some(Untracked)),
                fs("f", None, Some(Conflicted)),
                fs("g", None, Some(TypeChanged)),
                fs("h", Some(Added), Some(Deleted)),
            ],
            ..Default::default()
        };
        let stats = HashMap::from([("a".to_string(), (3, 1))]);
        let got = tree_files(&status, &stats);
        use TreeStatus as T;
        let kinds: Vec<T> = got.iter().map(|f| f.status).collect();
        assert_eq!(
            kinds,
            [T::Modified, T::Added, T::Deleted, T::Renamed, T::Untracked, T::Conflicted, T::Modified, T::Deleted]
        );
        assert_eq!((got[0].added, got[0].removed), (3, 1));
        assert_eq!((got[1].added, got[1].removed), (0, 0));
    }

    #[test]
    fn keeps_the_rename_source() {
        let mut f = fs("new.rs", Some(Change::Renamed), None);
        f.orig_path = Some("old.rs".into());
        let got = tree_files(&Status { files: vec![f], ..Default::default() }, &HashMap::new());
        assert_eq!(got[0].orig_path.as_deref(), Some("old.rs"));
    }

    #[test]
    fn nests_and_sorts_folders_before_files() {
        let files = [
            file("zeta.rs", 1, 0),
            file("src/b.rs", 1, 0),
            file("Src/x/a.rs", 1, 0),
            file("alpha/B.rs", 1, 0),
            file("alpha/a.rs", 1, 0),
        ];
        let rows = tree_rows(&files, &HashSet::new());
        assert_eq!(names(&rows), ["alpha/", " a.rs", " B.rs", "Src/x/", " a.rs", "src/", " b.rs", "zeta.rs"]);
        assert!(matches!(rows[1], TreeRow::File { index: 4, depth: 1, .. }));
    }

    #[test]
    fn closed_folders_hide_children() {
        let files = [file("a/b.rs", 1, 0), file("a/c/d.rs", 2, 0), file("e.rs", 0, 1)];
        let closed = HashSet::from(["a".to_string()]);
        let rows = tree_rows(&files, &closed);
        assert_eq!(names(&rows), ["a/ (closed)", "e.rs"]);
        let open = tree_rows(&files, &HashSet::new());
        assert_eq!(names(&open), ["a/", " c/", "  d.rs", " b.rs", "e.rs"]);
    }

    #[test]
    fn totals_sum_everything_below() {
        let files = [file("a/b.rs", 1, 2), file("a/c/d.rs", 10, 20), file("e.rs", 100, 200)];
        let rows = tree_rows(&files, &HashSet::new());
        assert_eq!(
            rows[0],
            TreeRow::Folder {
                path: "a".into(),
                name: "a".into(),
                depth: 0,
                open: true,
                files: 2,
                added: 11,
                removed: 22
            }
        );
        assert!(matches!(&rows[1], TreeRow::Folder { path, files: 1, added: 10, removed: 20, .. } if path == "a/c"));
    }

    #[test]
    fn compacts_single_child_chains() {
        let files = [file("src/app/ui/a.rs", 1, 0), file("src/app/ui/b.rs", 2, 0), file("docs/x/y.md", 1, 1)];
        let rows = tree_rows(&files, &HashSet::new());
        assert_eq!(names(&rows), ["docs/x/", " y.md", "src/app/ui/", " a.rs", " b.rs"]);
        assert!(
            matches!(&rows[2], TreeRow::Folder { path, name, files: 2, added: 3, .. } if path == "src/app/ui" && name == "src/app/ui")
        );
        // Closing is by the deepest folder's path.
        let closed = HashSet::from(["src/app/ui".to_string()]);
        assert_eq!(names(&tree_rows(&files, &closed)), ["docs/x/", " y.md", "src/app/ui/ (closed)"]);
    }

    #[test]
    fn a_folder_with_files_and_a_folder_is_not_compacted() {
        let files = [file("src/main.rs", 1, 0), file("src/app/a.rs", 1, 0)];
        let rows = tree_rows(&files, &HashSet::new());
        assert_eq!(names(&rows), ["src/", " app/", "  a.rs", " main.rs"]);
    }

    #[test]
    fn empty_list_has_no_rows() {
        assert!(tree_rows(&[], &HashSet::new()).is_empty());
    }
}
