//! A caret and selection over read-only lines: what the file viewer moves with the keys
//! and drags with the mouse, and the text it copies. Positions are a row and a byte
//! offset in it, always on a character boundary.

use std::ops::Range;

/// The rows a caret moves over.
pub trait Lines {
    fn count(&self) -> usize;
    /// The row's text, or `None` for a row that isn't part of the file (a hunk header,
    /// a note): the caret steps over it and copying leaves it out.
    fn text(&self, row: usize) -> Option<&str>;
}

impl<S: AsRef<str>> Lines for [Option<S>] {
    fn count(&self) -> usize {
        self.len()
    }

    fn text(&self, row: usize) -> Option<&str> {
        self.get(row)?.as_ref().map(AsRef::as_ref)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Pos {
    pub row: usize,
    /// A byte offset in the row's text.
    pub col: usize,
}

impl Pos {
    pub fn new(row: usize, col: usize) -> Self {
        Self { row, col }
    }
}

/// A key's move.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Move {
    Left,
    Right,
    WordLeft,
    WordRight,
    Up,
    Down,
    /// Up or down this many rows.
    PageUp(usize),
    PageDown(usize),
    LineStart,
    LineEnd,
    DocStart,
    DocEnd,
}

/// The selection runs from `anchor` to `head`; the caret is at `head`. Empty when they meet.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Caret {
    pub anchor: Pos,
    pub head: Pos,
    /// The column (in characters) up / down aim for, kept across short rows.
    goal: Option<usize>,
}

impl Caret {
    pub fn at(pos: Pos) -> Self {
        Self { anchor: pos, head: pos, goal: None }
    }

    /// The first row with text, at its start.
    pub fn start(lines: &(impl Lines + ?Sized)) -> Self {
        Self::at(Pos::new(next_text_row(lines, 0, true).unwrap_or(0), 0))
    }

    /// Both ends in order, if anything is selected.
    pub fn range(&self) -> Option<(Pos, Pos)> {
        let (a, h) = (self.anchor, self.head);
        (a != h).then(|| if a <= h { (a, h) } else { (h, a) })
    }

    /// Moves the caret; `extend` keeps the anchor (Shift), otherwise the selection goes.
    pub fn move_by(&mut self, lines: &(impl Lines + ?Sized), m: Move, extend: bool) {
        // Left / right without Shift first collapse a selection to its side.
        if !extend && let Some((lo, hi)) = self.range() {
            match m {
                Move::Left => return self.collapse(lo),
                Move::Right => return self.collapse(hi),
                _ => {}
            }
        }
        let head = clamp(lines, self.head);
        let text = lines.text(head.row).unwrap_or("");
        let vertical = matches!(m, Move::Up | Move::Down | Move::PageUp(_) | Move::PageDown(_));
        let goal = self.goal.unwrap_or_else(|| text[..head.col].chars().count());
        let new = match m {
            Move::Left => match text[..head.col].chars().next_back() {
                Some(c) => Pos::new(head.row, head.col - c.len_utf8()),
                None => match next_text_row(lines, head.row.wrapping_sub(1), false) {
                    Some(r) if head.row > 0 => Pos::new(r, line_len(lines, r)),
                    _ => head,
                },
            },
            Move::Right => match text[head.col..].chars().next() {
                Some(c) => Pos::new(head.row, head.col + c.len_utf8()),
                None => match next_text_row(lines, head.row + 1, true) {
                    Some(r) => Pos::new(r, 0),
                    None => head,
                },
            },
            Move::WordLeft => match word_left(text, head.col) {
                Some(col) => Pos::new(head.row, col),
                None => return self.move_by(lines, Move::Left, extend),
            },
            Move::WordRight => match word_right(text, head.col) {
                Some(col) => Pos::new(head.row, col),
                None => return self.move_by(lines, Move::Right, extend),
            },
            Move::Up | Move::PageUp(_) => {
                let by = if let Move::PageUp(n) = m { n.max(1) } else { 1 };
                let target = head.row.saturating_sub(by);
                let row = next_text_row(lines, target, false)
                    .filter(|&r| r < head.row)
                    .or_else(|| next_text_row(lines, target, true).filter(|&r| r < head.row));
                match row {
                    Some(r) => Pos::new(r, col_for(lines, r, goal)),
                    // Already on the first row: to its start.
                    None => Pos::new(head.row, 0),
                }
            }
            Move::Down | Move::PageDown(_) => {
                let by = if let Move::PageDown(n) = m { n.max(1) } else { 1 };
                let target = (head.row + by).min(lines.count().saturating_sub(1));
                let row = next_text_row(lines, target, true)
                    .or_else(|| next_text_row(lines, target, false))
                    .filter(|&r| r > head.row);
                match row {
                    Some(r) => Pos::new(r, col_for(lines, r, goal)),
                    None => Pos::new(head.row, text.len()),
                }
            }
            Move::LineStart => Pos::new(head.row, 0),
            Move::LineEnd => Pos::new(head.row, text.len()),
            Move::DocStart => Caret::start(lines).head,
            Move::DocEnd => {
                let r = next_text_row(lines, lines.count().saturating_sub(1), false).unwrap_or(0);
                Pos::new(r, line_len(lines, r))
            }
        };
        self.head = new;
        if !extend {
            self.anchor = new;
        }
        self.goal = vertical.then_some(goal);
    }

    fn collapse(&mut self, at: Pos) {
        *self = Self::at(at);
    }

    /// Everything, from the first row's start to the last row's end.
    pub fn select_all(&mut self, lines: &(impl Lines + ?Sized)) {
        let start = Caret::start(lines).head;
        let mut end = *self;
        end.move_by(lines, Move::DocEnd, false);
        *self = Self { anchor: start, head: end.head, goal: None };
    }

    /// The pointer moved to `pos` while dragging: the selection follows it.
    pub fn drag_to(&mut self, lines: &(impl Lines + ?Sized), pos: Pos) {
        self.head = clamp(lines, pos);
        self.goal = None;
    }

    /// A double click: the word (or run of spaces or punctuation) at `pos`.
    pub fn select_word(lines: &(impl Lines + ?Sized), pos: Pos) -> Self {
        let pos = clamp(lines, pos);
        let text = lines.text(pos.row).unwrap_or("");
        let (lo, hi) = word_at(text, pos.col);
        Self { anchor: Pos::new(pos.row, lo), head: Pos::new(pos.row, hi), goal: None }
    }

    /// A triple click: the whole row, up to the start of the next one.
    pub fn select_line(lines: &(impl Lines + ?Sized), row: usize) -> Self {
        let row = row.min(lines.count().saturating_sub(1));
        let end = match next_text_row(lines, row + 1, true) {
            Some(next) => Pos::new(next, 0),
            None => Pos::new(row, line_len(lines, row)),
        };
        Self { anchor: Pos::new(row, 0), head: end, goal: None }
    }

    /// The selected bytes of `row`, whose text is `len` bytes long.
    pub fn row_range(&self, row: usize, len: usize) -> Option<Range<usize>> {
        let (lo, hi) = self.range()?;
        if row < lo.row || row > hi.row {
            return None;
        }
        let start = if row == lo.row { lo.col.min(len) } else { 0 };
        let end = if row == hi.row { hi.col.min(len) } else { len };
        Some(start..end.max(start))
    }

    /// Whether the selection runs on past the end of `row` into the next one.
    pub fn takes_newline(&self, row: usize) -> bool {
        self.range().is_some_and(|(lo, hi)| row >= lo.row && row < hi.row)
    }
}

/// The selected text, rows joined with `\n`; rows that aren't the file's left out.
pub fn selected_text(lines: &(impl Lines + ?Sized), caret: &Caret) -> Option<String> {
    let (lo, hi) = caret.range()?;
    let mut parts = Vec::new();
    for row in lo.row..=hi.row.min(lines.count().saturating_sub(1)) {
        let Some(text) = lines.text(row) else { continue };
        let r = caret.row_range(row, text.len()).unwrap_or(0..0);
        if row == hi.row && r.is_empty() && row != lo.row {
            // Ends at the very start of this row: only the newline before it was taken.
            parts.push("");
            continue;
        }
        parts.push(text.get(r).unwrap_or(""));
    }
    let text = parts.join("\n");
    (!text.is_empty()).then_some(text)
}

/// `pos` moved onto a row with text and a character boundary in it.
fn clamp(lines: &(impl Lines + ?Sized), pos: Pos) -> Pos {
    let last = lines.count().saturating_sub(1);
    let row = pos.row.min(last);
    let row = if lines.text(row).is_some() {
        row
    } else {
        next_text_row(lines, row, true).or_else(|| next_text_row(lines, row, false)).unwrap_or(row)
    };
    let text = lines.text(row).unwrap_or("");
    let mut col = pos.col.min(text.len());
    while !text.is_char_boundary(col) {
        col -= 1;
    }
    Pos::new(row, col)
}

/// The first row from `from` (itself included) with text, going down or up.
fn next_text_row(lines: &(impl Lines + ?Sized), from: usize, down: bool) -> Option<usize> {
    let n = lines.count();
    if from >= n {
        return if down { None } else { next_text_row(lines, n.checked_sub(1)?, false) };
    }
    if down {
        (from..n).find(|&r| lines.text(r).is_some())
    } else {
        (0..=from).rev().find(|&r| lines.text(r).is_some())
    }
}

fn line_len(lines: &(impl Lines + ?Sized), row: usize) -> usize {
    lines.text(row).map_or(0, str::len)
}

/// The byte offset of character column `goal` in `row`, or its end.
fn col_for(lines: &(impl Lines + ?Sized), row: usize, goal: usize) -> usize {
    let text = lines.text(row).unwrap_or("");
    text.char_indices().nth(goal).map_or(text.len(), |(i, _)| i)
}

fn class(c: char) -> u8 {
    if c.is_alphanumeric() || c == '_' {
        0
    } else if c.is_whitespace() {
        1
    } else {
        2
    }
}

/// The word (or run of punctuation, or of spaces) around byte `ix`.
pub fn word_at(text: &str, ix: usize) -> (usize, usize) {
    let ix = (0..=ix.min(text.len())).rev().find(|i| text.is_char_boundary(*i)).unwrap_or(0);
    let Some(c) = text[ix..].chars().next().or_else(|| text[..ix].chars().next_back()) else { return (ix, ix) };
    let k = class(c);
    let start = text[..ix].char_indices().rev().take_while(|(_, c)| class(*c) == k).last().map_or(ix, |(i, _)| i);
    let end = text[ix..].char_indices().find(|(_, c)| class(*c) != k).map_or(text.len(), |(i, _)| ix + i);
    (start, end.max(start))
}

/// The start of the word before `col`, skipping spaces; `None` at the row's start.
fn word_left(text: &str, col: usize) -> Option<usize> {
    let before = &text[..col];
    let trimmed = before.trim_end();
    let c = trimmed.chars().next_back()?;
    let k = class(c);
    Some(trimmed.char_indices().rev().take_while(|(_, c)| class(*c) == k).last().map_or(0, |(i, _)| i))
}

/// The end of the word after `col`, skipping spaces; `None` at the row's end.
fn word_right(text: &str, col: usize) -> Option<usize> {
    let after = &text[col..];
    let skipped = after.len() - after.trim_start().len();
    let rest = &after[skipped..];
    let c = rest.chars().next()?;
    let k = class(c);
    let len = rest.char_indices().find(|(_, c)| class(*c) != k).map_or(rest.len(), |(i, _)| i);
    Some(col + skipped + len)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc() -> Vec<Option<&'static str>> {
        vec![None, Some("fn main() {"), Some("    let x = 1;"), None, Some("é"), Some("}")]
    }

    fn moved(c: &mut Caret, lines: &[Option<&str>], m: Move) -> Pos {
        c.move_by(lines, m, false);
        c.head
    }

    #[test]
    fn starts_on_the_first_text_row() {
        assert_eq!(Caret::start(doc().as_slice()).head, Pos::new(1, 0));
    }

    #[test]
    fn left_and_right_wrap_over_rows_that_arent_text() {
        let d = doc();
        let mut c = Caret::at(Pos::new(4, 0));
        assert_eq!(moved(&mut c, &d, Move::Left), Pos::new(2, 14));
        assert_eq!(moved(&mut c, &d, Move::Right), Pos::new(4, 0));
        assert_eq!(moved(&mut c, &d, Move::Right), Pos::new(4, 2), "é is two bytes");
        assert_eq!(moved(&mut c, &d, Move::Right), Pos::new(5, 0));
        assert_eq!(moved(&mut c, &d, Move::Right), Pos::new(5, 1));
        assert_eq!(moved(&mut c, &d, Move::Right), Pos::new(5, 1), "stays at the end");
        let mut c = Caret::at(Pos::new(1, 0));
        assert_eq!(moved(&mut c, &d, Move::Left), Pos::new(1, 0), "stays at the start");
    }

    #[test]
    fn up_and_down_keep_the_goal_column() {
        let d = doc();
        let mut c = Caret::at(Pos::new(2, 12));
        assert_eq!(moved(&mut c, &d, Move::Down), Pos::new(4, 2), "short row: its end");
        assert_eq!(moved(&mut c, &d, Move::Up), Pos::new(2, 12), "back to the goal");
        assert_eq!(moved(&mut c, &d, Move::Up), Pos::new(1, 11));
        assert_eq!(moved(&mut c, &d, Move::Up), Pos::new(1, 0), "top row: its start");
        let mut c = Caret::at(Pos::new(5, 0));
        assert_eq!(moved(&mut c, &d, Move::Down), Pos::new(5, 1), "last row: its end");
        let mut c = Caret::at(Pos::new(1, 3));
        assert_eq!(moved(&mut c, &d, Move::PageDown(10)), Pos::new(5, 1));
        assert_eq!(moved(&mut c, &d, Move::PageUp(10)), Pos::new(1, 3), "the goal survives the short row");
    }

    #[test]
    fn words_home_end_and_document_ends() {
        let d = doc();
        let mut c = Caret::at(Pos::new(2, 0));
        assert_eq!(moved(&mut c, &d, Move::WordRight), Pos::new(2, 7));
        assert_eq!(moved(&mut c, &d, Move::WordRight), Pos::new(2, 9));
        assert_eq!(moved(&mut c, &d, Move::WordLeft), Pos::new(2, 8));
        assert_eq!(moved(&mut c, &d, Move::LineEnd), Pos::new(2, 14));
        assert_eq!(moved(&mut c, &d, Move::WordRight), Pos::new(4, 0), "past the end: next row");
        assert_eq!(moved(&mut c, &d, Move::LineStart), Pos::new(4, 0));
        assert_eq!(moved(&mut c, &d, Move::DocEnd), Pos::new(5, 1));
        assert_eq!(moved(&mut c, &d, Move::DocStart), Pos::new(1, 0));
    }

    #[test]
    fn shift_extends_and_plain_moves_collapse() {
        let d = doc();
        let mut c = Caret::at(Pos::new(1, 3));
        c.move_by(d.as_slice(), Move::Right, true);
        c.move_by(d.as_slice(), Move::Right, true);
        assert_eq!(c.range(), Some((Pos::new(1, 3), Pos::new(1, 5))));
        c.move_by(d.as_slice(), Move::Left, false);
        assert_eq!((c.range(), c.head), (None, Pos::new(1, 3)));
    }

    #[test]
    fn copies_across_rows_without_the_others() {
        let d = doc();
        let mut c = Caret::at(Pos::new(1, 3));
        c.drag_to(d.as_slice(), Pos::new(4, 2));
        assert_eq!(selected_text(d.as_slice(), &c).unwrap(), "main() {\n    let x = 1;\né");
        let mut all = Caret::default();
        all.select_all(d.as_slice());
        assert_eq!(selected_text(d.as_slice(), &all).unwrap(), "fn main() {\n    let x = 1;\né\n}");
        let line = Caret::select_line(d.as_slice(), 2);
        assert_eq!(selected_text(d.as_slice(), &line).unwrap(), "    let x = 1;\n");
        assert!(line.takes_newline(2) && !line.takes_newline(4));
        assert_eq!(selected_text(d.as_slice(), &Caret::at(Pos::new(1, 1))), None);
    }

    #[test]
    fn rows_ranges_and_words() {
        let d = doc();
        let w = Caret::select_word(d.as_slice(), Pos::new(2, 8));
        assert_eq!(w.range(), Some((Pos::new(2, 8), Pos::new(2, 9))));
        let mut c = Caret::at(Pos::new(1, 3));
        c.drag_to(d.as_slice(), Pos::new(2, 4));
        assert_eq!(c.row_range(1, 11), Some(3..11));
        assert_eq!(c.row_range(2, 14), Some(0..4));
        assert_eq!(c.row_range(4, 2), None);
        // A drag onto a row that isn't text lands on the next one with text.
        c.drag_to(d.as_slice(), Pos::new(3, 50));
        assert_eq!(c.head, Pos::new(4, 2));
        // Inside a character: back to its start.
        c.drag_to(d.as_slice(), Pos::new(4, 1));
        assert_eq!(c.head, Pos::new(4, 0));
    }

    #[test]
    fn empty_documents_dont_panic() {
        let d: Vec<Option<&str>> = vec![];
        let mut c = Caret::start(d.as_slice());
        for m in [Move::Left, Move::Right, Move::Up, Move::Down, Move::DocEnd, Move::WordLeft, Move::PageDown(5)] {
            c.move_by(d.as_slice(), m, true);
        }
        c.select_all(d.as_slice());
        let _ = Caret::select_line(d.as_slice(), 3);
        let _ = Caret::select_word(d.as_slice(), Pos::new(2, 2));
        assert_eq!(selected_text(d.as_slice(), &c), None);
    }
}
