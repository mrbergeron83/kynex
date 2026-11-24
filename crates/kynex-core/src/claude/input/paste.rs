//! Long pastes folded into a chip, as the CLI does: the composer holds a short
//! `[Pasted text #1 +120 lines]` in the paste's place, and the full text goes out
//! when the message is sent. Clicking the chip puts the text back in place.

use std::collections::BTreeMap;
use std::ops::Range;

/// A paste with at least this many lines is folded…
pub const FOLD_LINES: usize = 10;
/// …and so is one with at least this many characters.
pub const FOLD_CHARS: usize = 1000;

/// Whether a paste of `text` is long enough to fold into a chip.
pub fn folds(text: &str) -> bool {
    text.lines().count() >= FOLD_LINES || text.chars().count() >= FOLD_CHARS
}

/// The pastes of one draft, by number.
#[derive(Clone, Debug, Default)]
pub struct Pastes {
    texts: BTreeMap<u32, String>,
    next: u32,
}

/// A chip found in the composer's text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chip {
    /// Its bytes in the text.
    pub range: Range<usize>,
    pub id: u32,
}

const OPEN: &str = "[Pasted text #";

impl Pastes {
    /// Keeps `text` and returns the chip to put in its place.
    pub fn fold(&mut self, text: String) -> String {
        self.next += 1;
        let chip = label(self.next, &text);
        self.texts.insert(self.next, text);
        chip
    }

    /// Whether any paste is kept.
    pub fn is_empty(&self) -> bool {
        self.texts.is_empty()
    }

    /// The chips of known pastes in `text`, in order.
    pub fn chips(&self, text: &str) -> Vec<Chip> {
        let mut out = Vec::new();
        let mut at = 0;
        while let Some(i) = text[at..].find(OPEN) {
            let start = at + i;
            let after = start + OPEN.len();
            let digits = text[after..].bytes().take_while(u8::is_ascii_digit).count();
            let close = text[after..].find(']').map(|j| after + j);
            match (text[after..after + digits].parse::<u32>(), close) {
                (Ok(id), Some(close)) if self.texts.get(&id).is_some_and(|t| text[start..=close] == label(id, t)) => {
                    out.push(Chip { range: start..close + 1, id });
                    at = close + 1;
                }
                _ => at = after,
            }
        }
        out
    }

    /// `text` with every chip replaced by its paste: what is sent.
    pub fn expand(&self, text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        let mut at = 0;
        for chip in self.chips(text) {
            out.push_str(&text[at..chip.range.start]);
            out.push_str(&self.texts[&chip.id]);
            at = chip.range.end;
        }
        out.push_str(&text[at..]);
        out
    }

    /// The paste behind chip `id`, to put back in its place.
    pub fn text(&self, id: u32) -> Option<&str> {
        self.texts.get(&id).map(String::as_str)
    }

    /// Forgets the pastes, for a new draft. Numbering goes on, as in the CLI.
    pub fn clear(&mut self) {
        self.texts.clear();
    }
}

/// The chip for paste `id`: its line count, or its length when it's one long line.
fn label(id: u32, text: &str) -> String {
    match text.lines().count() {
        n if n > 1 => format!("{OPEN}{id} +{n} lines]"),
        _ => format!("{OPEN}{id} +{} chars]", text.chars().count()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(n: usize) -> String {
        (1..=n).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn only_long_pastes_fold() {
        assert!(!folds("a short one"));
        assert!(!folds(&lines(FOLD_LINES - 1)));
        assert!(folds(&lines(FOLD_LINES)));
        assert!(folds(&"x".repeat(FOLD_CHARS)));
    }

    #[test]
    fn a_chip_says_how_long_its_paste_is() {
        let mut p = Pastes::default();
        assert_eq!(p.fold(lines(120)), "[Pasted text #1 +120 lines]");
        assert_eq!(p.fold("x".repeat(1500)), "[Pasted text #2 +1500 chars]");
    }

    #[test]
    fn sending_expands_every_chip() {
        let mut p = Pastes::default();
        let a = p.fold(lines(12));
        let b = p.fold(lines(15));
        let text = format!("look at {a} and {b} please");
        assert_eq!(p.expand(&text), format!("look at {} and {} please", lines(12), lines(15)));
        let chips = p.chips(&text);
        assert_eq!(chips.len(), 2);
        assert_eq!(&text[chips[0].range.clone()], a);
        assert_eq!(chips[1].id, 2);
    }

    #[test]
    fn typed_lookalikes_and_edited_chips_stay_text() {
        let mut p = Pastes::default();
        let chip = p.fold(lines(20));
        // A chip of no paste, a chip whose count was edited, a cut-off one.
        for text in ["[Pasted text #9 +20 lines]", "[Pasted text #1 +2 lines]", "[Pasted text #1", "[Pasted text #]"] {
            assert!(p.chips(text).is_empty(), "{text}");
            assert_eq!(p.expand(text), text);
        }
        assert_eq!(p.expand(&format!("[{chip}]")), format!("[{}]", lines(20)));
    }

    #[test]
    fn numbering_goes_on_after_a_clear() {
        let mut p = Pastes::default();
        p.fold(lines(10));
        p.clear();
        assert!(p.is_empty());
        assert_eq!(p.fold(lines(10)), "[Pasted text #2 +10 lines]");
        assert_eq!(p.text(2), Some(lines(10).as_str()));
    }
}
