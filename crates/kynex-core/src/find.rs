//! Find in the transcript (Ctrl+F): every place a query appears in a session's
//! messages, replies, thinking and tool output, case-insensitively.
//!
//! Text is searched block by block, the way the transcript draws it (a reply's
//! paragraphs, list items and code blocks are separate blocks), so the matches
//! counted here are the ones highlighted there.

use std::ops::Range;

use crate::markdown::{self, Block};
use crate::session::Item;

/// One match: the item it's in and which match it is within that item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hit {
    pub item: usize,
    pub nth: usize,
}

/// Where `query` appears in `text`, ignoring case (Unicode-aware), as byte
/// ranges of `text`, left to right and not overlapping.
pub fn occurrences(text: &str, query: &str) -> Vec<Range<usize>> {
    let q: Vec<char> = query.chars().flat_map(char::to_lowercase).collect();
    if q.is_empty() {
        return Vec::new();
    }
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(c) = text[from..].chars().next() {
        match match_at(text, from, &q) {
            Some(end) => {
                found.push(from..end);
                from = end;
            }
            None => from += c.len_utf8(),
        }
    }
    found
}

/// Where a match of the lowercased `q` starting at byte `from` ends, if one does.
fn match_at(text: &str, from: usize, q: &[char]) -> Option<usize> {
    let mut want = q.iter();
    for (i, c) in text[from..].char_indices() {
        for l in c.to_lowercase() {
            if want.next() != Some(&l) {
                return None;
            }
        }
        if want.len() == 0 {
            return Some(from + i + c.len_utf8());
        }
    }
    None
}

/// The blocks of text an item draws as, in order; empty for items with no
/// searchable text (tool calls draw their result's).
pub fn blocks(item: &Item) -> Vec<String> {
    match item {
        Item::User { text, .. } => vec![text.clone()],
        Item::Assistant(text) => {
            let mut out = Vec::new();
            flatten(&markdown::parse(text), &mut out);
            out
        }
        Item::Thinking(t) => vec![t.text.clone()],
        Item::ToolResult { content, .. } => vec![content.clone()],
        Item::Error(e) => vec![e.clone()],
        _ => Vec::new(),
    }
}

fn flatten(doc: &[Block], out: &mut Vec<String>) {
    for b in doc {
        match b {
            Block::Paragraph(i) | Block::Heading(_, i) => out.push(i.text.clone()),
            Block::Code { code, .. } => out.push(code.clone()),
            Block::Quote(d) => flatten(d, out),
            Block::List(items) => items.iter().for_each(|i| flatten(&i.blocks, out)),
            Block::Table { rows, .. } => rows.iter().flat_map(|r| &r.cells).for_each(|c| flatten(c, out)),
            Block::Rule => {}
        }
    }
}

/// Every match of `query` in `items`, top to bottom. Blocks over
/// `MAX_BLOCK` bytes (huge tool output) are searched in their first part only.
pub fn find(items: &[Item], query: &str) -> Vec<Hit> {
    if query.trim().is_empty() {
        return Vec::new();
    }
    let mut hits = Vec::new();
    for (ix, item) in items.iter().enumerate() {
        let n: usize = blocks(item).iter().map(|b| occurrences(head(b), query).len()).sum();
        hits.extend((0..n).map(|nth| Hit { item: ix, nth }));
    }
    hits
}

/// Bytes of one block searched at most.
const MAX_BLOCK: usize = 512 * 1024;

fn head(s: &str) -> &str {
    if s.len() <= MAX_BLOCK {
        return s;
    }
    let mut end = MAX_BLOCK;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_any_case() {
        assert_eq!(occurrences("Fix the Terminal; terminal!", "TERMINAL"), [8..16, 18..26]);
        assert_eq!(occurrences("aaa", "aa"), vec![0..2]);
        assert!(occurrences("abc", "").is_empty());
        assert!(occurrences("abc", "abcd").is_empty());
    }

    #[test]
    fn unicode_case_and_boundaries() {
        assert_eq!(occurrences("Ärger ärger", "ÄRGER"), [0..6, 7..13]);
        assert_eq!(occurrences("é é", "é"), [0..2, 3..5]);
        // `İ` lowercases to two chars; it doesn't break the scan.
        assert_eq!(occurrences("İx ix", "ix"), vec![4..6]);
    }

    #[test]
    fn counts_per_item_in_drawn_blocks() {
        let items = vec![
            Item::User { text: "where is the parser?".into(), images: Vec::new() },
            Item::Assistant("The **parser** is in `parser.rs`.\n\n- a parser list item\n\n```\nparser()\n```".into()),
            Item::Error("no match here".into()),
        ];
        let hits = find(&items, "parser");
        assert_eq!(hits.iter().filter(|h| h.item == 0).count(), 1);
        // Bold and code markers don't count: the drawn text has 4 matches.
        assert_eq!(hits.iter().filter(|h| h.item == 1).count(), 4);
        assert_eq!(hits.last(), Some(&Hit { item: 1, nth: 3 }));
        assert!(find(&items, "  ").is_empty());
    }
}
