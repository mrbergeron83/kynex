//! Unified diffs, parsed into files, hunks and numbered lines.

use std::path::Path;

use super::git;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Added,
    Removed,
    /// `\ No newline at end of file`.
    NoNewline,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DiffLine {
    pub kind: LineKind,
    /// Line number in the old file (context and removed lines).
    pub old: Option<u32>,
    /// Line number in the new file (context and added lines).
    pub new: Option<u32>,
    pub text: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Hunk {
    /// The `@@ -a,b +c,d @@ context` line.
    pub header: String,
    pub lines: Vec<DiffLine>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FileDiff {
    pub path: String,
    /// Set when the file was renamed or copied from somewhere else.
    pub old_path: Option<String>,
    pub new_file: bool,
    pub deleted: bool,
    pub binary: bool,
    pub hunks: Vec<Hunk>,
    pub added: usize,
    pub removed: usize,
}

/// One file's diff: its staged changes (index vs HEAD) or its unstaged ones
/// (working tree vs index). An untracked file reads as all added.
pub fn file_diff(root: &Path, path: &str, staged: bool, untracked: bool) -> Result<FileDiff, String> {
    if untracked {
        return Ok(untracked_diff(
            path,
            std::fs::read(root.join(path)).map_err(|e| format!("Couldn't read {path}: {e}"))?,
        ));
    }
    let mut args = vec!["diff", "--no-ext-diff", "-M"];
    if staged {
        args.push("--cached");
    }
    args.extend(["--", path]);
    let text = git(root, &args)?;
    Ok(parse(&text).into_iter().next().unwrap_or_else(|| FileDiff { path: path.to_string(), ..Default::default() }))
}

/// What a commit changed, against its first parent.
pub fn commit_diff(root: &Path, sha: &str) -> Result<Vec<FileDiff>, String> {
    git(root, &["show", "--format=", "--no-ext-diff", "-M", "--diff-merges=first-parent", sha]).map(|s| parse(&s))
}

/// Lines `(added, removed)` in the working tree against HEAD, staged or not,
/// counting each of `untracked` (repo-relative paths) as all added. Runs
/// `git diff --numstat` and reads the untracked files (the first 200); binary
/// files and ones over 1 MB count as no lines.
pub fn diff_stat(root: &Path, untracked: &[&str]) -> Result<(usize, usize), String> {
    let (added, removed) = parse_numstat(&git(root, &["diff", "HEAD", "--numstat", "--no-ext-diff", "--no-renames"])?);
    let new_lines: usize = untracked
        .iter()
        .take(200)
        .filter(|p| std::fs::metadata(root.join(p)).is_ok_and(|m| m.len() <= 1 << 20))
        .filter_map(|p| std::fs::read(root.join(p)).ok())
        .map(|bytes| untracked_diff("", bytes).added)
        .sum();
    Ok((added + new_lines, removed))
}

/// Sums `git diff --numstat`: `<added>\t<removed>\t<path>` per file, `-` for binary.
fn parse_numstat(s: &str) -> (usize, usize) {
    s.lines().fold((0, 0), |(added, removed), line| {
        let mut cols = line.split('\t').map(|n| n.parse::<usize>().unwrap_or(0));
        (added + cols.next().unwrap_or(0), removed + cols.next().unwrap_or(0))
    })
}

fn untracked_diff(path: &str, bytes: Vec<u8>) -> FileDiff {
    let mut d = FileDiff { path: path.to_string(), new_file: true, ..Default::default() };
    if bytes.contains(&0) {
        d.binary = true;
        return d;
    }
    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<DiffLine> = text
        .lines()
        .enumerate()
        .map(|(i, l)| DiffLine { kind: LineKind::Added, old: None, new: Some(i as u32 + 1), text: l.to_string() })
        .collect();
    d.added = lines.len();
    if !lines.is_empty() {
        d.hunks.push(Hunk { header: format!("@@ -0,0 +1,{} @@", lines.len()), lines });
    }
    d
}

/// Parses `git diff` / `git show` output (one or more files).
pub(super) fn parse(s: &str) -> Vec<FileDiff> {
    let mut files: Vec<FileDiff> = Vec::new();
    // Lines left in the current hunk, old side and new side.
    let (mut old_left, mut new_left) = (0u32, 0u32);
    let (mut old_no, mut new_no) = (0u32, 0u32);
    for line in s.lines() {
        let in_hunk = old_left > 0 || new_left > 0;
        if in_hunk && let Some(f) = files.last_mut() {
            let hunk = f.hunks.last_mut().expect("in a hunk");
            let (kind, text) = match line.chars().next() {
                Some('+') => (LineKind::Added, &line[1..]),
                Some('-') => (LineKind::Removed, &line[1..]),
                Some('\\') => (LineKind::NoNewline, line),
                Some(' ') => (LineKind::Context, &line[1..]),
                // Some tools strip the trailing space of empty context lines.
                None => (LineKind::Context, ""),
                _ => (LineKind::Context, line),
            };
            let (old, new) = match kind {
                LineKind::Added => {
                    new_left = new_left.saturating_sub(1);
                    new_no += 1;
                    f.added += 1;
                    (None, Some(new_no))
                }
                LineKind::Removed => {
                    old_left = old_left.saturating_sub(1);
                    old_no += 1;
                    f.removed += 1;
                    (Some(old_no), None)
                }
                LineKind::Context => {
                    old_left = old_left.saturating_sub(1);
                    new_left = new_left.saturating_sub(1);
                    old_no += 1;
                    new_no += 1;
                    (Some(old_no), Some(new_no))
                }
                LineKind::NoNewline => (None, None),
            };
            hunk.lines.push(DiffLine { kind, old, new, text: text.to_string() });
            continue;
        }
        if let Some(rest) = line.strip_prefix("diff --git ") {
            files.push(FileDiff { path: git_line_path(rest), ..Default::default() });
            continue;
        }
        // A `\ No newline` right after the hunk's last line.
        if line.starts_with('\\')
            && let Some(h) = files.last_mut().and_then(|f| f.hunks.last_mut())
        {
            h.lines.push(DiffLine { kind: LineKind::NoNewline, old: None, new: None, text: line.to_string() });
            continue;
        }
        let Some(f) = files.last_mut() else { continue };
        if let Some(header) = line.strip_prefix("@@ ") {
            let ranges = header.split(" @@").next().unwrap_or("");
            let mut parts = ranges.split_whitespace();
            let (o_start, o_len) = range(parts.next().unwrap_or("-0").trim_start_matches('-'));
            let (n_start, n_len) = range(parts.next().unwrap_or("+0").trim_start_matches('+'));
            (old_left, new_left) = (o_len, n_len);
            (old_no, new_no) = (o_start.saturating_sub(1), n_start.saturating_sub(1));
            f.hunks.push(Hunk { header: line.to_string(), lines: Vec::new() });
        } else if line.starts_with("new file mode") {
            f.new_file = true;
        } else if line.starts_with("deleted file mode") {
            f.deleted = true;
        } else if let Some(p) = line.strip_prefix("rename from ").or_else(|| line.strip_prefix("copy from ")) {
            f.old_path = Some(p.to_string());
        } else if let Some(p) = line.strip_prefix("rename to ").or_else(|| line.strip_prefix("copy to ")) {
            f.path = p.to_string();
        } else if let Some(p) = line.strip_prefix("+++ ") {
            if let Some(p) = p.strip_prefix("b/") {
                f.path = p.to_string();
            }
        } else if let Some(p) = line.strip_prefix("--- ") {
            if f.deleted
                && let Some(p) = p.strip_prefix("a/")
            {
                f.path = p.to_string();
            }
        } else if line.starts_with("Binary files ") || line == "GIT binary patch" {
            f.binary = true;
        }
    }
    files
}

/// `12,5` → (12, 5); a bare `12` means one line.
fn range(s: &str) -> (u32, u32) {
    match s.split_once(',') {
        Some((start, len)) => (start.parse().unwrap_or(0), len.parse().unwrap_or(0)),
        None => (s.parse().unwrap_or(0), 1),
    }
}

/// The path from `a/x b/x` in a `diff --git` line; the `+++`, `rename` and
/// `---` lines that follow override it where present.
fn git_line_path(rest: &str) -> String {
    let half = rest.len() / 2;
    let b = &rest[half..];
    b.strip_prefix(" b/").or_else(|| rest.rsplit_once(" b/").map(|(_, p)| p)).unwrap_or(rest).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIFF: &str = "diff --git a/src/lib.rs b/src/lib.rs
index 1111111..2222222 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,4 +1,4 @@ mod a;
 use x;
-fn old() {}
+fn new() {}
+fn more() {}

--- not a header, a removed line starting with dashes
@@ -10 +11 @@
-a
+b
\\ No newline at end of file
diff --git a/old name.rs b/new name.rs
similarity index 90%
rename from old name.rs
rename to new name.rs
diff --git a/gone.txt b/gone.txt
deleted file mode 100644
index 3333333..0000000
--- a/gone.txt
+++ /dev/null
@@ -1 +0,0 @@
-bye
diff --git a/logo.png b/logo.png
new file mode 100644
Binary files /dev/null and b/logo.png differ
";

    #[test]
    fn parses_files_hunks_and_line_numbers() {
        let files = parse(DIFF);
        assert_eq!(files.len(), 4);
        let f = &files[0];
        assert_eq!(f.path, "src/lib.rs");
        assert_eq!(f.hunks.len(), 2);
        let lines = &f.hunks[0].lines;
        assert_eq!(lines[0], DiffLine { kind: LineKind::Context, old: Some(1), new: Some(1), text: "use x;".into() });
        assert_eq!(lines[1].kind, LineKind::Removed);
        assert_eq!((lines[1].old, lines[1].new), (Some(2), None));
        assert_eq!((lines[3].old, lines[3].new), (None, Some(3)));
        assert_eq!(lines[5].kind, LineKind::Removed, "a `---` line inside a hunk is content");
        assert_eq!(f.hunks[1].lines[0].old, Some(10));
        assert_eq!(f.hunks[1].lines[1].new, Some(11));
        assert_eq!(f.hunks[1].lines[2].kind, LineKind::NoNewline);
        assert_eq!((f.added, f.removed), (3, 3));

        assert_eq!(files[1].path, "new name.rs");
        assert_eq!(files[1].old_path.as_deref(), Some("old name.rs"));
        assert!(files[1].hunks.is_empty());

        assert_eq!(files[2].path, "gone.txt");
        assert!(files[2].deleted);
        assert_eq!(files[2].removed, 1);

        assert!(files[3].binary && files[3].new_file);
        assert_eq!(files[3].path, "logo.png");
    }

    #[test]
    fn untracked_files_read_as_added() {
        let d = untracked_diff("a.txt", b"one\ntwo\n".to_vec());
        assert_eq!(d.added, 2);
        assert_eq!(d.hunks[0].lines[1].new, Some(2));
        assert!(untracked_diff("b.bin", vec![0, 1, 2]).binary);
    }

    #[test]
    fn sums_numstat() {
        assert_eq!(parse_numstat("3\t1\tsrc/a.rs\n-\t-\tlogo.png\n10\t0\tnew file.rs\n"), (13, 1));
        assert_eq!(parse_numstat(""), (0, 0));
    }

    #[test]
    fn stats_this_repo() {
        assert!(diff_stat(Path::new(env!("CARGO_MANIFEST_DIR")), &[]).is_ok());
    }
}
