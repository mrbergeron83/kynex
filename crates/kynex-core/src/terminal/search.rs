//! Find in the scrollback, and copy mode (vi-style motions and selection over the grid).
//! Both work on the terminal's grid in its own coordinates: history lines are negative.

use alacritty_terminal::Term;
use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::TermMode;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::vi_mode::ViMotion;

use std::ops::Range;

use super::SelectBy;

/// Non-overlapping case-insensitive byte ranges of `query` in `line`.
fn occurrences(line: &str, query: &str) -> Vec<Range<usize>> {
    if query.is_empty() || line.is_empty() {
        return Vec::new();
    }
    let needle = query.to_lowercase();
    if needle.is_empty() {
        return Vec::new();
    }
    let mut lower = String::with_capacity(line.len());
    let mut starts = Vec::with_capacity(line.len());
    let mut ends = Vec::with_capacity(line.len());
    for (orig_start, ch) in line.char_indices() {
        let orig_end = orig_start + ch.len_utf8();
        for lc in ch.to_lowercase() {
            let before = lower.len();
            lower.push(lc);
            for _ in before..lower.len() {
                starts.push(orig_start);
                ends.push(orig_end);
            }
        }
    }
    let mut out = Vec::new();
    let mut pos = 0;
    while let Some(rel) = lower[pos..].find(&needle) {
        let start = pos + rel;
        let end = start + needle.len();
        out.push(starts[start]..ends[end - 1]);
        pos = end;
    }
    out
}

/// One match on a single row: grid line (negative in the scrollback) and the columns it
/// covers, `end` exclusive (a wide character covers two).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Match {
    pub line: i32,
    pub start: usize,
    pub end: usize,
}

/// The matches of the last search and which one is current.
#[derive(Clone, Debug, Default)]
pub(super) struct FindState {
    pub matches: Vec<Match>,
    pub current: usize,
}

/// A copy-mode cursor move.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CopyMotion {
    Up,
    Down,
    Left,
    Right,
    LineStart,
    LineEnd,
    FirstNonBlank,
    WordForward,
    WordBack,
    WordEnd,
    Top,
    Bottom,
    HalfPageUp,
    HalfPageDown,
}

/// Every place `query` appears (ignoring case) in the scrollback and screen, oldest first.
/// Matches don't span rows. Wide characters count two columns; their spacer cells are skipped.
pub(super) fn find<T: EventListener>(term: &Term<T>, query: &str) -> Vec<Match> {
    let grid = term.grid();
    let mut out = Vec::new();
    if query.is_empty() {
        return out;
    }
    let top = -(grid.history_size() as i32);
    for l in top..grid.screen_lines() as i32 {
        let row = &grid[Line(l)];
        let mut text = String::new();
        // Byte offset, column and width of each character cell.
        let mut cells: Vec<(usize, usize, usize)> = Vec::new();
        for c in 0..grid.columns() {
            let cell = &row[Column(c)];
            if cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                continue;
            }
            cells.push((text.len(), c, if cell.flags.contains(Flags::WIDE_CHAR) { 2 } else { 1 }));
            text.push(if cell.c == '\0' { ' ' } else { cell.c });
            if let Some(extra) = cell.zerowidth() {
                text.extend(extra);
            }
        }
        for r in occurrences(&text, query) {
            let first = cells.partition_point(|&(b, ..)| b <= r.start).saturating_sub(1);
            let last = cells.partition_point(|&(b, ..)| b < r.end).saturating_sub(1);
            out.push(Match { line: l, start: cells[first].1, end: cells[last].1 + cells[last].2 });
        }
    }
    out
}

/// Scrolls so the match is on screen, centred when it wasn't; in copy mode the cursor goes
/// to its start.
pub(super) fn reveal<T: EventListener>(term: &mut Term<T>, m: &Match) {
    let offset = term.grid().display_offset() as i32;
    let rows = term.screen_lines() as i32;
    if m.line < -offset || m.line >= rows - offset {
        term.scroll_display(Scroll::Delta(rows / 2 - m.line - offset));
    }
    if copy_mode(term) {
        term.vi_goto_point(Point::new(Line(m.line), Column(m.start)));
    }
}

pub(super) fn copy_mode<T>(term: &Term<T>) -> bool {
    term.mode().contains(TermMode::VI)
}

/// Enters or leaves copy mode; leaving drops the selection.
pub(super) fn toggle_copy_mode<T: EventListener>(term: &mut Term<T>) {
    term.toggle_vi_mode();
    if !copy_mode(term) {
        term.selection = None;
    }
}

/// Moves the copy-mode cursor (no-op outside copy mode), scrolling to keep it on screen.
pub(super) fn copy_motion<T: EventListener>(term: &mut Term<T>, motion: CopyMotion) {
    if !copy_mode(term) {
        return;
    }
    let vi = |m| Some(m);
    let step = match motion {
        CopyMotion::Up => vi(ViMotion::Up),
        CopyMotion::Down => vi(ViMotion::Down),
        CopyMotion::Left => vi(ViMotion::Left),
        CopyMotion::Right => vi(ViMotion::Right),
        CopyMotion::LineStart => vi(ViMotion::First),
        CopyMotion::LineEnd => vi(ViMotion::Last),
        CopyMotion::FirstNonBlank => vi(ViMotion::FirstOccupied),
        CopyMotion::WordForward => vi(ViMotion::SemanticRight),
        CopyMotion::WordBack => vi(ViMotion::SemanticLeft),
        CopyMotion::WordEnd => vi(ViMotion::SemanticRightEnd),
        _ => None,
    };
    if let Some(m) = step {
        term.vi_motion(m);
    } else {
        let at = term.vi_mode_cursor.point;
        let top = -(term.history_size() as i32);
        let bottom = term.screen_lines() as i32 - 1;
        let half = (term.screen_lines() as i32 / 2).max(1);
        let to = match motion {
            CopyMotion::Top => Point::new(Line(top), Column(0)),
            CopyMotion::Bottom => Point::new(Line(bottom), Column(0)),
            CopyMotion::HalfPageUp => Point::new(Line((at.line.0 - half).max(top)), at.column),
            _ => Point::new(Line((at.line.0 + half).min(bottom)), at.column),
        };
        term.vi_goto_point(to);
    }
    let point = term.vi_mode_cursor.point;
    term.scroll_to_point(point);
}

/// Starts a selection of the given kind at the copy-mode cursor, or ends the one of that kind.
pub(super) fn copy_select<T: EventListener>(term: &mut Term<T>, by: SelectBy) {
    if !copy_mode(term) {
        return;
    }
    let ty = match by {
        SelectBy::Cell => SelectionType::Simple,
        SelectBy::Word => SelectionType::Semantic,
        SelectBy::Line => SelectionType::Lines,
    };
    let point = term.vi_mode_cursor.point;
    match term.selection.as_mut() {
        Some(sel) if sel.ty == ty && !sel.is_empty() => term.selection = None,
        Some(sel) if !sel.is_empty() => sel.ty = ty,
        _ => term.selection = Some(Selection::new(ty, point, Side::Left)),
    }
    // The first cell counts as selected.
    if let Some(sel) = term.selection.as_mut() {
        sel.include_all();
    }
}

#[cfg(test)]
mod tests {
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::term::Config;
    use alacritty_terminal::vte::ansi::Processor;

    use super::super::Size;
    use super::super::snapshot::{snapshot, tests::test_palette};
    use super::*;

    fn term(bytes: &[u8]) -> Term<VoidListener> {
        let mut term = Term::new(Config::default(), &Size { columns: 20, lines: 4 }, VoidListener);
        let mut parser: Processor = Processor::new();
        parser.advance(&mut term, bytes);
        term
    }

    /// Twelve numbered lines: the first eight are in the scrollback of a 4-line screen.
    fn numbered() -> Term<VoidListener> {
        let text: String = (0..12).map(|i| format!("line {i}\r\n")).collect();
        term(text.as_bytes())
    }

    #[test]
    fn finds_matches_in_the_scrollback_ignoring_case() {
        let t = numbered();
        assert!(t.grid().history_size() > 0);
        let found = find(&t, "LINE 1");
        // line 1, line 10 and line 11.
        assert_eq!(found.len(), 3);
        assert!(found[0].line < 0, "{found:?}");
        assert_eq!((found[0].start, found[0].end), (0, 6));
        assert!(found.windows(2).all(|w| w[0].line < w[1].line));
        assert!(find(&t, "").is_empty());
        assert!(find(&t, "nope").is_empty());
    }

    #[test]
    fn several_matches_on_a_row_do_not_overlap() {
        let t = term(b"aaaa a");
        let found = find(&t, "aa");
        assert_eq!(found.iter().map(|m| (m.start, m.end)).collect::<Vec<_>>(), [(0, 2), (2, 4)]);
    }

    #[test]
    fn wide_characters_count_two_columns() {
        let t = term("日本語 本".as_bytes());
        let found = find(&t, "本");
        assert_eq!(found.iter().map(|m| (m.start, m.end)).collect::<Vec<_>>(), [(2, 4), (7, 9)]);
        // A match across a wide character starts and ends on its cells.
        let found = find(&t, "本語");
        assert_eq!(found.iter().map(|m| (m.start, m.end)).collect::<Vec<_>>(), [(2, 6)]);
        // The text after the wide characters keeps its real columns.
        let t = term("日x".as_bytes());
        assert_eq!((find(&t, "x")[0].start, find(&t, "x")[0].end), (2, 3));
    }

    #[test]
    fn reveal_scrolls_a_hidden_match_into_view() {
        let mut t = numbered();
        let m = find(&t, "line 1 ")[0];
        assert_eq!(t.grid().display_offset(), 0);
        reveal(&mut t, &m);
        let offset = t.grid().display_offset() as i32;
        assert!(offset > 0);
        assert!(m.line >= -offset && m.line < 4 - offset);
        // Already visible: it stays.
        reveal(&mut t, &m);
        assert_eq!(t.grid().display_offset() as i32, offset);
        // In copy mode the cursor goes to the match.
        toggle_copy_mode(&mut t);
        let m2 = find(&t, "line 2 ")[0];
        reveal(&mut t, &m2);
        assert_eq!(t.vi_mode_cursor.point, Point::new(Line(m2.line), Column(m2.start)));
    }

    #[test]
    fn matches_are_tinted_in_the_snapshot() {
        let t = term(b"foo bar foo");
        let p = test_palette();
        let state = FindState { matches: find(&t, "foo"), current: 1 };
        let s = snapshot(&t, &p, Some(&state), None);
        let bg_at =
            |col: usize| s.rows[0].runs.iter().find(|r| r.column <= col && col < r.column + r.cells).unwrap().bg;
        assert_eq!(bg_at(1), Some(p.find));
        assert_eq!(bg_at(5), None);
        assert_eq!(bg_at(9), Some(p.find_current));
        // A scrolled-back match shows on the row it's drawn on.
        let mut t = numbered();
        let state = FindState { matches: find(&t, "line 2"), current: 0 };
        reveal(&mut t, &state.matches[0]);
        let s = snapshot(&t, &p, Some(&state), None);
        let row = s.rows.iter().position(|r| r.text.starts_with("line 2")).unwrap();
        assert_eq!(s.rows[row].runs[0].bg, Some(p.find_current));
    }

    #[test]
    fn copy_mode_selects_and_yanks_text() {
        let mut t = term(b"alpha beta\r\ngamma");
        copy_motion(&mut t, CopyMotion::Top);
        assert!(!copy_mode(&t));
        toggle_copy_mode(&mut t);
        assert!(copy_mode(&t));
        copy_motion(&mut t, CopyMotion::Top);
        copy_select(&mut t, SelectBy::Cell);
        copy_motion(&mut t, CopyMotion::WordEnd);
        assert_eq!(t.selection_to_string().as_deref(), Some("alpha"));
        // Another word: the selection grows.
        copy_motion(&mut t, CopyMotion::WordForward);
        copy_motion(&mut t, CopyMotion::WordEnd);
        assert_eq!(t.selection_to_string().as_deref(), Some("alpha beta"));
        // Line-wise, from the line below.
        copy_select(&mut t, SelectBy::Line);
        assert_eq!(t.selection.as_ref().unwrap().ty, SelectionType::Lines);
        toggle_copy_mode(&mut t);
        assert!(!copy_mode(&t) && t.selection.is_none());
    }

    #[test]
    fn copy_mode_reaches_the_scrollback() {
        let mut t = numbered();
        toggle_copy_mode(&mut t);
        copy_motion(&mut t, CopyMotion::Top);
        assert!(t.grid().display_offset() > 0);
        assert_eq!(t.vi_mode_cursor.point.line.0, -(t.history_size() as i32));
        // The cursor is drawn on the viewport's first row.
        let s = snapshot(&t, &test_palette(), None, None);
        assert_eq!(s.cursor.map(|c| (c.line, c.column)), Some((0, 0)));
        copy_select(&mut t, SelectBy::Cell);
        copy_motion(&mut t, CopyMotion::LineEnd);
        assert_eq!(t.selection_to_string().as_deref(), Some("line 0"));
        copy_motion(&mut t, CopyMotion::HalfPageDown);
        assert!(t.vi_mode_cursor.point.line.0 > -(t.history_size() as i32));
        copy_motion(&mut t, CopyMotion::Bottom);
        assert_eq!(t.grid().display_offset(), 0);
        assert_eq!(t.vi_mode_cursor.point.line.0, 3);
        copy_motion(&mut t, CopyMotion::HalfPageUp);
        assert_eq!(t.vi_mode_cursor.point.line.0, 1);
    }

    #[test]
    fn motions_do_nothing_outside_copy_mode() {
        let mut t = numbered();
        copy_motion(&mut t, CopyMotion::Top);
        copy_select(&mut t, SelectBy::Cell);
        assert_eq!(t.grid().display_offset(), 0);
        assert!(t.selection.is_none());
    }
}
