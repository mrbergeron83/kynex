//! Pure-Rust composer helpers for ClaudeChat:
//! - [`PasteFold`]: collapses large pasted snippets into compact `[Pasted text #N +M lines]` chips.
//! - [`FileIndex`]: indexes workspace files (`git ls-files` with directory walk fallback) for `@` lookup.
//! - [`ComposerTrigger`] / [`active_trigger`] / [`match_commands`]: `/` slash command and `@` file triggers.

use std::cmp::Reverse;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::fuzzy;

/// Minimum line count before a pasted snippet is folded into a chip.
pub const FOLD_MIN_LINES: usize = 10;

/// Minimum character count before a pasted snippet is folded into a chip.
pub const FOLD_MIN_CHARS: usize = 1_000;

/// Maximum workspace files indexed by [`FileIndex`].
pub const MAX_INDEXED_FILES: usize = 25_000;

const IGNORED_DIRS: &[&str] =
    &[".git", ".hg", ".svn", "target", "node_modules", "dist", "build", "__pycache__", ".venv", ".next", ".cache"];

// ---------------------------------------------------------------------------
// Paste folding
// ---------------------------------------------------------------------------

/// One folded paste block stored alongside the composer text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PastedBlock {
    pub id: usize,
    pub label: String,
    pub content: String,
    pub line_count: usize,
}

/// Manages folded large pastes (`[Pasted text #N +M lines]`) for a single composer.
#[derive(Clone, Debug, Default)]
pub struct PasteFold {
    next_id: usize,
    blocks: Vec<PastedBlock>,
}

impl PasteFold {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns `true` when `text` meets the folding threshold (`>= 10` lines or `>= 1,000` chars).
    pub fn should_fold(text: &str) -> bool {
        text.split('\n').count() >= FOLD_MIN_LINES || text.chars().count() >= FOLD_MIN_CHARS
    }

    /// Folds `text` if it meets the threshold, returning the created [`PastedBlock`] chip.
    pub fn ingest(&mut self, text: String) -> Option<PastedBlock> {
        if !Self::should_fold(&text) {
            return None;
        }
        Some(self.fold_always(text))
    }

    /// Unconditionally folds `text` into a new [`PastedBlock`] chip.
    pub fn fold_always(&mut self, text: String) -> PastedBlock {
        self.next_id += 1;
        let id = self.next_id;
        let line_count = text.lines().count().max(1);
        let label = format!("[Pasted text #{id} +{line_count} lines]");
        let block = PastedBlock { id, label, content: text, line_count };
        self.blocks.push(block.clone());
        block
    }

    /// All currently active folded paste blocks.
    pub fn blocks(&self) -> &[PastedBlock] {
        &self.blocks
    }

    /// Looks up a block by its chip label.
    pub fn get(&self, label: &str) -> Option<&PastedBlock> {
        self.blocks.iter().find(|b| b.label == label)
    }

    /// Drops any stored blocks whose labels no longer appear in `composer_text`.
    pub fn prune_unused(&mut self, composer_text: &str) {
        self.blocks.retain(|b| composer_text.contains(&b.label));
    }

    /// Expands the first occurrence of `label` inside `composer_text`, returning the updated text.
    /// Removes the block from storage if no further occurrences remain.
    pub fn expand_one(&mut self, composer_text: &str, label: &str) -> Option<String> {
        let idx = self.blocks.iter().position(|b| b.label == label)?;
        let pos = composer_text.find(label)?;
        let content = &self.blocks[idx].content;
        let mut out = String::with_capacity(composer_text.len() - label.len() + content.len());
        out.push_str(&composer_text[..pos]);
        out.push_str(content);
        out.push_str(&composer_text[pos + label.len()..]);
        if !out.contains(label) {
            self.blocks.remove(idx);
        }
        Some(out)
    }

    /// Expands every known paste chip in `composer_text` in a single left-to-right pass
    /// (so expanded contents are never re-expanded).
    pub fn expand(&self, composer_text: &str) -> String {
        if self.blocks.is_empty() {
            return composer_text.to_string();
        }
        let mut out = String::with_capacity(composer_text.len());
        let mut cursor = 0usize;
        while cursor < composer_text.len() {
            let rest = &composer_text[cursor..];
            if rest.starts_with("[Pasted text #")
                && let Some(block) = self.blocks.iter().find(|b| rest.starts_with(&b.label))
            {
                out.push_str(&block.content);
                cursor += block.label.len();
                continue;
            }
            let ch = rest.chars().next().unwrap();
            out.push(ch);
            cursor += ch.len_utf8();
        }
        out
    }

    /// Clears all stored blocks and resets numbering.
    pub fn clear(&mut self) {
        self.blocks.clear();
        self.next_id = 0;
    }
}

// ---------------------------------------------------------------------------
// Workspace file index
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct IndexedEntry {
    path: String,
    lower: String,
    filename_start: usize,
}

/// One fuzzy search hit returned by [`FileIndex::search`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileMatch {
    pub path: String,
    /// Byte offsets of matched characters inside `path`.
    pub positions: Vec<usize>,
    pub score: i64,
}

/// Workspace-relative file list indexed for fast `@file` fuzzy completion.
#[derive(Clone, Debug, Default)]
pub struct FileIndex {
    entries: Vec<IndexedEntry>,
}

impl FileIndex {
    /// Builds an index for `root` via `git ls-files`, falling back to a directory walk.
    /// Blocks on filesystem/process I/O; run off the UI thread.
    pub fn build(root: &Path) -> Self {
        let mut paths = git_ls_files(root).unwrap_or_else(|| walk_workspace(root));
        paths.sort();
        paths.dedup();
        paths.truncate(MAX_INDEXED_FILES);
        Self::from_paths(paths)
    }

    /// Builds an index directly from relative paths (useful for tests).
    pub fn from_paths<I, S>(paths: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let entries = paths
            .into_iter()
            .take(MAX_INDEXED_FILES)
            .map(|p| {
                let path = normalize_slashes(p.into());
                let lower = fuzzy::lower(&path);
                let filename_start = lower.rfind('/').map_or(0, |i| i + 1);
                IndexedEntry { path, lower, filename_start }
            })
            .collect();
        Self { entries }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn files(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|e| e.path.as_str())
    }

    /// Fuzzy-searches indexed files for `query`, returning up to `limit` matches sorted best-first.
    pub fn search(&self, query: &str, limit: usize) -> Vec<FileMatch> {
        if limit == 0 || self.entries.is_empty() {
            return Vec::new();
        }
        let q = fuzzy::lower(query.trim()).replace('\\', "/");
        if q.is_empty() {
            return self
                .entries
                .iter()
                .take(limit)
                .map(|e| FileMatch { path: e.path.clone(), positions: Vec::new(), score: 0 })
                .collect();
        }

        let mut matches: Vec<FileMatch> = Vec::new();
        let has_sep = q.contains('/');

        for entry in &self.entries {
            let best = if !has_sep {
                if let Some(pos) = fuzzy::find(&entry.lower, &q, entry.filename_start) {
                    let mut score = fuzzy::score(&entry.lower, &pos) + 40;
                    if entry.lower[entry.filename_start..].starts_with(&q) {
                        score += 25;
                    }
                    Some((pos, score))
                } else {
                    fuzzy::find(&entry.lower, &q, 0).map(|pos| {
                        let score = fuzzy::score(&entry.lower, &pos);
                        (pos, score)
                    })
                }
            } else {
                fuzzy::find(&entry.lower, &q, 0).map(|pos| {
                    let mut score = fuzzy::score(&entry.lower, &pos);
                    if entry.lower.starts_with(&q) {
                        score += 20;
                    }
                    (pos, score)
                })
            };

            if let Some((positions, mut score)) = best {
                // Slightly prefer shorter paths when scores tie.
                score -= (entry.path.len() as i64) / 8;
                matches.push(FileMatch { path: entry.path.clone(), positions, score });
            }
        }

        matches.sort_by(|a, b| {
            b.score.cmp(&a.score).then_with(|| a.path.len().cmp(&b.path.len())).then_with(|| a.path.cmp(&b.path))
        });
        matches.truncate(limit);
        matches
    }
}

fn normalize_slashes(path: String) -> String {
    if path.contains('\\') { path.replace('\\', "/") } else { path }
}

fn git_ls_files(root: &Path) -> Option<Vec<String>> {
    let mut cmd = Command::new("git");
    cmd.args(["-C"])
        .arg(root)
        .args(["ls-files", "--cached", "--others", "--exclude-standard", "-z"])
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let output = cmd.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let mut files = Vec::new();
    for raw in output.stdout.split(|&b| b == 0) {
        if raw.is_empty() {
            continue;
        }
        if let Ok(s) = std::str::from_utf8(raw) {
            files.push(normalize_slashes(s.to_string()));
            if files.len() >= MAX_INDEXED_FILES {
                break;
            }
        }
    }
    Some(files)
}

fn walk_workspace(root: &Path) -> Vec<String> {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_symlink() {
                continue;
            }
            if ft.is_dir() {
                let name = entry.file_name();
                if name.to_str().is_some_and(|n| IGNORED_DIRS.contains(&n)) {
                    continue;
                }
                stack.push(path);
            } else if ft.is_file()
                && let Ok(rel) = path.strip_prefix(root)
                && let Some(s) = rel.to_str()
            {
                files.push(normalize_slashes(s.to_string()));
                if files.len() >= MAX_INDEXED_FILES {
                    return files;
                }
            }
        }
    }
    files
}

// ---------------------------------------------------------------------------
// Composer triggers (`/command`, `@file`) & slash command matching
// ---------------------------------------------------------------------------

/// Active autocomplete trigger at the composer cursor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ComposerTrigger {
    /// Leading `/command` query (`start` is the byte offset of `/`).
    Slash { start: usize, query: String },
    /// `@file` mention query (`start` is the byte offset of `@`).
    AtFile { start: usize, query: String },
}

/// Detects an active `/` command or `@` file trigger immediately preceding `cursor` (byte offset).
/// Panic-free even if `cursor` is out of bounds or not on a UTF-8 char boundary.
pub fn active_trigger(text: &str, cursor: usize) -> Option<ComposerTrigger> {
    let mut clamped = cursor.min(text.len());
    while clamped > 0 && !text.is_char_boundary(clamped) {
        clamped -= 1;
    }
    let before = &text[..clamped];

    // Scan back to the start of the current whitespace-delimited token.
    let token_start =
        before.char_indices().rev().find(|&(_, c)| c.is_whitespace()).map_or(0, |(i, c)| i + c.len_utf8());
    let token = &before[token_start..];

    if let Some(query) = token.strip_prefix('/') {
        // Slash commands only trigger at the very beginning of the composer (ignoring leading whitespace).
        if before[..token_start].trim().is_empty() && !query.contains('/') {
            return Some(ComposerTrigger::Slash { start: token_start, query: query.to_string() });
        }
        return None;
    }

    if let Some(query) = token.strip_prefix('@') {
        return Some(ComposerTrigger::AtFile { start: token_start, query: query.to_string() });
    }

    // Also allow `@file` immediately after opening punctuation like `(@foo` or `"@foo`.
    if let Some(rel_at) = token.rfind('@') {
        let prev = token[..rel_at].chars().next_back();
        if prev.is_some_and(|c| matches!(c, '(' | '[' | '{' | '"' | '\'')) {
            let start = token_start + rel_at;
            return Some(ComposerTrigger::AtFile { start, query: token[rel_at + 1..].to_string() });
        }
    }

    None
}

/// A slash command or skill item shown in the `/` completion popup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandItem {
    pub name: String,
    pub description: String,
    pub argument_hint: Option<String>,
    pub aliases: Vec<String>,
}

/// One ranked match returned by [`match_commands`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandMatch {
    /// Index into the input `commands` slice.
    pub index: usize,
    pub score: i64,
    /// Byte offsets matched within `CommandItem::name` (if matched against name).
    pub positions: Vec<usize>,
}

/// Fuzzy-matches `query` against `commands` (name, aliases, and description), best-first.
pub fn match_commands(commands: &[CommandItem], query: &str, limit: usize) -> Vec<CommandMatch> {
    if limit == 0 || commands.is_empty() {
        return Vec::new();
    }
    let q = fuzzy::lower(query.trim().trim_start_matches('/'));
    if q.is_empty() {
        return (0..commands.len().min(limit))
            .map(|index| CommandMatch { index, score: 0, positions: Vec::new() })
            .collect();
    }

    let mut ranked: Vec<CommandMatch> = Vec::new();
    for (index, cmd) in commands.iter().enumerate() {
        let raw_name = cmd.name.trim_start_matches('/');
        let name_lower = fuzzy::lower(raw_name);
        let mut best: Option<(i64, Vec<usize>)> = None;

        if let Some(pos) = fuzzy::find(&name_lower, &q, 0) {
            let mut score = fuzzy::score(&name_lower, &pos) + 60;
            if name_lower == q {
                score += 200;
            } else if name_lower.starts_with(&q) {
                score += 100;
            }
            best = Some((score, pos));
        }

        for alias in &cmd.aliases {
            let alias_lower = fuzzy::lower(alias.trim_start_matches('/'));
            if let Some(pos) = fuzzy::find(&alias_lower, &q, 0) {
                let mut score = fuzzy::score(&alias_lower, &pos) + 40;
                if alias_lower == q {
                    score += 150;
                } else if alias_lower.starts_with(&q) {
                    score += 80;
                }
                if best.as_ref().is_none_or(|(s, _)| score > *s) {
                    best = Some((score, Vec::new()));
                }
            }
        }

        let desc_lower = fuzzy::lower(&cmd.description);
        if let Some(pos) = fuzzy::find_words(&desc_lower, &q) {
            let score = fuzzy::score(&desc_lower, &pos);
            if best.as_ref().is_none_or(|(s, _)| score > *s) {
                best = Some((score, Vec::new()));
            }
        }

        if let Some((score, positions)) = best {
            ranked.push(CommandMatch { index, score, positions });
        }
    }

    ranked.sort_by_key(|m| (Reverse(m.score), m.index));
    ranked.truncate(limit);
    ranked
}

/// A message that starts with a slash command: its name and what follows,
/// e.g. `/compact keep the plan` → `("compact", "keep the plan")`.
pub fn slash_command(text: &str) -> Option<(&str, &str)> {
    let rest = text.trim_start().strip_prefix('/')?;
    let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
    let name = &rest[..end];
    let ok = |c: char| c.is_alphanumeric() || matches!(c, '-' | '_' | ':' | '.');
    (!name.is_empty() && name.chars().all(ok)).then(|| (name, rest[end..].trim()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slash_commands_split_name_and_arguments() {
        assert_eq!(slash_command("/compact keep the plan"), Some(("compact", "keep the plan")));
        assert_eq!(slash_command("  /review"), Some(("review", "")));
        assert_eq!(slash_command("/plugin:skill a\nb"), Some(("plugin:skill", "a\nb")));
        assert_eq!(slash_command("/ nothing"), None);
        assert_eq!(slash_command("src/main.rs"), None);
        assert_eq!(slash_command("/usr/bin is a path"), None);
    }

    #[test]
    fn paste_fold_ingests_prunes_and_expands_without_reexpanding() {
        let mut fold = PasteFold::new();
        assert!(fold.ingest("short paste".into()).is_none());

        let long_lines: String = (1..=12).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
        let b1 = fold.ingest(long_lines.clone()).unwrap();
        assert_eq!(b1.label, "[Pasted text #1 +12 lines]");

        let long_chars = "x".repeat(1_050);
        let b2 = fold.ingest(long_chars.clone()).unwrap();
        assert_eq!(b2.label, "[Pasted text #2 +1 lines]");

        let composer = format!("Look at {} and {}", b1.label, b2.label);
        let expanded = fold.expand(&composer);
        assert_eq!(expanded, format!("Look at {long_lines} and {long_chars}"));

        let after_one = fold.expand_one(&composer, &b1.label).unwrap();
        assert!(after_one.contains(&long_lines));
        assert!(after_one.contains(&b2.label));
        assert_eq!(fold.blocks().len(), 1);

        fold.prune_unused("no chips left");
        assert!(fold.blocks().is_empty());

        fold.clear();
        assert_eq!(fold.fold_always("a\nb".into()).id, 1);
    }

    #[test]
    fn file_index_prefers_filename_and_fuzzy_matches() {
        let idx = FileIndex::from_paths([
            "crates/kynex-core/src/claude/composer.rs",
            "src/claude_chat/composer.rs",
            "src/app/tiles.rs",
            "README.md",
        ]);
        let hits = idx.search("comp", 5);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].path, "src/claude_chat/composer.rs");

        let by_path = idx.search("core/comp", 5);
        assert_eq!(by_path.len(), 1);
        assert_eq!(by_path[0].path, "crates/kynex-core/src/claude/composer.rs");
    }

    #[test]
    fn file_index_walks_non_git_directory() {
        let dir = std::env::temp_dir().join(format!("kynex-file-index-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::create_dir_all(dir.join("target")).unwrap();
        std::fs::write(dir.join("src/main.rs"), "").unwrap();
        std::fs::write(dir.join("target/ignored.rs"), "").unwrap();

        let idx = FileIndex::build(&dir);
        let paths: Vec<&str> = idx.files().collect();
        assert_eq!(paths, vec!["src/main.rs"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn active_trigger_handles_slash_at_and_unicode_boundaries() {
        assert_eq!(active_trigger("/comp", 5), Some(ComposerTrigger::Slash { start: 0, query: "comp".into() }));
        assert_eq!(active_trigger("explain /comp", 13), None);
        assert_eq!(active_trigger("/a/b", 4), None);

        assert_eq!(
            active_trigger("check @src/ma", 13),
            Some(ComposerTrigger::AtFile { start: 6, query: "src/ma".into() })
        );
        assert_eq!(active_trigger("see (@foo", 9), Some(ComposerTrigger::AtFile { start: 5, query: "foo".into() }));

        // Mid-char byte index must never panic.
        let s = "héllo @wörld";
        let _ = active_trigger(s, 2);
        let _ = active_trigger(s, 999);
    }

    #[test]
    fn match_commands_ranks_name_aliases_and_descriptions() {
        let cmds = vec![
            CommandItem {
                name: "compact".into(),
                description: "Summarize conversation context".into(),
                argument_hint: Some("[instructions]".into()),
                aliases: vec!["compress".into()],
            },
            CommandItem {
                name: "fork".into(),
                description: "Branch into a new chat pane".into(),
                argument_hint: None,
                aliases: vec!["branch".into()],
            },
            CommandItem {
                name: "clear".into(),
                description: "Reset the current conversation".into(),
                argument_hint: None,
                aliases: vec![],
            },
        ];

        let m = match_commands(&cmds, "comp", 10);
        assert_eq!(m[0].index, 0);

        let by_alias = match_commands(&cmds, "branch", 10);
        assert_eq!(by_alias[0].index, 1);

        let by_desc = match_commands(&cmds, "reset", 10);
        assert_eq!(by_desc[0].index, 2);
    }
}
