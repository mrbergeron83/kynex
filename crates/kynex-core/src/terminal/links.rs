//! Clickable links in a terminal grid: OSC 8 hyperlinks, URLs (`http://`, `https://`),
//! and local file paths (`src/app/mod.rs:123`, `C:\x\y.rs:4`, `main.rs#L12`) across wrapped rows.

use std::ops::Range;
use std::path::{Path, PathBuf};

use alacritty_terminal::Term;
use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::Flags;

/// What a clickable link in a terminal grid points to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkTarget {
    Url(String),
    File { path: PathBuf, line: Option<u32>, col: Option<u32> },
}

/// One visible horizontal span of a link on screen (0-based row, half-open column range).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LinkSpan {
    pub line: usize,
    pub start_col: usize,
    pub end_col: usize,
}

/// A link found under a visible terminal cell, along with all visible spans it covers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalLink {
    pub target: LinkTarget,
    pub spans: Vec<LinkSpan>,
}

/// Maximum wrapped rows to walk backward/forward around the hovered row.
const MAX_WRAP_BACK: i32 = 16;
const MAX_WRAP_SPAN: i32 = 32;

/// Finds a clickable link under visible `(screen_line, screen_col)` (0-based), if any.
/// Does not touch the disk; callers verify `LinkTarget::File` existence off the UI thread.
pub(super) fn link_at<T: EventListener>(
    term: &Term<T>,
    cwd: &Path,
    screen_line: usize,
    screen_col: usize,
) -> Option<TerminalLink> {
    let grid = term.grid();
    let cols = grid.columns();
    let screen_lines = grid.screen_lines();
    if screen_line >= screen_lines || screen_col >= cols {
        return None;
    }
    let offset = grid.display_offset() as i32;
    let target_line = screen_line as i32 - offset;

    if let Some(osc8) = osc8_link_at(term, cwd, target_line, screen_col, offset) {
        return Some(osc8);
    }

    let (first_line, last_line) = wrapped_bounds(term, target_line);
    let mut text = String::new();
    // Each character placed into `text`: `(byte_start, byte_end, grid_line, col_start, col_width)`.
    let mut map: Vec<(usize, usize, i32, usize, usize)> = Vec::new();
    let mut cursor_byte = None;

    for l in first_line..=last_line {
        let row = &grid[Line(l)];
        let is_wrapped_next = l < last_line;
        // Trim trailing empty cells on the final row of the logical line only.
        let row_end = if is_wrapped_next {
            cols
        } else {
            (0..cols)
                .rev()
                .find(|&c| {
                    let cell = &row[Column(c)];
                    !cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
                        && cell.c != '\0'
                        && cell.c != ' '
                })
                .map_or(0, |c| c + 1)
        };
        for c in 0..row_end {
            let cell = &row[Column(c)];
            if cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                continue;
            }
            let width = if cell.flags.contains(Flags::WIDE_CHAR) { 2 } else { 1 };
            let start = text.len();
            let ch = if cell.c == '\0' { ' ' } else { cell.c };
            text.push(ch);
            if let Some(extra) = cell.zerowidth() {
                text.extend(extra);
            }
            let end = text.len();
            if l == target_line && screen_col >= c && screen_col < c + width {
                cursor_byte = Some(start);
            }
            map.push((start, end, l, c, width));
        }
    }

    let byte = cursor_byte?;

    // URLs first so `https://example.com/src/main.rs` opens as a URL, not a local path.
    for range in find_urls(&text) {
        if range.start <= byte && byte < range.end {
            let spans = spans_for_range(&map, &range, offset, screen_lines);
            if !spans.is_empty() {
                return Some(TerminalLink { target: LinkTarget::Url(text[range].to_string()), spans });
            }
        }
    }

    for file_ref in crate::links::find(&text) {
        if file_ref.range.start <= byte && byte < file_ref.range.end {
            let spans = spans_for_range(&map, &file_ref.range, offset, screen_lines);
            if !spans.is_empty() {
                let path = crate::links::resolve(cwd, &file_ref.path);
                return Some(TerminalLink {
                    target: LinkTarget::File { path, line: file_ref.line, col: file_ref.col },
                    spans,
                });
            }
        }
    }

    None
}

/// Checks for an explicit OSC 8 hyperlink on `(target_line, col)`, ignoring internal `knx:` marks.
fn osc8_link_at<T: EventListener>(
    term: &Term<T>,
    cwd: &Path,
    target_line: i32,
    col: usize,
    offset: i32,
) -> Option<TerminalLink> {
    let grid = term.grid();
    let cols = grid.columns();
    let screen_lines = grid.screen_lines();
    let row = &grid[Line(target_line)];
    let hit_col = if row[Column(col)].flags.contains(Flags::WIDE_CHAR_SPACER) && col > 0 { col - 1 } else { col };
    let uri = row[Column(hit_col)].hyperlink()?.uri().to_string();
    if uri.starts_with("knx:") {
        return None;
    }
    let target = if uri.starts_with("http://") || uri.starts_with("https://") {
        LinkTarget::Url(uri.clone())
    } else {
        let f = crate::links::file_url(&uri)?;
        LinkTarget::File { path: crate::links::resolve(cwd, &f.path), line: f.line, col: f.col }
    };

    let (first_line, last_line) = wrapped_bounds(term, target_line);
    let mut spans = Vec::new();
    for l in first_line..=last_line {
        let vis = l + offset;
        if vis < 0 || (vis as usize) >= screen_lines {
            continue;
        }
        let r = &grid[Line(l)];
        let mut start = None;
        for c in 0..cols {
            let same = r[Column(c)].hyperlink().is_some_and(|h| h.uri() == uri);
            if same && start.is_none() {
                start = Some(c);
            } else if !same && let Some(s) = start.take() {
                spans.push(LinkSpan { line: vis as usize, start_col: s, end_col: c });
            }
        }
        if let Some(s) = start {
            spans.push(LinkSpan { line: vis as usize, start_col: s, end_col: cols });
        }
    }
    // Keep only the contiguous component containing `(target_line, hit_col)`.
    if spans.is_empty() {
        return None;
    }
    Some(TerminalLink { target, spans })
}

/// First and last grid lines belonging to the same wrapped logical line as `line`.
fn wrapped_bounds<T: EventListener>(term: &Term<T>, line: i32) -> (i32, i32) {
    let grid = term.grid();
    let cols = grid.columns();
    let min_line = -(grid.history_size() as i32);
    let max_line = grid.screen_lines() as i32 - 1;
    let mut first = line;
    while first > min_line
        && line - first < MAX_WRAP_BACK
        && grid[Line(first - 1)][Column(cols - 1)].flags.contains(Flags::WRAPLINE)
    {
        first -= 1;
    }
    let mut last = line;
    while last < max_line
        && last - first < MAX_WRAP_SPAN
        && grid[Line(last)][Column(cols - 1)].flags.contains(Flags::WRAPLINE)
    {
        last += 1;
    }
    (first, last)
}

/// Converts a byte range in the logical line text into visible `LinkSpan`s.
fn spans_for_range(
    map: &[(usize, usize, i32, usize, usize)],
    range: &Range<usize>,
    offset: i32,
    screen_lines: usize,
) -> Vec<LinkSpan> {
    let mut spans: Vec<LinkSpan> = Vec::new();
    for &(start, end, grid_line, col, width) in map {
        if end <= range.start || start >= range.end {
            continue;
        }
        let vis = grid_line + offset;
        if vis < 0 || (vis as usize) >= screen_lines {
            continue;
        }
        let vis = vis as usize;
        let col_end = col + width;
        if let Some(last) = spans.last_mut()
            && last.line == vis
            && last.end_col == col
        {
            last.end_col = col_end;
        } else {
            spans.push(LinkSpan { line: vis, start_col: col, end_col: col_end });
        }
    }
    spans
}

/// Finds `http://` and `https://` URLs in `text`. Pure and char-boundary safe.
pub fn find_urls(text: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut search_from = 0;
    while search_from < text.len() {
        let rest = &text[search_from..];
        let next_http = rest.find("http://");
        let next_https = rest.find("https://");
        let rel = match (next_http, next_https) {
            (Some(a), Some(b)) => a.min(b),
            (Some(a), None) | (None, Some(a)) => a,
            (None, None) => break,
        };
        let start = search_from + rel;
        let mut end = start;
        for (i, c) in text[start..].char_indices() {
            if c.is_whitespace() || c.is_control() || matches!(c, '<' | '>' | '"' | '\'' | '`') {
                break;
            }
            end = start + i + c.len_utf8();
        }
        let trimmed_end = trim_url_end(&text[start..end]);
        let actual_end = start + trimmed_end;
        let scheme_len = if text[start..].starts_with("https://") { 8 } else { 7 };
        if actual_end > start + scheme_len {
            out.push(start..actual_end);
        }
        search_from = end.max(start + scheme_len);
    }
    out
}

/// Strips trailing sentence punctuation and unbalanced closing brackets from a URL candidate.
fn trim_url_end(mut url: &str) -> usize {
    while let Some(last) = url.chars().last() {
        let drop = match last {
            '.' | ',' | ';' | ':' | '!' | '?' => true,
            ')' => unbalanced(url, '(', ')'),
            ']' => unbalanced(url, '[', ']'),
            '}' => unbalanced(url, '{', '}'),
            _ => false,
        };
        if drop {
            url = &url[..url.len() - last.len_utf8()];
        } else {
            break;
        }
    }
    url.len()
}

fn unbalanced(s: &str, open: char, close: char) -> bool {
    let opens = s.chars().filter(|&c| c == open).count();
    let closes = s.chars().filter(|&c| c == close).count();
    closes > opens
}

#[cfg(test)]
mod tests {
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::term::Config;
    use alacritty_terminal::vte::ansi::Processor;

    use super::super::Size;
    use super::*;

    fn term(columns: usize, lines: usize, bytes: &[u8]) -> Term<VoidListener> {
        let mut term = Term::new(Config::default(), &Size { columns, lines }, VoidListener);
        let mut parser: Processor = Processor::new();
        parser.advance(&mut term, bytes);
        term
    }

    #[test]
    fn finds_urls_and_trims_punctuation() {
        let s = "see (https://en.wikipedia.org/wiki/Rust_(language)), and http://example.com/a.";
        let urls: Vec<&str> = find_urls(s).into_iter().map(|r| &s[r]).collect();
        assert_eq!(urls, ["https://en.wikipedia.org/wiki/Rust_(language)", "http://example.com/a"]);
    }

    #[test]
    fn detects_wrapped_url_across_rows() {
        let t = term(20, 4, b"open https://example.com/very/long/path here");
        // Row 0: "open https://example" (cols 5..20)
        // Row 1: ".com/very/long/path " (cols 0..19)
        let hit = link_at(&t, Path::new("/repo"), 1, 4).expect("wrapped URL");
        assert_eq!(hit.target, LinkTarget::Url("https://example.com/very/long/path".into()));
        assert_eq!(
            hit.spans,
            vec![LinkSpan { line: 0, start_col: 5, end_col: 20 }, LinkSpan { line: 1, start_col: 0, end_col: 19 }]
        );
    }

    #[test]
    fn detects_file_path_with_line_and_resolves_against_cwd() {
        let t = term(60, 4, b"error at src/app/mod.rs:42:7 in build");
        let hit = link_at(&t, Path::new("/repo"), 0, 14).expect("file link");
        assert_eq!(
            hit.target,
            LinkTarget::File { path: Path::new("/repo").join("src/app/mod.rs"), line: Some(42), col: Some(7) }
        );
        assert_eq!(hit.spans, vec![LinkSpan { line: 0, start_col: 9, end_col: 28 }]);
    }

    #[test]
    fn detects_osc8_and_ignores_internal_knx_marks() {
        let t = term(
            60,
            4,
            b"\x1b]8;;https://github.com/mrbergeron83/kynex\x1b\\Kynex\x1b]8;;\x1b\\ \x1b]8;;knx:out:1\x1b\\plain\x1b]8;;\x1b\\",
        );
        let hit = link_at(&t, Path::new("/repo"), 0, 2).expect("osc8");
        assert_eq!(hit.target, LinkTarget::Url("https://github.com/mrbergeron83/kynex".into()));
        assert_eq!(hit.spans, vec![LinkSpan { line: 0, start_col: 0, end_col: 5 }]);
        assert!(link_at(&t, Path::new("/repo"), 0, 7).is_none());
    }
}
