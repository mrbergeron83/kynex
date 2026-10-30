//! Line numbers for an edit's diff. The Edit tool gives the old and new text
//! but not where they sit, so the file says: where the new text is (once the
//! edit landed) or the old text (while it waits for a yes).

/// The 1-based line of `file` where the edited text starts: `new`'s line if
/// it's in the file once, else `old`'s. `None` when neither is found exactly
/// once (or both are empty), so a guess never shows.
pub fn start_line(file: &str, old: &str, new: &str) -> Option<u32> {
    [new, old].into_iter().filter(|s| !s.is_empty()).find_map(|s| {
        let mut found = file.match_indices(s);
        let (at, _) = found.next()?;
        found.next().is_none().then(|| line_of(file, at))
    })
}

fn line_of(text: &str, byte: usize) -> u32 {
    text[..byte].bytes().filter(|b| *b == b'\n').count() as u32 + 1
}

/// Line numbers for the lines of a diff (`' '` context, `'-'` removed, `'+'`
/// added, in order) whose first line is line `start` on both sides: removed
/// lines count on the old side, the rest on the new one.
pub fn number(start: u32, signs: impl IntoIterator<Item = char>) -> Vec<u32> {
    let (mut old, mut new) = (start, start);
    signs
        .into_iter()
        .map(|sign| match sign {
            '-' => {
                old += 1;
                old - 1
            }
            '+' => {
                new += 1;
                new - 1
            }
            _ => {
                old += 1;
                new += 1;
                new - 1
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_new_text_then_the_old() {
        let file = "a\nb\nfn new() {}\nc\n";
        assert_eq!(start_line(file, "fn old() {}", "fn new() {}"), Some(3));
        // Not applied yet: the old text is where it'll go.
        assert_eq!(start_line("x\nfn old() {}\n", "fn old() {}", "fn new() {}"), Some(2));
        // Twice, or nowhere: no guess.
        assert_eq!(start_line("dup\ndup\n", "", "dup"), None);
        assert_eq!(start_line("x\n", "y", "z"), None);
        assert_eq!(start_line("x\n", "", ""), None);
        // Starting mid-line counts that line.
        assert_eq!(start_line("let a = 1;\nlet b = 2;\n", "", "b = 2"), Some(2));
    }

    #[test]
    fn numbers_each_side() {
        // Context 10, 11; removed 12, 13 (old); added 12 (new); context 13 (new).
        assert_eq!(number(10, [' ', ' ', '-', '-', '+', ' ']), [10, 11, 12, 13, 12, 13]);
        assert_eq!(number(1, ['+', '+']), [1, 2]);
        assert!(number(5, []).is_empty());
    }
}
