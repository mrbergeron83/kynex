//! File paths in Claude's text (prose, inline code, tool row summaries), for
//! clickable links: `src/app/mod.rs:123`, `C:\x\y.rs:12:5`, `main.rs#L12`.
//! `find` only looks at the text; the app checks `exists` off the UI thread
//! before showing a link, so a false positive here just stays plain text.

use std::ops::Range;
use std::path::{Path, PathBuf};

/// A path found in some text, with the line (and column) it points at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileRef {
    /// Bytes of the path plus its `:line[:col]` / `#L12` suffix: what gets underlined.
    pub range: Range<usize>,
    pub path: String,
    pub line: Option<u32>,
    pub col: Option<u32>,
}

/// Extensions a bare file name (no separator) needs to count as a path, so
/// `example.com`, `e.g.` and `1.2.3` stay text.
const EXTENSIONS: &str = "rs toml lock md markdown txt json jsonc json5 yml yaml xml ini cfg conf env \
    ts tsx js jsx mjs cjs mts cts vue svelte css scss sass less html htm py pyi ipynb rb go mod sum \
    c h cc cpp cxx hpp hh cs csproj sln fs java kt kts gradle swift m mm scala clj ex exs erl hs ml \
    lua php pl r dart zig nim jl sql graphql proto sh bash zsh fish ps1 psm1 psd1 bat cmd nix tf hcl \
    dockerfile make mk cmake svg png jpg jpeg gif webp ico pdf csv tsv log diff patch wgsl glsl hlsl \
    metal wasm rst tex adoc org plist props targets vcxproj editorconfig gitignore gitattributes";

/// Bare names without an extension that are still clearly files.
const NAMES: &[&str] = &["Makefile", "Dockerfile", "Justfile", "Rakefile", "Gemfile", "LICENSE", "CODEOWNERS"];

/// Candidate file paths in `text` (prose, an inline code span, a tool row
/// summary). Pure; doesn't touch the disk. Ranges are sorted, don't overlap
/// and sit on char boundaries. Paths with spaces aren't found.
pub fn find(text: &str) -> Vec<FileRef> {
    let mut out = Vec::new();
    let mut start = None;
    for (i, c) in text.char_indices().chain(std::iter::once((text.len(), ' '))) {
        if is_delimiter(c) {
            if let Some(s) = start.take()
                && let Some(r) = token(text, s, i)
            {
                out.push(r);
            }
        } else if start.is_none() {
            start = Some(i);
        }
    }
    out
}

/// `path` against `cwd`: absolute paths as they are, `~/` from the home folder.
/// Pure apart from looking up the home folder.
pub fn resolve(cwd: &Path, path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\"))
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest);
    }
    if is_drive(path) || Path::new(path).is_absolute() {
        return PathBuf::from(path);
    }
    let rel = path.strip_prefix("./").or_else(|| path.strip_prefix(".\\")).unwrap_or(path);
    cwd.join(rel)
}

/// Whether `path` (against `cwd`) is a file. Blocking: touches the disk.
pub fn exists(cwd: &Path, path: &str) -> bool {
    resolve(cwd, path).is_file()
}

fn is_delimiter(c: char) -> bool {
    c.is_whitespace()
        || matches!(c, '`' | '"' | '<' | '>' | '|' | '(' | ')' | '[' | ']' | '{' | '}' | ',' | ';' | '=' | '*')
}

/// `C:\` or `C:/` at the start.
fn is_drive(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/')
}

/// The path in `text[start..end]`, one whitespace/delimiter-separated token.
fn token(text: &str, mut start: usize, mut end: usize) -> Option<FileRef> {
    if end - start > 1024 {
        return None;
    }
    let mut t = &text[start..end];
    // Leading quotes and Claude Code's `@file` mentions; trailing sentence punctuation.
    while let Some(c) = t.chars().next().filter(|c| matches!(c, '\'' | '@' | '«' | '“' | '‘')) {
        start += c.len_utf8();
        t = &text[start..end];
    }
    while let Some(c) = t.chars().last().filter(|c| matches!(c, '.' | ':' | '\'' | '!' | '?' | '…' | '»' | '”' | '’'))
    {
        end -= c.len_utf8();
        t = &text[start..end];
    }
    if t.is_empty() || t.contains("://") || t.contains('@') || t.to_ascii_lowercase().starts_with("www.") {
        return None;
    }
    let (path, line, col, path_end) = split_suffix(t);
    // The range covers `#L12` but not other anchors (`README.md#install`).
    let range_end = if line.is_some() || path_end == t.len() { end } else { start + path_end };
    if !is_path(path) {
        return None;
    }
    Some(FileRef { range: start..range_end, path: path.to_string(), line, col })
}

/// Splits `t` into its path and its `:line[:col]` / `#L12[-L20]` suffix, with
/// where the path ends in `t`.
fn split_suffix(t: &str) -> (&str, Option<u32>, Option<u32>, usize) {
    if let Some(hash) = t.find('#') {
        let frag = &t[hash + 1..];
        let line = frag.strip_prefix('L').map(|f| f.split("-L").next().unwrap_or(f)).and_then(digits);
        let range_ok = frag.strip_prefix('L').is_some_and(|f| match f.split_once("-L") {
            Some((_, b)) => digits(b).is_some(),
            None => true,
        });
        return (&t[..hash], line.filter(|_| range_ok), None, hash);
    }
    let Some((head, last)) = t.rsplit_once(':') else { return (t, None, None, t.len()) };
    let Some(n) = digits(last) else { return (t, None, None, t.len()) };
    if let Some((path, line)) = head.rsplit_once(':')
        && let Some(l) = digits(line)
    {
        return (path, Some(l), Some(n), path.len());
    }
    (head, Some(n), None, head.len())
}

fn digits(s: &str) -> Option<u32> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

/// Whether `p` (suffix gone) looks like a file path rather than a word.
fn is_path(p: &str) -> bool {
    if p.is_empty() || p.contains(['?', '#', '\'', '$', '%', '!', '&', '+', '^']) {
        return false;
    }
    let drive = is_drive(p);
    // A colon only as a drive letter's: `localhost:8080`, `a:b` aren't paths.
    if p.char_indices().any(|(i, c)| c == ':' && !(drive && i == 1)) {
        return false;
    }
    if !p.chars().any(char::is_alphabetic) {
        return false;
    }
    let posix_abs = p.starts_with('/');
    if p.starts_with("//") || p.starts_with("\\\\") {
        return false; // URLs without a scheme, UNC paths
    }
    let has_sep = p.contains(['/', '\\']);
    let last = p.rsplit(['/', '\\']).next().unwrap_or(p);
    let explicit = ["./", "../", ".\\", "..\\", "~/", "~\\"].iter().any(|pre| p.starts_with(pre));
    if drive || explicit {
        return true;
    }
    if posix_abs {
        // `/clear`, `/model` are slash commands: an absolute path needs a folder or an extension.
        return p[1..].contains('/') || has_ext(last);
    }
    if has_sep {
        // `and/or`, `km/h`, `2024/01/02` have no extension.
        return has_ext(last);
    }
    NAMES.contains(&p) || known_ext(p)
}

/// `name.ext` where ext has a letter: `mod.rs`, not `1.5`.
fn has_ext(name: &str) -> bool {
    match name.rsplit_once('.') {
        Some((_, ext)) => {
            !ext.is_empty()
                && ext.len() <= 12
                && ext.bytes().all(|b| b.is_ascii_alphanumeric())
                && ext.bytes().any(|b| b.is_ascii_alphabetic())
        }
        None => false,
    }
}

/// A bare `name.ext` with an extension from `EXTENSIONS`.
fn known_ext(name: &str) -> bool {
    let Some((stem, ext)) = name.rsplit_once('.') else { return false };
    !stem.is_empty()
        && !stem.ends_with('.')
        && stem.chars().all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.'))
        && EXTENSIONS.split_whitespace().any(|e| e.eq_ignore_ascii_case(ext))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(text: &str) -> Vec<(String, Option<u32>, Option<u32>)> {
        find(text).into_iter().map(|r| (r.path, r.line, r.col)).collect()
    }

    fn one(text: &str) -> FileRef {
        let found = find(text);
        assert_eq!(found.len(), 1, "{text:?}: {found:?}");
        found.into_iter().next().unwrap()
    }

    #[test]
    fn relative_paths_and_suffixes() {
        let r = one("see src/app/mod.rs:123 for that");
        assert_eq!((r.path.as_str(), r.line, r.col), ("src/app/mod.rs", Some(123), None));
        assert_eq!(&"see src/app/mod.rs:123 for that"[r.range], "src/app/mod.rs:123");
        assert_eq!(paths("src/main.rs:12:5"), [("src/main.rs".into(), Some(12), Some(5))]);
        assert_eq!(paths("./build"), [("./build".into(), None, None)]);
        assert_eq!(paths("..\\x"), [("..\\x".into(), None, None)]);
        assert_eq!(paths("~/notes.md"), [("~/notes.md".into(), None, None)]);
        assert_eq!(paths(".github/workflows/ci.yml"), [(".github/workflows/ci.yml".into(), None, None)]);
    }

    #[test]
    fn github_line_anchors() {
        let t = "at crates/x/lib.rs#L12.";
        let r = one(t);
        assert_eq!((r.path.as_str(), r.line), ("crates/x/lib.rs", Some(12)));
        assert_eq!(&t[r.range], "crates/x/lib.rs#L12");
        assert_eq!(paths("lib.rs#L12-L20"), [("lib.rs".into(), Some(12), None)]);
        let t = "README.md#install";
        let r = one(t);
        assert_eq!((r.path.as_str(), r.line, &t[r.range]), ("README.md", None, "README.md"));
    }

    #[test]
    fn windows_paths() {
        assert_eq!(paths(r"open C:\x\y.rs:4"), [(r"C:\x\y.rs".into(), Some(4), None)]);
        assert_eq!(paths("C:/Users/me/repo"), [("C:/Users/me/repo".into(), None, None)]);
        assert_eq!(paths(r"src\app\mod.rs:3:9"), [(r"src\app\mod.rs".into(), Some(3), Some(9))]);
        assert!(paths(r"\\server\share\x.rs").is_empty());
    }

    #[test]
    fn posix_paths() {
        assert_eq!(paths("/usr/lib/x.rs:7"), [("/usr/lib/x.rs".into(), Some(7), None)]);
        assert_eq!(paths("/etc/hosts"), [("/etc/hosts".into(), None, None)]);
        assert!(paths("run /clear or /model").is_empty());
    }

    #[test]
    fn bare_names() {
        assert_eq!(
            paths("Edit Cargo.toml and main.rs."),
            [("Cargo.toml".into(), None, None), ("main.rs".into(), None, None)]
        );
        assert_eq!(paths("the Makefile"), [("Makefile".into(), None, None)]);
        assert_eq!(paths("check.ps1:3"), [("check.ps1".into(), Some(3), None)]);
    }

    #[test]
    fn strips_punctuation() {
        for t in [
            "(src/a.rs)",
            "[src/a.rs]",
            "'src/a.rs'",
            "\"src/a.rs\",",
            "`src/a.rs`:",
            "src/a.rs;",
            "**src/a.rs**",
            "@src/a.rs",
        ] {
            let r = one(t);
            assert_eq!(r.path, "src/a.rs", "{t}");
            assert_eq!(&t[r.range], "src/a.rs", "{t}");
        }
        assert_eq!(paths("(src/a.rs:4)."), [("src/a.rs".into(), Some(4), None)]);
    }

    #[test]
    fn leaves_non_paths_alone() {
        for t in [
            "https://example.com/a/b.rs",
            "see www.example.com",
            "example.com",
            "version 1.2.3",
            "mail me@example.com",
            "git@github.com:x/y.git",
            "e.g. this, i.e. that.",
            "It works. Done.",
            "and/or km/h 2024/01/02 1/2.5",
            "localhost:8080 12:30",
            "src/*.rs",
            "//cdn.example.com/x.js",
            "a:b.rs",
            "",
        ] {
            assert!(paths(t).is_empty(), "{t:?}: {:?}", find(t));
        }
    }

    #[test]
    fn non_ascii_is_safe() {
        let t = "é src/日本.rs:3 ünï/ç.md… «main.rs» 😀/x.rs";
        let found = find(t);
        for r in &found {
            assert!(t.is_char_boundary(r.range.start) && t.is_char_boundary(r.range.end));
        }
        assert_eq!(found[0].path, "src/日本.rs");
        assert!(found.iter().any(|r| r.path == "ünï/ç.md"));
        assert!(found.iter().any(|r| r.path == "main.rs"));
        for t in ["é:", "日本.rs:", ":é", "#é", "é#L", "\u{0}", "C:é"] {
            let _ = find(t);
        }
    }

    #[test]
    fn resolves_against_cwd() {
        let cwd = Path::new("/work/app");
        assert_eq!(resolve(cwd, "src/x.rs"), cwd.join("src/x.rs"));
        assert_eq!(resolve(cwd, "./x.rs"), cwd.join("x.rs"));
        assert_eq!(resolve(cwd, r"C:\x\y.rs"), PathBuf::from(r"C:\x\y.rs"));
        if let Some(home) = dirs::home_dir() {
            assert_eq!(resolve(cwd, "~/a.md"), home.join("a.md"));
        }
        #[cfg(unix)]
        assert_eq!(resolve(cwd, "/usr/x.rs"), PathBuf::from("/usr/x.rs"));
    }

    #[test]
    fn exists_checks_files() {
        let dir = std::env::temp_dir().join(format!("kynex-links-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/a.rs"), "").unwrap();
        assert!(exists(&dir, "src/a.rs"));
        assert!(!exists(&dir, "src"));
        assert!(!exists(&dir, "src/b.rs"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
