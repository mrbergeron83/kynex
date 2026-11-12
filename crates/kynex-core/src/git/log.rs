//! History: commits with their branch and tag labels, and one commit in full.

use std::path::Path;

use super::diff::{FileDiff, commit_diff};
use super::git;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefKind {
    /// The checked-out branch (`HEAD -> main`), or a bare `HEAD` when detached.
    Head,
    Branch,
    Remote,
    Tag,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Ref {
    pub name: String,
    pub kind: RefKind,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Commit {
    pub sha: String,
    pub short: String,
    pub parents: Vec<String>,
    pub author: String,
    /// Author time, seconds since the Unix epoch.
    pub time: i64,
    pub subject: String,
    pub refs: Vec<Ref>,
}

/// One commit in full: its whole message and what it changed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CommitDetail {
    pub commit: Commit,
    pub email: String,
    /// The message after the subject line, trimmed.
    pub body: String,
    pub files: Vec<FileDiff>,
}

const FORMAT: &str = "--format=%H%x1f%h%x1f%P%x1f%an%x1f%at%x1f%D%x1f%s%x1f%ae%x1f%b%x1e";

/// The newest `limit` commits reachable from HEAD. Empty in a repo with no commits yet.
pub fn log(root: &Path, limit: usize) -> Result<Vec<Commit>, String> {
    let n = format!("-n{limit}");
    match git(root, &["log", &n, FORMAT]) {
        Ok(s) => Ok(parse(&s).into_iter().map(|d| d.commit).collect()),
        Err(e) if e.contains("does not have any commits") => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

pub fn commit_detail(root: &Path, sha: &str) -> Result<CommitDetail, String> {
    let s = git(root, &["log", "-n1", FORMAT, sha])?;
    let mut detail = parse(&s).into_iter().next().ok_or_else(|| format!("No commit {sha}"))?;
    detail.files = commit_diff(root, sha)?;
    Ok(detail)
}

pub(crate) fn parse(s: &str) -> Vec<CommitDetail> {
    s.split('\x1e')
        .filter_map(|rec| {
            let rec = rec.trim_start_matches('\n');
            let f: Vec<&str> = rec.split('\x1f').collect();
            if f.len() < 9 {
                return None;
            }
            Some(CommitDetail {
                commit: Commit {
                    sha: f[0].to_string(),
                    short: f[1].to_string(),
                    parents: f[2].split_whitespace().map(str::to_string).collect(),
                    author: f[3].to_string(),
                    time: f[4].parse().unwrap_or(0),
                    refs: parse_refs(f[5]),
                    subject: f[6].to_string(),
                },
                email: f[7].to_string(),
                body: f[8].trim().to_string(),
                files: Vec::new(),
            })
        })
        .collect()
}

/// Parses `%D`: `HEAD -> main, origin/main, origin/HEAD, tag: v1.0`.
pub(crate) fn parse_refs(s: &str) -> Vec<Ref> {
    let mut refs = Vec::new();
    for part in s.split(", ").filter(|p| !p.is_empty()) {
        let (name, kind) = if let Some(b) = part.strip_prefix("HEAD -> ") {
            (b, RefKind::Head)
        } else if let Some(t) = part.strip_prefix("tag: ") {
            (t, RefKind::Tag)
        } else if part == "HEAD" {
            (part, RefKind::Head)
        } else if part.ends_with("/HEAD") {
            continue;
        } else if part.contains('/') {
            // Local branch names can have slashes too; `git log` lists the
            // checked-out and local ones first, so take `x/y` after a local
            // `y` as a remote. Good enough for labels.
            (part, RefKind::Remote)
        } else {
            (part, RefKind::Branch)
        };
        refs.push(Ref { name: name.to_string(), kind });
    }
    refs
}

/// How many commits a history list reads at a time.
pub const HISTORY_PAGE: usize = 500;

/// How many commits to ask [`log`] for next as a history list scrolls: a page more once the
/// rows in sight reach within a quarter page of the end of the `loaded` ones, if the last
/// read (of `limit`) came back full, so there may be more. `None`: nothing more to read yet.
pub fn more_history(loaded: usize, limit: usize, seen_to: usize) -> Option<usize> {
    (loaded >= limit && seen_to + HISTORY_PAGE / 4 >= loaded).then_some(limit + HISTORY_PAGE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn more_history_is_read_near_the_end_of_a_full_page() {
        let p = HISTORY_PAGE;
        assert_eq!(more_history(p, p, 10), None, "far from the end");
        assert_eq!(more_history(p, p, p - 5), Some(2 * p));
        assert_eq!(more_history(2 * p, 2 * p, 2 * p), Some(3 * p));
        assert_eq!(more_history(120, p, 119), None, "the whole history is read");
        assert_eq!(more_history(0, p, 0), None, "no commits");
    }

    #[test]
    fn parses_records() {
        let s = "aaaa\x1faa\x1fpp qq\x1fMe\x1f1700000000\x1fHEAD -> main, origin/main, origin/HEAD, tag: v1\x1fSubject line\x1fme@x.com\x1fBody text\n\nmore\n\x1e\n\
                 bbbb\x1fbb\x1f\x1fYou\x1f1600000000\x1f\x1fFirst\x1fyou@x.com\x1f\x1e\n";
        let list = parse(s);
        assert_eq!(list.len(), 2);
        let c = &list[0].commit;
        assert_eq!(c.parents, ["pp", "qq"]);
        assert_eq!(c.time, 1_700_000_000);
        assert_eq!(c.subject, "Subject line");
        assert_eq!(
            c.refs,
            [
                Ref { name: "main".into(), kind: RefKind::Head },
                Ref { name: "origin/main".into(), kind: RefKind::Remote },
                Ref { name: "v1".into(), kind: RefKind::Tag },
            ]
        );
        assert_eq!(list[0].body, "Body text\n\nmore");
        assert!(list[1].commit.parents.is_empty() && list[1].commit.refs.is_empty());
    }

    #[test]
    fn reads_this_repo() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let commits = log(root, 5).unwrap();
        assert!(!commits.is_empty());
        let detail = commit_detail(root, &commits[0].sha).unwrap();
        assert_eq!(detail.commit.sha, commits[0].sha);
    }
}
