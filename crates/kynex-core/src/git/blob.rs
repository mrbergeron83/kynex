//! A file's bytes as git keeps them: at HEAD or in the index, for showing the old side
//! of a change that isn't text (an image's before).

use std::path::Path;

use super::{command_with, failure, read_program};

/// A file's content from git.
#[derive(Debug, PartialEq, Eq)]
pub enum Blob {
    /// Not there: a new file has no HEAD side, a deleted one no index side.
    Missing,
    /// Bigger than the limit asked for, at this many bytes.
    TooLarge(u64),
    Bytes(Vec<u8>),
}

/// Where a side of a file comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rev {
    Head,
    Index,
}

impl Rev {
    fn spec(self, path: &str) -> String {
        match self {
            Rev::Head => format!("HEAD:{path}"),
            Rev::Index => format!(":{path}"),
        }
    }
}

/// `path` (repo-relative) at `rev`, if it's at most `max` bytes. Blocking.
pub fn blob(root: &Path, rev: Rev, path: &str, max: u64) -> Result<Blob, String> {
    let spec = rev.spec(path);
    let size = match run(root, &["cat-file", "-s", &spec]) {
        Ok(size) => size,
        Err(e) if is_missing(&e) => return Ok(Blob::Missing),
        Err(e) => return Err(e),
    };
    let size: u64 = String::from_utf8_lossy(&size).trim().parse().map_err(|_| "git gave no size".to_string())?;
    if size > max {
        return Ok(Blob::TooLarge(size));
    }
    // `cat-file blob` gives the bytes as stored: no filters, no textconv.
    run(root, &["cat-file", "blob", &spec]).map(Blob::Bytes)
}

/// Whether git's error says the path isn't at that revision (or there's no HEAD yet),
/// rather than that something went wrong.
fn is_missing(err: &str) -> bool {
    let e = err.to_ascii_lowercase();
    ["does not exist", "not in the index", "invalid object name", "not a valid object name", "bad revision"]
        .iter()
        .any(|m| e.contains(m))
}

fn run(root: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let out = command_with(read_program(), root).args(args).output().map_err(|e| format!("Couldn't run git: {e}"))?;
    if out.status.success() { Ok(out.stdout) } else { Err(failure(&out.stderr, &out.stdout)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_head_and_index_sides() {
        let root = std::env::temp_dir().join(format!("kynex-blob-{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        std::fs::create_dir_all(&root).unwrap();
        let git = |args: &[&str]| {
            let ok = std::process::Command::new("git").arg("-C").arg(&root).args(args).output().unwrap();
            assert!(ok.status.success(), "{args:?}: {}", String::from_utf8_lossy(&ok.stderr));
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@example.com"]);
        git(&["config", "user.name", "t"]);
        std::fs::write(root.join("a.bin"), b"\0old").unwrap();
        git(&["add", "a.bin"]);
        git(&["commit", "-qm", "x"]);
        std::fs::write(root.join("a.bin"), b"\0staged").unwrap();
        git(&["add", "a.bin"]);
        assert_eq!(blob(&root, Rev::Head, "a.bin", 100).unwrap(), Blob::Bytes(b"\0old".to_vec()));
        assert_eq!(blob(&root, Rev::Index, "a.bin", 100).unwrap(), Blob::Bytes(b"\0staged".to_vec()));
        assert_eq!(blob(&root, Rev::Head, "a.bin", 2).unwrap(), Blob::TooLarge(4));
        assert_eq!(blob(&root, Rev::Head, "nope.png", 100).unwrap(), Blob::Missing);
        assert_eq!(blob(&root, Rev::Index, "nope.png", 100).unwrap(), Blob::Missing);
        assert!(
            blob(&root.join("not-a-repo-here"), Rev::Head, "a.bin", 100).is_err(),
            "a failure isn't a missing file"
        );
        std::fs::remove_dir_all(&root).ok();
    }
}
