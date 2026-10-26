//! What's on screen, ready to draw: rows of styled runs, colours resolved,
//! the selection and cursor applied. Built under the terminal lock, drawn
//! without it.

use alacritty_terminal::Term;
use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::TermMode;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn hex(v: u32) -> Self {
        Rgb { r: (v >> 16) as u8, g: (v >> 8) as u8, b: v as u8 }
    }

    /// `self` moved `t` (0..1) of the way towards `other`.
    pub fn mix(self, other: Rgb, t: f32) -> Rgb {
        let m = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
        Rgb { r: m(self.r, other.r), g: m(self.g, other.g), b: m(self.b, other.b) }
    }
}

/// The colours a terminal draws with when programs don't pick their own.
#[derive(Clone, Debug, PartialEq)]
pub struct Palette {
    /// Black, red, green, yellow, blue, magenta, cyan, white, then the bright eight.
    pub ansi: [Rgb; 16],
    pub foreground: Rgb,
    pub background: Rgb,
    pub cursor: Rgb,
    pub selection: Rgb,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    /// UTF-8 length of this run's slice of [`Row::text`].
    pub len: usize,
    /// First cell, and how many cells it covers.
    pub column: usize,
    pub cells: usize,
    /// A single double-width character.
    pub wide: bool,
    pub fg: Rgb,
    /// `None` for the terminal's own background (nothing to paint).
    pub bg: Option<Rgb>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Row {
    pub text: String,
    pub runs: Vec<Run>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorStyle {
    Block,
    Beam,
    Underline,
    Hollow,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cursor {
    pub line: usize,
    pub column: usize,
    pub style: CursorStyle,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub rows: Vec<Row>,
    pub columns: usize,
    /// Hidden while scrolled back or when the program hides it.
    pub cursor: Option<Cursor>,
    /// Lines scrolled back into history; 0 at the bottom.
    pub display_offset: usize,
    pub mode_app_cursor: bool,
    pub mode_bracketed_paste: bool,
    pub mode_mouse: bool,
}

pub(super) fn snapshot<T: EventListener>(term: &Term<T>, palette: &Palette) -> Snapshot {
    let content = term.renderable_content();
    let offset = content.display_offset as i32;
    let columns = term.grid().columns();
    let lines = term.grid().screen_lines();
    let mut rows: Vec<Row> = vec![Row::default(); lines];
    let selection = content.selection;
    for indexed in content.display_iter {
        let cell = indexed.cell;
        if cell.flags.contains(Flags::WIDE_CHAR_SPACER) || cell.flags.contains(Flags::LEADING_WIDE_CHAR_SPACER) {
            continue;
        }
        let line = (indexed.point.line.0 + offset) as usize;
        let Some(row) = rows.get_mut(line) else { continue };
        let mut fg = resolve(cell.fg, content.colors, palette);
        let mut bg = resolve(cell.bg, content.colors, palette);
        if cell.flags.contains(Flags::INVERSE) {
            std::mem::swap(&mut fg, &mut bg);
        }
        if cell.flags.contains(Flags::DIM) {
            fg = fg.mix(bg, 0.35);
        }
        if cell.flags.contains(Flags::HIDDEN) {
            fg = bg;
        }
        if selection.is_some_and(|s| s.contains(indexed.point)) {
            bg = palette.selection;
        }
        let run = Run {
            len: 0,
            column: indexed.point.column.0,
            cells: if cell.flags.contains(Flags::WIDE_CHAR) { 2 } else { 1 },
            wide: cell.flags.contains(Flags::WIDE_CHAR),
            fg,
            bg: (bg != palette.background).then_some(bg),
            bold: cell.flags.contains(Flags::BOLD),
            italic: cell.flags.contains(Flags::ITALIC),
            underline: cell.flags.intersects(Flags::ALL_UNDERLINES),
            strike: cell.flags.contains(Flags::STRIKEOUT),
        };
        let c = if cell.c == '\0' { ' ' } else { cell.c };
        // Extend the last run when the style matches and it's contiguous; wide
        // characters always get their own, so each sits on its own two cells.
        let extend = row.runs.last().is_some_and(|last| {
            !run.wide && !last.wide && last.column + last.cells == run.column && same_style(last, &run)
        });
        row.text.push(c);
        if let Some(extra) = cell.zerowidth() {
            row.text.extend(extra);
        }
        let added = row.text.len() - row.runs.iter().map(|r| r.len).sum::<usize>();
        if extend {
            let last = row.runs.last_mut().unwrap();
            last.len += added;
            last.cells += 1;
        } else {
            row.runs.push(Run { len: added, ..run });
        }
    }
    let show_cursor = content.mode.contains(TermMode::SHOW_CURSOR) && content.display_offset == 0;
    let cursor = (show_cursor && content.cursor.shape != CursorShape::Hidden).then(|| Cursor {
        line: (content.cursor.point.line.0 + offset).max(0) as usize,
        column: content.cursor.point.column.0,
        style: match content.cursor.shape {
            CursorShape::Beam => CursorStyle::Beam,
            CursorShape::Underline => CursorStyle::Underline,
            CursorShape::HollowBlock => CursorStyle::Hollow,
            _ => CursorStyle::Block,
        },
    });
    Snapshot {
        rows,
        columns,
        cursor,
        display_offset: content.display_offset,
        mode_app_cursor: content.mode.contains(TermMode::APP_CURSOR),
        mode_bracketed_paste: content.mode.contains(TermMode::BRACKETED_PASTE),
        mode_mouse: content.mode.intersects(TermMode::MOUSE_MODE),
    }
}

fn same_style(a: &Run, b: &Run) -> bool {
    a.fg == b.fg
        && a.bg == b.bg
        && a.bold == b.bold
        && a.italic == b.italic
        && a.underline == b.underline
        && a.strike == b.strike
}

/// A cell colour as RGB: the program's own palette changes win, then ours.
fn resolve(color: Color, colors: &Colors, p: &Palette) -> Rgb {
    let from = |rgb: alacritty_terminal::vte::ansi::Rgb| Rgb { r: rgb.r, g: rgb.g, b: rgb.b };
    match color {
        Color::Spec(rgb) => from(rgb),
        Color::Named(n) => colors[n].map(from).unwrap_or_else(|| named(n, p)),
        Color::Indexed(i) => colors[i as usize].map(from).unwrap_or_else(|| indexed(i, p)),
    }
}

fn named(n: NamedColor, p: &Palette) -> Rgb {
    let i = n as usize;
    if i < 16 {
        return p.ansi[i];
    }
    match n {
        NamedColor::Background => p.background,
        NamedColor::Cursor => p.cursor,
        NamedColor::DimForeground => p.foreground.mix(p.background, 0.35),
        NamedColor::DimBlack
        | NamedColor::DimRed
        | NamedColor::DimGreen
        | NamedColor::DimYellow
        | NamedColor::DimBlue
        | NamedColor::DimMagenta
        | NamedColor::DimCyan
        | NamedColor::DimWhite => {
            let base = n as usize - NamedColor::DimBlack as usize;
            p.ansi[base].mix(p.background, 0.35)
        }
        _ => p.foreground,
    }
}

/// xterm's 256 colours: the 16 named ones, a 6×6×6 cube, then 24 greys.
fn indexed(i: u8, p: &Palette) -> Rgb {
    match i {
        0..=15 => p.ansi[i as usize],
        16..=231 => {
            let i = i - 16;
            let level = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            Rgb { r: level(i / 36), g: level(i / 6 % 6), b: level(i % 6) }
        }
        _ => {
            let v = 8 + (i - 232) * 10;
            Rgb { r: v, g: v, b: v }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::index::{Column, Line, Point, Side};
    use alacritty_terminal::selection::{Selection, SelectionType};
    use alacritty_terminal::term::Config;
    use alacritty_terminal::vte::ansi::Processor;

    use super::super::Size;
    use super::*;

    fn palette() -> Palette {
        test_palette()
    }

    pub(crate) fn test_palette() -> Palette {
        Palette {
            ansi: [
                0x000000, 0xcd3131, 0x0dbc79, 0xe5e510, 0x2472c8, 0xbc3fbc, 0x11a8cd, 0xe5e5e5, 0x666666, 0xf14c4c,
                0x23d18b, 0xf5f543, 0x3b8eea, 0xd670d6, 0x29b8db, 0xffffff,
            ]
            .map(Rgb::hex),
            foreground: Rgb::hex(0xcccccc),
            background: Rgb::hex(0x1e1e1e),
            cursor: Rgb::hex(0xffffff),
            selection: Rgb::hex(0x264f78),
        }
    }

    fn term(bytes: &[u8]) -> Term<VoidListener> {
        let mut term = Term::new(Config::default(), &Size { columns: 20, lines: 4 }, VoidListener);
        let mut parser: Processor = Processor::new();
        parser.advance(&mut term, bytes);
        term
    }

    #[test]
    fn plain_text_is_one_run() {
        let s = snapshot(&term(b"hello"), &palette());
        assert_eq!(s.rows.len(), 4);
        assert!(s.rows[0].text.starts_with("hello"));
        assert_eq!(s.rows[0].runs.len(), 1);
        assert_eq!(s.rows[0].runs[0].cells, 20);
        assert_eq!(s.rows[0].runs[0].bg, None);
        assert_eq!(s.cursor, Some(Cursor { line: 0, column: 5, style: CursorStyle::Block }));
    }

    #[test]
    fn colours_split_runs() {
        let s = snapshot(&term(b"a\x1b[31;1mb\x1b[0m\x1b[48;2;1;2;3mc\x1b[38;5;196md"), &palette());
        let runs = &s.rows[0].runs;
        assert_eq!(runs[0].fg, Rgb::hex(0xcccccc));
        assert_eq!((runs[1].fg, runs[1].bold, runs[1].column), (Rgb::hex(0xcd3131), true, 1));
        assert_eq!(runs[2].bg, Some(Rgb { r: 1, g: 2, b: 3 }));
        assert_eq!(runs[3].fg, Rgb { r: 255, g: 0, b: 0 });
        // Byte lengths add up to the row's text.
        assert_eq!(runs.iter().map(|r| r.len).sum::<usize>(), s.rows[0].text.len());
    }

    #[test]
    fn wide_characters_take_two_cells() {
        let s = snapshot(&term("a日b".as_bytes()), &palette());
        let runs = &s.rows[0].runs;
        assert_eq!((runs[1].column, runs[1].cells), (1, 2));
        assert_eq!(runs[2].column, 3);
    }

    #[test]
    fn selection_is_highlighted() {
        let mut t = term(b"select me");
        let mut sel = Selection::new(SelectionType::Simple, Point::new(Line(0), Column(0)), Side::Left);
        sel.update(Point::new(Line(0), Column(5)), Side::Right);
        t.selection = Some(sel);
        let s = snapshot(&t, &palette());
        assert_eq!(s.rows[0].runs[0].bg, Some(palette().selection));
        assert_eq!(s.rows[0].runs[0].cells, 6);
        assert_eq!(t.selection_to_string().as_deref(), Some("select"));
    }

    #[test]
    fn colour_cube() {
        assert_eq!(indexed(16, &palette()), Rgb { r: 0, g: 0, b: 0 });
        assert_eq!(indexed(231, &palette()), Rgb { r: 255, g: 255, b: 255 });
        assert_eq!(indexed(232, &palette()), Rgb { r: 8, g: 8, b: 8 });
    }
}
