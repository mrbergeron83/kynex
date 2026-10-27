//! `git status`: the branch, its upstream, and every changed file, split into
//! what's staged and what isn't.

use std::path::Path;

use super::git;

/// What happened to a file, on one side (index or working tree).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    Modified,
    Added,
    Deleted,
    Renamed,
    Copied,
    TypeChanged,
    Untracked,
    Conflicted,
}

impl Change {
    fn from_code(c: char) -> Option<Self> {
        Some(match c {
            'M' => Self::Modified,
            'A' => Self::Added,
            'D' => Self::Deleted,
            'R' => Self::Renamed,
            'C' => Self::Copied,
            'T' => Self::TypeChanged,
            _ => return None,
        })
    }

    /// One letter, as `git status --short` would show it.
    pub fn letter(self) -> char {
        match self {
            Self::Modified => 'M',
            Self::Added => 'A',
            Self::Deleted => 'D',
            Self::Renamed => 'R',
            Self::Copied => 'C',
            Self::TypeChanged => 'T',
            Self::Untracked => 'U',
            Self::Conflicted => '!',
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            Self::Modified => "Modified",
            Self::Added => "Added",
            Self::Deleted => "Deleted",
            Self::Renamed => "Renamed",
            Self::Copied => "Copied",
            Self::TypeChanged => "Type changed",
            Self::Untracked => "New, untracked",
            Self::Conflicted => "Conflict: resolve it, then stage it",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct FileStatus {
    /// Repo-relative, with `/` separators.
    pub path: String,
    /// Where a rename or copy came from.
    pub orig_path: Option<String>,
    /// The change in the index (what the next commit gets).
    pub staged: Option<Change>,
    /// The change in the working tree that isn't staged.
    pub unstaged: Option<Change>,
}

/// Where HEAD points.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Head {
    Branch(String),
    /// A commit, not on a branch; the short hash.
    Detached(String),
    /// A branch with no commits yet (a fresh repo).
    Unborn(String),
}

impl Default for Head {
    fn default() -> Self {
        Head::Detached(String::new())
    }
}

impl Head {
    pub fn branch(&self) -> Option<&str> {
        match self {
            Head::Branch(b) | Head::Unborn(b) => Some(b),
            Head::Detached(_) => None,
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Head::Branch(b) | Head::Unborn(b) | Head::Detached(b) => b,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Status {
    pub head: Head,
    /// The full hash of HEAD; empty before the first commit.
    pub oid: String,
    /// e.g. `origin/main`.
    pub upstream: Option<String>,
    pub ahead: usize,
    pub behind: usize,
    pub files: Vec<FileStatus>,
}

impl Status {
    pub fn staged(&self) -> impl Iterator<Item = &FileStatus> {
        self.files.iter().filter(|f| f.staged.is_some())
    }

    pub fn unstaged(&self) -> impl Iterator<Item = &FileStatus> {
        self.files.iter().filter(|f| f.unstaged.is_some())
    }

    pub fn has_conflicts(&self) -> bool {
        self.files.iter().any(|f| f.unstaged == Some(Change::Conflicted))
    }
}

pub fn status(root: &Path) -> Result<Status, String> {
    git(root, &["status", "--porcelain=v2", "--branch", "-z", "--untracked-files=all"]).map(|s| parse(&s))
}

/// Parses `git status --porcelain=v2 --branch -z`.
pub(crate) fn parse(s: &str) -> Status {
    let mut st = Status::default();
    let mut head = None;
    let mut entries = s.split('\0');
    while let Some(entry) = entries.next() {
        if let Some(header) = entry.strip_prefix("# ") {
            let (key, value) = header.split_once(' ').unwrap_or((header, ""));
            match key {
                "branch.oid" => st.oid = if value == "(initial)" { String::new() } else { value.to_string() },
                "branch.head" => head = Some(value.to_string()),
                "branch.upstream" => st.upstream = Some(value.to_string()),
                "branch.ab" => {
                    for part in value.split_whitespace() {
                        if let Some(n) = part.strip_prefix('+') {
                            st.ahead = n.parse().unwrap_or(0);
                        } else if let Some(n) = part.strip_prefix('-') {
                            st.behind = n.parse().unwrap_or(0);
                        }
                    }
                }
                _ => {}
            }
            continue;
        }
        let kind = entry.chars().next();
        let file = match kind {
            Some('1') => entry.splitn(9, ' ').collect::<Vec<_>>().get(8).map(|path| (entry, *path, None)),
            Some('2') => {
                let orig = entries.next().map(str::to_string);
                entry.splitn(10, ' ').collect::<Vec<_>>().get(9).map(|path| (entry, *path, orig))
            }
            Some('u') => {
                if let Some(path) = entry.splitn(11, ' ').nth(10) {
                    st.files.push(FileStatus {
                        path: path.to_string(),
                        orig_path: None,
                        staged: None,
                        unstaged: Some(Change::Conflicted),
                    });
                }
                None
            }
            Some('?') => {
                if let Some(path) = entry.get(2..) {
                    st.files.push(FileStatus {
                        path: path.to_string(),
                        orig_path: None,
                        staged: None,
                        unstaged: Some(Change::Untracked),
                    });
                }
                None
            }
            _ => None,
        };
        if let Some((entry, path, orig_path)) = file {
            let mut xy = entry.get(2..4).unwrap_or("").chars();
            st.files.push(FileStatus {
                path: path.to_string(),
                orig_path,
                staged: xy.next().and_then(Change::from_code),
                unstaged: xy.next().and_then(Change::from_code),
            });
        }
    }
    st.head = match head.as_deref() {
        Some("(detached)") | None => Head::Detached(st.oid.chars().take(7).collect()),
        Some(name) if st.oid.is_empty() => Head::Unborn(name.to_string()),
        Some(name) => Head::Branch(name.to_string()),
    };
    st.files.sort_by(|a, b| a.path.cmp(&b.path));
    st
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "# branch.oid cc450cc644cecaa5f6871f1fae3b84102351caaf\0\
        # branch.head main\0\
        # branch.upstream origin/main\0\
        # branch.ab +2 -1\0\
        1 .M N... 100644 100644 100644 aaa bbb src/app/mod.rs\0\
        1 A. N... 000000 100644 100644 000 ccc new file.rs\0\
        2 R. N... 100644 100644 100644 ddd ddd R100 crates/b.rs\0crates/a.rs\0\
        u UU N... 100644 100644 100644 100644 e1 e2 e3 conflict.rs\0\
        ? notes/todo.md\0";

    #[test]
    fn parses_branch_and_files() {
        let st = parse(SAMPLE);
        assert_eq!(st.head, Head::Branch("main".into()));
        assert_eq!(st.upstream.as_deref(), Some("origin/main"));
        assert_eq!((st.ahead, st.behind), (2, 1));
        let by = |p: &str| st.files.iter().find(|f| f.path == p).unwrap().clone();
        assert_eq!(by("src/app/mod.rs").unstaged, Some(Change::Modified));
        assert_eq!(by("src/app/mod.rs").staged, None);
        assert_eq!(by("new file.rs").staged, Some(Change::Added));
        assert_eq!(by("crates/b.rs").orig_path.as_deref(), Some("crates/a.rs"));
        assert_eq!(by("crates/b.rs").staged, Some(Change::Renamed));
        assert_eq!(by("conflict.rs").unstaged, Some(Change::Conflicted));
        assert_eq!(by("notes/todo.md").unstaged, Some(Change::Untracked));
        assert_eq!(st.staged().count(), 2);
        assert_eq!(st.unstaged().count(), 3);
        assert!(st.has_conflicts());
    }

    #[test]
    fn detached_and_unborn_heads() {
        let st = parse("# branch.oid 1234567890abcdef\0# branch.head (detached)\0");
        assert_eq!(st.head, Head::Detached("1234567".into()));
        let st = parse("# branch.oid (initial)\0# branch.head main\0");
        assert_eq!(st.head, Head::Unborn("main".into()));
        assert_eq!(st.head.branch(), Some("main"));
    }

    /// Found by fuzzing: short entries and multi-byte characters where the
    /// status codes should be.
    #[test]
    fn odd_entries_dont_panic() {
        assert!(parse("?\0").files.is_empty());
        let st = parse("1 aé b c d e f g path\0");
        assert_eq!(st.files[0].path, "path");
    }

    #[test]
    fn reads_this_repo() {
        let st = status(Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap();
        assert!(!st.oid.is_empty());
    }
}
