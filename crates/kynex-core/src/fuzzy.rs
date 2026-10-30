//! Fuzzy matching as you type: a query's characters in order anywhere in a
//! text, scored so word starts and unbroken runs count most. Shared by the
//! `@file` mentions and the command palette, which add their own bonuses.
//!
//! Texts and queries are compared ASCII-lowercased (see [`lower`]), so byte
//! offsets found in the lowered text are valid in the original.

/// `s` lowercased without moving any byte: ASCII letters only.
pub fn lower(s: &str) -> String {
    s.to_ascii_lowercase()
}

/// Whether the character at byte `i` starts a word: the text's start, or after
/// a separator (`/ _ - . : space`).
pub fn word_start(text: &str, i: usize) -> bool {
    i == 0 || matches!(text.as_bytes()[i - 1], b'/' | b'_' | b'-' | b'.' | b':' | b' ')
}

/// Byte offsets of `query`'s characters in `text` from byte `from` on, taken in
/// order; `None` if they aren't all there. Each one jumps ahead to a word start
/// where there is one (`ac` → `app/composer`); if that leaves the rest
/// unmatched, plain first occurrences.
pub fn find(text: &str, query: &str, from: usize) -> Option<Vec<usize>> {
    let chars: Vec<(usize, char)> = text.get(from..)?.char_indices().map(|(i, c)| (i + from, c)).collect();
    find_in(text, &chars, query, true).or_else(|| find_in(text, &chars, query, false))
}

fn find_in(text: &str, chars: &[(usize, char)], query: &str, jump: bool) -> Option<Vec<usize>> {
    let mut positions: Vec<usize> = Vec::with_capacity(query.len());
    let mut at = 0;
    for q in query.chars() {
        let first = at + chars[at..].iter().position(|&(_, c)| c == q)?;
        // A match running straight on from the previous one stays put.
        let runs_on = !positions.is_empty() && at == first;
        let pick = if jump && !runs_on {
            chars[first..].iter().position(|&(i, c)| c == q && word_start(text, i)).map_or(first, |o| first + o)
        } else {
            first
        };
        positions.push(chars[pick].0);
        at = pick + 1;
    }
    Some(positions)
}

/// Whether `p` comes right after `last` in `text`.
fn follows(text: &str, last: Option<usize>, p: usize) -> bool {
    last.is_some_and(|l| p == l + text[l..].chars().next().map_or(1, char::len_utf8))
}

/// Whether every run of matched characters starts a word, as in `term` →
/// "Toggle **term**inal" or `ns` → "**N**ew **s**ession", not letters picked
/// from all over a sentence.
pub fn at_word_starts(text: &str, positions: &[usize]) -> bool {
    let mut last = None;
    positions.iter().all(|&p| {
        let ok = follows(text, last, p) || word_start(text, p);
        last = Some(p);
        ok
    })
}

/// The best match whose runs all start words ([`at_word_starts`]), trying each
/// word start the query's first character is at; `None` if there's none.
pub fn find_words(text: &str, query: &str) -> Option<Vec<usize>> {
    let first = query.chars().next()?;
    text.char_indices()
        .filter(|&(i, c)| c == first && word_start(text, i))
        .filter_map(|(i, _)| find(text, query, i).filter(|p| at_word_starts(text, p)))
        .max_by_key(|p| (score(text, p), std::cmp::Reverse(p[0])))
}

/// How good a match at `positions` in `text` is: word starts and runs score,
/// gaps between matched characters cost a little.
pub fn score(text: &str, positions: &[usize]) -> i64 {
    let mut score = 0i64;
    let mut last: Option<usize> = None;
    for &p in positions {
        if word_start(text, p) {
            score += 10;
        }
        if follows(text, last, p) {
            score += 8;
        } else if let Some(l) = last {
            score -= (p - l - 1).min(10) as i64;
        }
        last = Some(p);
    }
    score
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_word_starts() {
        assert_eq!(find("src/app/composer.rs", "ac", 0), Some(vec![4, 8]));
        assert_eq!(find("new session", "ns", 0), Some(vec![0, 4]));
        assert_eq!(find("abc", "abd", 0), None);
    }

    #[test]
    fn falls_back_when_a_jump_strands_the_rest() {
        assert_eq!(find("xcb_c", "cb", 0), Some(vec![1, 2]));
    }

    #[test]
    fn runs_and_word_starts_score_higher() {
        let run = score("toggle terminal", &find("toggle terminal", "term", 0).unwrap());
        let scattered = score("toggle terminal", &[1, 9, 12, 14]);
        assert!(run > scattered);
    }

    #[test]
    fn word_start_runs() {
        assert_eq!(find_words("toggle terminal", "term"), Some(vec![7, 8, 9, 10]));
        assert_eq!(find_words("new session", "ns"), Some(vec![0, 4]));
        assert_eq!(find_words("rewind to an earlier message", "term"), None);
        assert!(at_word_starts("ab cd", &[0, 1, 3]));
        assert!(!at_word_starts("ab cd", &[0, 4]));
    }

    #[test]
    fn non_ascii_is_safe() {
        assert_eq!(find("café ok", "éo", 0), Some(vec![3, 6]));
        assert!(score("café ok", &[3, 6]) > 0);
        assert_eq!(find("é", "x", 1), None);
    }
}
