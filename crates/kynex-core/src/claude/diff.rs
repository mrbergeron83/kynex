//! Zero-dependency line-based unified diff engine for Claude `Edit`, `MultiEdit`, and `Write`
//! tool calls.
//!
//! Produces compact unified hunks with `+added` / `-removed` totals and 1-based old/new line
//! numbers, safely capped so huge file writes stay fast and responsive.

use serde_json::Value;

/// Maximum number of diff lines emitted across all hunks before truncating.
pub const MAX_DIFF_LINES: usize = 400;

/// Default number of unchanged context lines kept around each change.
pub const DEFAULT_CONTEXT: usize = 3;

/// Maximum DP cell count (`old_mid * new_mid`) before falling back to a straight replace block.
const MAX_LCS_CELLS: usize = 250_000;

/// Kind of a single line in a unified diff hunk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffLineKind {
    Context,
    Add,
    Remove,
}

/// One line inside a [`DiffHunk`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    /// 1-based line number in the old text (present for `Context` and `Remove`).
    pub old_line: Option<usize>,
    /// 1-based line number in the new text (present for `Context` and `Add`).
    pub new_line: Option<usize>,
    pub text: String,
}

/// A contiguous group of changed lines with surrounding context.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffHunk {
    pub old_start: usize,
    pub old_count: usize,
    pub new_start: usize,
    pub new_count: usize,
    pub lines: Vec<DiffLine>,
}

/// Result of diffing an `Edit` or `Write` tool call.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct EditDiff {
    /// Total added lines across the entire edit (counted before truncation).
    pub added: usize,
    /// Total removed lines across the entire edit (counted before truncation).
    pub removed: usize,
    pub hunks: Vec<DiffHunk>,
    /// `true` when emitted lines were capped at [`MAX_DIFF_LINES`].
    pub truncated: bool,
}

impl EditDiff {
    pub fn is_empty(&self) -> bool {
        self.added == 0 && self.removed == 0 && self.hunks.is_empty()
    }
}

/// Computes a unified diff for an `Edit` (`old_string` -> `new_string`) with [`DEFAULT_CONTEXT`].
pub fn diff_edit(old_string: &str, new_string: &str) -> EditDiff {
    diff_edit_with_context(old_string, new_string, DEFAULT_CONTEXT)
}

/// Computes a unified diff for an `Edit` (`old_string` -> `new_string`) with `context` lines.
pub fn diff_edit_with_context(old_string: &str, new_string: &str, context: usize) -> EditDiff {
    let old_lines = split_lines(old_string);
    let new_lines = split_lines(new_string);

    if old_lines == new_lines {
        return EditDiff::default();
    }

    let ops = compute_ops(&old_lines, &new_lines);
    build_diff_from_ops(&ops, context, MAX_DIFF_LINES)
}

/// Computes a diff for a `Write` (`content`) where every line is an addition.
pub fn diff_write(content: &str) -> EditDiff {
    let lines = split_lines(content);
    if lines.is_empty() {
        return EditDiff::default();
    }

    let added = lines.len();
    let truncated = added > MAX_DIFF_LINES;
    let emit_count = added.min(MAX_DIFF_LINES);
    let mut hunk_lines = Vec::with_capacity(emit_count);
    for (idx, line) in lines.iter().take(emit_count).enumerate() {
        hunk_lines.push(DiffLine {
            kind: DiffLineKind::Add,
            old_line: None,
            new_line: Some(idx + 1),
            text: (*line).to_string(),
        });
    }

    EditDiff {
        added,
        removed: 0,
        hunks: vec![DiffHunk { old_start: 0, old_count: 0, new_start: 1, new_count: emit_count, lines: hunk_lines }],
        truncated,
    }
}

/// Computes an [`EditDiff`] directly from a Claude tool call's name and JSON `input`.
///
/// Supports `Edit`, `MultiEdit`, and `Write`. Returns `None` for other tools or malformed inputs.
pub fn diff_from_tool_input(tool_name: &str, input: &Value) -> Option<EditDiff> {
    match tool_name {
        "Edit" => {
            let old_s = input.get("old_string").and_then(Value::as_str)?;
            let new_s = input.get("new_string").and_then(Value::as_str)?;
            Some(diff_edit(old_s, new_s))
        }
        "MultiEdit" => {
            let edits = input.get("edits").and_then(Value::as_array)?;
            let mut combined = EditDiff::default();
            let mut emitted = 0usize;
            for edit in edits {
                let Some(old_s) = edit.get("old_string").and_then(Value::as_str) else { continue };
                let Some(new_s) = edit.get("new_string").and_then(Value::as_str) else { continue };
                let part = diff_edit(old_s, new_s);
                combined.added += part.added;
                combined.removed += part.removed;
                if part.truncated {
                    combined.truncated = true;
                }
                for mut hunk in part.hunks {
                    if emitted >= MAX_DIFF_LINES {
                        combined.truncated = true;
                        break;
                    }
                    let remaining = MAX_DIFF_LINES - emitted;
                    if hunk.lines.len() > remaining {
                        hunk.lines.truncate(remaining);
                        recompute_hunk_counts(&mut hunk);
                        combined.truncated = true;
                    }
                    emitted += hunk.lines.len();
                    if !hunk.lines.is_empty() {
                        combined.hunks.push(hunk);
                    }
                }
            }
            Some(combined)
        }
        "Write" => {
            let content = input.get("content").and_then(Value::as_str)?;
            Some(diff_write(content))
        }
        _ => None,
    }
}

#[derive(Clone, Debug)]
struct Op<'a> {
    kind: DiffLineKind,
    old_line: Option<usize>,
    new_line: Option<usize>,
    text: &'a str,
}

fn split_lines(s: &str) -> Vec<&str> {
    if s.is_empty() {
        return Vec::new();
    }
    let trimmed = s.strip_suffix("\r\n").or_else(|| s.strip_suffix('\n')).unwrap_or(s);
    if trimmed.is_empty() && s.len() <= 2 {
        return vec![""];
    }
    trimmed.split('\n').map(|line| line.strip_suffix('\r').unwrap_or(line)).collect()
}

fn compute_ops<'a>(old: &[&'a str], new: &[&'a str]) -> Vec<Op<'a>> {
    let mut prefix = 0usize;
    let max_prefix = old.len().min(new.len());
    while prefix < max_prefix && old[prefix] == new[prefix] {
        prefix += 1;
    }

    let mut suffix = 0usize;
    let max_suffix = (old.len() - prefix).min(new.len() - prefix);
    while suffix < max_suffix && old[old.len() - 1 - suffix] == new[new.len() - 1 - suffix] {
        suffix += 1;
    }

    let old_mid = &old[prefix..old.len() - suffix];
    let new_mid = &new[prefix..new.len() - suffix];

    let mut ops = Vec::with_capacity(old.len() + new.len());
    for (i, line) in old.iter().take(prefix).enumerate() {
        ops.push(Op { kind: DiffLineKind::Context, old_line: Some(i + 1), new_line: Some(i + 1), text: line });
    }

    if old_mid.len().saturating_mul(new_mid.len()) <= MAX_LCS_CELLS {
        push_lcs_ops(&mut ops, old_mid, new_mid, prefix, prefix);
    } else {
        for (i, line) in old_mid.iter().enumerate() {
            ops.push(Op { kind: DiffLineKind::Remove, old_line: Some(prefix + i + 1), new_line: None, text: line });
        }
        for (j, line) in new_mid.iter().enumerate() {
            ops.push(Op { kind: DiffLineKind::Add, old_line: None, new_line: Some(prefix + j + 1), text: line });
        }
    }

    let old_tail = old.len() - suffix;
    let new_tail = new.len() - suffix;
    for (k, line) in old[old_tail..].iter().enumerate() {
        ops.push(Op {
            kind: DiffLineKind::Context,
            old_line: Some(old_tail + k + 1),
            new_line: Some(new_tail + k + 1),
            text: line,
        });
    }

    ops
}

fn push_lcs_ops<'a>(ops: &mut Vec<Op<'a>>, a: &[&'a str], b: &[&'a str], a_base: usize, b_base: usize) {
    let m = a.len();
    let n = b.len();
    if m == 0 {
        for (j, line) in b.iter().enumerate() {
            ops.push(Op { kind: DiffLineKind::Add, old_line: None, new_line: Some(b_base + j + 1), text: line });
        }
        return;
    }
    if n == 0 {
        for (i, line) in a.iter().enumerate() {
            ops.push(Op { kind: DiffLineKind::Remove, old_line: Some(a_base + i + 1), new_line: None, text: line });
        }
        return;
    }

    let stride = n + 1;
    let mut dp = vec![0u32; (m + 1) * stride];
    for i in (0..m).rev() {
        for j in (0..n).rev() {
            dp[i * stride + j] = if a[i] == b[j] {
                dp[(i + 1) * stride + (j + 1)] + 1
            } else {
                dp[(i + 1) * stride + j].max(dp[i * stride + (j + 1)])
            };
        }
    }

    let (mut i, mut j) = (0usize, 0usize);
    while i < m && j < n {
        if a[i] == b[j] {
            ops.push(Op {
                kind: DiffLineKind::Context,
                old_line: Some(a_base + i + 1),
                new_line: Some(b_base + j + 1),
                text: a[i],
            });
            i += 1;
            j += 1;
        } else if dp[(i + 1) * stride + j] >= dp[i * stride + (j + 1)] {
            ops.push(Op { kind: DiffLineKind::Remove, old_line: Some(a_base + i + 1), new_line: None, text: a[i] });
            i += 1;
        } else {
            ops.push(Op { kind: DiffLineKind::Add, old_line: None, new_line: Some(b_base + j + 1), text: b[j] });
            j += 1;
        }
    }
    while i < m {
        ops.push(Op { kind: DiffLineKind::Remove, old_line: Some(a_base + i + 1), new_line: None, text: a[i] });
        i += 1;
    }
    while j < n {
        ops.push(Op { kind: DiffLineKind::Add, old_line: None, new_line: Some(b_base + j + 1), text: b[j] });
        j += 1;
    }
}

fn build_diff_from_ops(ops: &[Op<'_>], context: usize, max_lines: usize) -> EditDiff {
    let mut added = 0usize;
    let mut removed = 0usize;
    let mut change_ranges: Vec<(usize, usize)> = Vec::new();

    for (idx, op) in ops.iter().enumerate() {
        match op.kind {
            DiffLineKind::Add => added += 1,
            DiffLineKind::Remove => removed += 1,
            DiffLineKind::Context => continue,
        }
        let start = idx.saturating_sub(context);
        let end = (idx + context + 1).min(ops.len());
        if let Some(last) = change_ranges.last_mut()
            && start <= last.1
        {
            last.1 = last.1.max(end);
        } else {
            change_ranges.push((start, end));
        }
    }

    if change_ranges.is_empty() {
        return EditDiff::default();
    }

    let mut hunks = Vec::with_capacity(change_ranges.len());
    let mut emitted = 0usize;
    let mut truncated = false;

    for (start, end) in change_ranges {
        if emitted >= max_lines {
            truncated = true;
            break;
        }
        let slice = &ops[start..end];
        let remaining = max_lines - emitted;
        let take = slice.len().min(remaining);
        if take < slice.len() {
            truncated = true;
        }

        let lines: Vec<DiffLine> = slice[..take]
            .iter()
            .map(|op| DiffLine {
                kind: op.kind,
                old_line: op.old_line,
                new_line: op.new_line,
                text: op.text.to_string(),
            })
            .collect();
        emitted += lines.len();

        let mut hunk = DiffHunk { old_start: 0, old_count: 0, new_start: 0, new_count: 0, lines };
        recompute_hunk_counts(&mut hunk);
        hunks.push(hunk);
    }

    EditDiff { added, removed, hunks, truncated }
}

fn recompute_hunk_counts(hunk: &mut DiffHunk) {
    hunk.old_start = hunk.lines.iter().find_map(|l| l.old_line).unwrap_or(0);
    hunk.new_start = hunk.lines.iter().find_map(|l| l.new_line).unwrap_or(0);
    hunk.old_count = hunk.lines.iter().filter(|l| l.old_line.is_some()).count();
    hunk.new_count = hunk.lines.iter().filter(|l| l.new_line.is_some()).count();
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn identical_text_produces_empty_diff() {
        let d = diff_edit("a\nb\nc\n", "a\nb\nc\n");
        assert!(d.is_empty());
        assert!(!d.truncated);
    }

    #[test]
    fn simple_line_replacement_tracks_counts_and_line_numbers() {
        let old = "one\ntwo\nthree\nfour";
        let new = "one\nTWO\nTHREE\nfour";
        let d = diff_edit(old, new);
        assert_eq!(d.added, 2);
        assert_eq!(d.removed, 2);
        assert!(!d.truncated);
        assert_eq!(d.hunks.len(), 1);
        let h = &d.hunks[0];
        assert_eq!((h.old_start, h.old_count, h.new_start, h.new_count), (1, 4, 1, 4));
        assert_eq!(h.lines[0].kind, DiffLineKind::Context);
        assert_eq!(h.lines[0].old_line, Some(1));
        assert_eq!(h.lines[0].new_line, Some(1));
        assert!(h.lines.iter().any(|l| l.kind == DiffLineKind::Remove && l.text == "two" && l.old_line == Some(2)));
        assert!(h.lines.iter().any(|l| l.kind == DiffLineKind::Add && l.text == "TWO" && l.new_line == Some(2)));
    }

    #[test]
    fn distant_edits_split_into_multiple_hunks() {
        let mut old: Vec<String> = (1..=20).map(|i| format!("line {i}")).collect();
        let mut new = old.clone();
        old[1] = "old top".into();
        new[1] = "new top".into();
        old[18] = "old bottom".into();
        new[18] = "new bottom".into();
        let d = diff_edit_with_context(&old.join("\n"), &new.join("\n"), 2);
        assert_eq!(d.added, 2);
        assert_eq!(d.removed, 2);
        assert_eq!(d.hunks.len(), 2);
        assert_eq!(d.hunks[0].old_start, 1);
        assert_eq!(d.hunks[1].old_start, 17);
    }

    #[test]
    fn handles_crlf_and_pure_insert_or_delete() {
        let ins = diff_edit("", "alpha\r\nbeta\r\n");
        assert_eq!((ins.added, ins.removed), (2, 0));
        assert_eq!(ins.hunks[0].lines[0].text, "alpha");
        assert_eq!(ins.hunks[0].lines[1].text, "beta");

        let del = diff_edit("alpha\nbeta", "");
        assert_eq!((del.added, del.removed), (0, 2));
        assert_eq!(del.hunks[0].lines[0].kind, DiffLineKind::Remove);
    }

    #[test]
    fn caps_large_writes_with_exact_added_count() {
        let content: String = (1..=600).map(|i| format!("row {i}\n")).collect();
        let d = diff_write(&content);
        assert_eq!(d.added, 600);
        assert_eq!(d.removed, 0);
        assert!(d.truncated);
        assert_eq!(d.hunks.len(), 1);
        assert_eq!(d.hunks[0].lines.len(), MAX_DIFF_LINES);
    }

    #[test]
    fn parses_tool_inputs_for_edit_multiedit_and_write() {
        let edit = diff_from_tool_input("Edit", &json!({"old_string": "a", "new_string": "b"})).unwrap();
        assert_eq!((edit.added, edit.removed), (1, 1));

        let multi = diff_from_tool_input(
            "MultiEdit",
            &json!({"edits": [{"old_string": "a", "new_string": "b"}, {"old_string": "x", "new_string": "y\nz"}]}),
        )
        .unwrap();
        assert_eq!((multi.added, multi.removed), (3, 2));

        let write = diff_from_tool_input("Write", &json!({"content": "hello\nworld"})).unwrap();
        assert_eq!((write.added, write.removed), (2, 0));

        assert!(diff_from_tool_input("Bash", &json!({"command": "ls"})).is_none());
    }
}
