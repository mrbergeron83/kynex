//! `@file` mentions in the composer: finding the `@path` being typed, the
//! project's files and folders to complete it with, and ranking them as you
//! type (a fuzzy match that favours file names and word starts).

use std::collections::BTreeSet;
use std::ops::Range;
use std::path::Path;

/// Most entries listed outside git, where nothing says what to skip.
const WALK_LIMIT: usize = 20_000;

/// Folders the walk outside git never enters: build output and dependencies.
const SKIP: &[&str] = &["node_modules", "target", "dist", "build", "__pycache__", "venv"];

/// The `@token` the cursor is at the end of in `text`: its range, `@` included,
/// and what's typed after the `@`. Only at the start or after whitespace, so an
/// email address isn't one.
pub fn at_cursor(text: &str, cursor: usize) -> Option<(Range<usize>, &str)> {
    let before = text.get(..cursor)?;
    let start = before.char_indices().rev().find(|(_, c)| c.is_whitespace()).map_or(0, |(i, c)| i + c.len_utf8());
    let token = &before[start..];
    let query = token.strip_prefix('@')?;
    Some((start..cursor, query))
}

/// The range a completion replaces: the token from its `@` through any of it
/// after the cursor, so picking one mid-word swaps the whole word.
pub fn replace_range(text: &str, token: Range<usize>) -> Range<usize> {
    let rest = &text[token.end..];
    token.start..token.end + rest.find(char::is_whitespace).unwrap_or(rest.len())
}

/// What a picked entry puts in the message. A file is followed by a space (the
/// mention is done); a folder keeps its `/`, so the list goes on into it. A path
/// with spaces is quoted, as Claude Code writes it.
pub fn insert_text(path: &str) -> String {
    let quoted = if path.contains(char::is_whitespace) { format!("@\"{path}\"") } else { format!("@{path}") };
    if path.ends_with('/') { quoted } else { quoted + " " }
}

/// Every file under `root` relative to it with `/` separators: git's list in a
/// repository, else a walk that skips hidden and build folders and stops at
/// [`WALK_LIMIT`]. Blocks.
pub fn list(root: &Path) -> Vec<String> {
    crate::git::files(root).unwrap_or_else(|_| walk(root))
}

fn walk(root: &Path) -> Vec<String> {
    let mut found = Vec::new();
    let mut dirs = vec![(root.to_path_buf(), String::new())];
    while let Some((dir, prefix)) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            if found.len() >= WALK_LIMIT {
                return found;
            }
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let rel = format!("{prefix}{name}");
            match e.file_type() {
                Ok(t) if t.is_dir() => {
                    if !SKIP.contains(&name.as_str()) {
                        dirs.push((e.path(), rel + "/"));
                    }
                }
                Ok(_) => found.push(rel),
                Err(_) => {}
            }
        }
    }
    found
}

/// A project's files and the folders they're in (with a trailing `/`),
/// lowercased once for matching as you type.
#[derive(Default)]
pub struct Files {
    pub entries: Vec<String>,
    lower: Vec<String>,
}

/// One match: an index into [`Files::entries`] and the byte offsets of the
/// matched characters, to bold them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Match {
    pub index: usize,
    pub positions: Vec<usize>,
}

impl Files {
    /// From file paths: their folders are added, and all sorted.
    pub fn new(files: Vec<String>) -> Self {
        let mut all = BTreeSet::new();
        for f in files {
            let f = f.replace('\\', "/");
            for (i, _) in f.match_indices('/') {
                all.insert(f[..=i].to_string());
            }
            all.insert(f);
        }
        let entries: Vec<String> = all.into_iter().collect();
        let lower = entries.iter().map(|e| e.to_ascii_lowercase()).collect();
        Self { entries, lower }
    }

    /// The best `limit` entries for `query`, best first. A query naming a folder
    /// (`src/app/`, or `src/app/comp`) looks inside it only; with nothing after
    /// the `/` that's the folder's own entries. An empty query lists the top level.
    pub fn matching(&self, query: &str, limit: usize) -> Vec<Match> {
        let q = query.replace('\\', "/").to_ascii_lowercase();
        let (dir, rest) = match q.rfind('/') {
            Some(i) if self.lower.binary_search(&q[..=i].to_string()).is_ok() => q.split_at(i + 1),
            _ => ("", q.as_str()),
        };
        let mut found: Vec<(i64, usize, Vec<usize>)> = self
            .lower
            .iter()
            .enumerate()
            .filter_map(|(i, path)| {
                let inside = path.strip_prefix(dir)?;
                if inside.is_empty() {
                    return None;
                }
                if rest.is_empty() {
                    // The folder's own entries: files and folders right in it.
                    let child = inside.trim_end_matches('/');
                    return (!child.contains('/')).then(|| (-(inside.ends_with('/') as i64), i, Vec::new()));
                }
                let (score, positions) = score(inside, rest)?;
                Some((-score, i, positions.into_iter().map(|p| p + dir.len()).collect()))
            })
            .collect();
        found.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| self.entries[a.1].cmp(&self.entries[b.1])));
        found.into_iter().take(limit).map(|(_, index, positions)| Match { index, positions }).collect()
    }
}

/// How well `query` matches `path` (both lowercased), and where; `None` if its
/// characters don't all appear in order. Matches in the file name count most,
/// then ones at the start of a word or right after the previous one; a long
/// path and gaps cost a little.
fn score(path: &str, query: &str) -> Option<(i64, Vec<usize>)> {
    let trimmed = path.trim_end_matches('/');
    let name_start = trimmed.rfind('/').map_or(0, |i| i + 1);
    // Prefer a match inside the file name; else anywhere in the path.
    let (positions, in_name) = match find(path, query, name_start) {
        Some(p) => (p, true),
        None => (find(path, query, 0)?, false),
    };
    let bytes = path.as_bytes();
    let mut score = 0i64;
    let mut last: Option<usize> = None;
    for &p in &positions {
        let boundary = p == 0 || matches!(bytes[p - 1], b'/' | b'_' | b'-' | b'.' | b' ');
        if boundary {
            score += 10;
        }
        match last {
            Some(l) if p == l + 1 => score += 8,
            Some(l) => score -= (p - l - 1).min(10) as i64,
            None => {}
        }
        last = Some(p);
    }
    if in_name {
        score += 30;
        if positions.first() == Some(&name_start) {
            score += 20;
        }
    }
    // Folders rank under files that match as well, unless the query asks for one.
    if path.ends_with('/') && !query.ends_with('/') {
        score -= 5;
    }
    score -= path.len() as i64 / 8;
    Some((score, positions))
}

/// Byte offsets of `query`'s characters in `path` from `from` on, taken in
/// order. Each one jumps ahead to a word start where there is one (`ac` →
/// `app/composer`); if that leaves the rest unmatched, plain first occurrences.
fn find(path: &str, query: &str, from: usize) -> Option<Vec<usize>> {
    let chars: Vec<(usize, char)> = path[from..].char_indices().map(|(i, c)| (i + from, c)).collect();
    find_in(path, &chars, query, true).or_else(|| find_in(path, &chars, query, false))
}

fn find_in(path: &str, chars: &[(usize, char)], query: &str, jump: bool) -> Option<Vec<usize>> {
    let word_start = |i: usize| i == 0 || matches!(path.as_bytes()[i - 1], b'/' | b'_' | b'-' | b'.' | b' ');
    let mut positions: Vec<usize> = Vec::with_capacity(query.len());
    let mut at = 0;
    for q in query.chars() {
        let first = at + chars[at..].iter().position(|&(_, c)| c == q)?;
        // A match running straight on from the previous one stays put.
        let runs_on = !positions.is_empty() && at == first;
        let pick = if jump && !runs_on {
            chars[first..].iter().position(|&(i, c)| c == q && word_start(i)).map_or(first, |o| first + o)
        } else {
            first
        };
        positions.push(chars[pick].0);
        at = pick + 1;
    }
    Some(positions)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files() -> Files {
        Files::new(
            [
                "src/app/composer.rs",
                "src/ui/completions.rs",
                "crates/kynex-core/src/commands.rs",
                "src/app/mod.rs",
                "README.md",
                "Cargo.toml",
            ]
            .map(String::from)
            .to_vec(),
        )
    }

    fn names(f: &Files, q: &str) -> Vec<String> {
        f.matching(q, 50).into_iter().map(|m| f.entries[m.index].clone()).collect()
    }

    #[test]
    fn finds_the_token_at_the_cursor() {
        assert_eq!(at_cursor("@comp", 5), Some((0..5, "comp")));
        assert_eq!(at_cursor("look at @src/a", 14), Some((8..14, "src/a")));
        assert_eq!(at_cursor("look at @", 9), Some((8..9, "")));
        assert_eq!(at_cursor("line\n@x", 7), Some((5..7, "x")));
        assert_eq!(at_cursor("mail me@host", 12), None);
        assert_eq!(at_cursor("@done and more", 14), None);
        assert_eq!(at_cursor("é @ü", "é @ü".len()), Some((3..6, "ü")));
        assert_eq!(at_cursor("abc", 9), None);
    }

    #[test]
    fn replaces_the_whole_word() {
        assert_eq!(replace_range("@comp.rs and", 0..3), 0..8);
        assert_eq!(replace_range("@comp", 0..5), 0..5);
    }

    #[test]
    fn inserts_files_folders_and_spaces() {
        assert_eq!(insert_text("src/app/composer.rs"), "@src/app/composer.rs ");
        assert_eq!(insert_text("src/app/"), "@src/app/");
        assert_eq!(insert_text("my docs/a b.md"), "@\"my docs/a b.md\" ");
    }

    #[test]
    fn adds_folders() {
        let f = files();
        assert!(f.entries.contains(&"src/app/".to_string()));
        assert!(f.entries.contains(&"crates/kynex-core/src/".to_string()));
    }

    #[test]
    fn ranks_file_names_first() {
        let f = files();
        assert_eq!(names(&f, "comp")[..2], ["src/app/composer.rs", "src/ui/completions.rs"]);
        assert_eq!(names(&f, "COMPL")[0], "src/ui/completions.rs");
        assert_eq!(names(&f, "mod")[0], "src/app/mod.rs");
        assert_eq!(names(&f, "kcc")[0], "crates/kynex-core/src/commands.rs");
        assert!(names(&f, "zzz").is_empty());
        // Jumping to a word start would strand the rest: plain order still matches.
        let f = Files::new(vec!["xcb_c".into()]);
        assert_eq!(f.matching("cb", 1)[0].positions, [1, 2]);
    }

    #[test]
    fn looks_inside_a_named_folder() {
        let f = files();
        assert_eq!(names(&f, "src/app/"), ["src/app/composer.rs", "src/app/mod.rs"]);
        assert_eq!(names(&f, "src/"), ["src/app/", "src/ui/"]);
        assert_eq!(names(&f, "src/app/co"), ["src/app/composer.rs"]);
        // Not a folder: the slash is just part of the fuzzy query.
        assert_eq!(names(&f, "sa/com")[0], "src/app/composer.rs");
    }

    #[test]
    fn empty_query_lists_the_top_level() {
        assert_eq!(names(&files(), ""), ["crates/", "src/", "Cargo.toml", "README.md"]);
    }

    #[test]
    fn marks_matched_characters() {
        let f = files();
        let m = &f.matching("comp", 1)[0];
        assert_eq!(f.entries[m.index], "src/app/composer.rs");
        assert_eq!(m.positions, [8, 9, 10, 11]);
        let m = &f.matching("src/app/mo", 1)[0];
        assert_eq!(m.positions, [8, 9]);
    }

    #[test]
    fn walks_outside_git() {
        let dir = std::env::temp_dir().join(format!("kynex-mentions-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("a/node_modules")).unwrap();
        std::fs::create_dir_all(dir.join(".hidden")).unwrap();
        std::fs::write(dir.join("a/x.txt"), "").unwrap();
        std::fs::write(dir.join("a/node_modules/y.js"), "").unwrap();
        std::fs::write(dir.join(".hidden/z"), "").unwrap();
        assert_eq!(walk(&dir), ["a/x.txt"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn many_files_match_fast() {
        let many: Vec<String> = (0..10_000).map(|i| format!("dir{}/sub{}/file_{i}.rs", i % 50, i % 7)).collect();
        let f = Files::new(many);
        let start = std::time::Instant::now();
        let found = f.matching("file9", 50);
        assert!(!found.is_empty());
        assert!(start.elapsed() < std::time::Duration::from_millis(500));
    }

    #[test]
    fn survives_the_fuzz_checks() {
        crate::fuzz::mentions("@é\\x @\nsrc/é.rs\n\\back\\slash.rs\n/\n//a\nA/B/");
        crate::fuzz::mentions("a/\na/b\na/c/d");
    }
}
