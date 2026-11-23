//! Find in the chat transcript (Ctrl+F): where a query appears in text,
//! case-insensitively (Unicode-aware), as the old transcript's find did.

use std::ops::Range;

/// Where `query` appears in `text`, ignoring case, as byte ranges of `text`,
/// left to right and not overlapping.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_every_match_in_any_case() {
        assert_eq!(occurrences("Fix the build, fix lints", "FIX"), vec![0..3, 15..18]);
        assert_eq!(occurrences("Ünïcode ÜNÏ", "ünï"), vec![0..5, 10..15]);
        assert_eq!(occurrences("aaa", "aa"), vec![0..2], "no overlaps");
        assert!(occurrences("text", "").is_empty());
        assert!(occurrences("", "x").is_empty());
    }
}
