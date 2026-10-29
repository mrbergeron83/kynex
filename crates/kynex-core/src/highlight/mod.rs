//! Syntax highlighting for code blocks and files: a small one-pass lexer with
//! per-language tables (keywords, comment and string forms), not a parser.
//! [`Highlighter`] carries what spans lines (block comments, multi-line
//! strings). It never panics, and its ranges always fall on char boundaries:
//! every token starts and ends at an ASCII byte or the end of the line.

mod langs;
mod markup;

use std::ops::Range;

pub use langs::Lang;
use langs::Spec;

/// What a highlighted range is; the app picks a colour per kind.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Token {
    Keyword,
    String,
    Type,
    Number,
    Comment,
    Function,
    Punctuation,
}

/// What an unfinished line leaves open for the next.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum State {
    Normal,
    Comment {
        open: &'static str,
        close: &'static str,
        nested: bool,
        depth: u32,
    },
    /// A string; `esc` is its escape byte or 0.
    Str {
        quote: u8,
        triple: bool,
        esc: u8,
    },
    /// A Rust raw string closed by `"` and this many `#`.
    Raw {
        hashes: usize,
    },
    /// A PowerShell here-string, closed by a line starting with `'@` / `"@`.
    Here {
        quote: u8,
    },
    /// Inside an HTML / XML tag.
    Tag,
}

/// Highlights a text line by line, carrying state from one line to the next.
pub struct Highlighter {
    lang: Lang,
    state: State,
    /// Brace depth (CSS) or open brackets of a value (TOML).
    depth: u32,
}

impl Highlighter {
    pub fn new(lang: Lang) -> Self {
        Highlighter { lang, state: State::Normal, depth: 0 }
    }

    /// Tokens of one line (without its newline; a trailing `\r` is ignored):
    /// byte ranges into `line`, sorted, not overlapping, on char boundaries.
    /// Plain text gets no token.
    pub fn line(&mut self, line: &str) -> Vec<(Range<usize>, Token)> {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let mut lx = Lx::new(line);
        let st = lx.resume(self.state);
        self.state = match self.lang {
            Lang::Shell => markup::shell(&mut lx, st, false),
            Lang::PowerShell => markup::shell(&mut lx, st, true),
            Lang::Json => markup::json(&mut lx, st),
            Lang::Toml => markup::toml(&mut lx, st, &mut self.depth),
            Lang::Yaml => markup::yaml(&mut lx, st),
            Lang::Html => markup::html(&mut lx, st),
            Lang::Css => markup::css(&mut lx, st, &mut self.depth),
            lang => code(&mut lx, st, lang),
        };
        let mut out = lx.out;
        out.retain(|(r, _)| line.is_char_boundary(r.start) && line.is_char_boundary(r.end));
        out
    }
}

/// Highlights a whole block: ranges into `code`, line by line (split on `\n`).
pub fn highlight(lang: Lang, code: &str) -> Vec<(Range<usize>, Token)> {
    let mut h = Highlighter::new(lang);
    let mut out = Vec::new();
    let mut offset = 0;
    for line in code.split('\n') {
        out.extend(h.line(line).into_iter().map(|(r, t)| (r.start + offset..r.end + offset, t)));
        offset += line.len() + 1;
    }
    out
}

/// Which side of a diff a line is on.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Side {
    Old,
    New,
    Both,
}

/// Each line's tokens for diff lines in order: removed lines (`Old`) carry the
/// old file's state (block comments, multi-line strings), added lines (`New`)
/// the new file's; context lines (`Both`) advance both and take the new side's
/// tokens.
pub fn diff<'a>(lang: Lang, lines: impl IntoIterator<Item = (Side, &'a str)>) -> Vec<Vec<(Range<usize>, Token)>> {
    let (mut old, mut new) = (Highlighter::new(lang), Highlighter::new(lang));
    lines
        .into_iter()
        .map(|(side, line)| match side {
            Side::Old => old.line(line),
            Side::New => new.line(line),
            Side::Both => {
                old.line(line);
                new.line(line)
            }
        })
        .collect()
}

/// A letter, digit, `_` or any non-ASCII byte (so words end only at ASCII).
fn is_ident(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c >= 0x80
}

fn is_space(c: u8) -> bool {
    c == b' ' || c == b'\t'
}

/// The cursor over one line and the tokens found so far.
struct Lx<'a> {
    s: &'a str,
    b: &'a [u8],
    i: usize,
    out: Vec<(Range<usize>, Token)>,
}

impl<'a> Lx<'a> {
    fn new(s: &'a str) -> Self {
        Lx { s, b: s.as_bytes(), i: 0, out: Vec::new() }
    }

    fn len(&self) -> usize {
        self.b.len()
    }

    /// The byte `n` past the cursor, or 0 past the end.
    fn peek(&self, n: usize) -> u8 {
        self.b.get(self.i + n).copied().unwrap_or(0)
    }

    fn at(&self, p: &str) -> bool {
        self.b.get(self.i..).is_some_and(|r| r.starts_with(p.as_bytes()))
    }

    /// The text from `from` to the cursor.
    fn word(&self, from: usize) -> &'a str {
        self.s.get(from..self.i).unwrap_or("")
    }

    fn skip(&mut self, f: impl Fn(u8) -> bool) {
        while self.i < self.b.len() && f(self.b[self.i]) {
            self.i += 1;
        }
    }

    /// The first byte at or after the cursor that isn't a space, or 0.
    fn next_solid(&self) -> u8 {
        self.b.get(self.i..).and_then(|r| r.iter().find(|c| !is_space(**c))).copied().unwrap_or(0)
    }

    /// Adds a token, joined to the previous one when they touch and match.
    fn push(&mut self, start: usize, end: usize, t: Token) {
        let end = end.min(self.b.len());
        let start = start.max(self.out.last().map_or(0, |(r, _)| r.end));
        if start >= end {
            return;
        }
        if let Some((r, last)) = self.out.last_mut()
            && *last == t
            && r.end == start
        {
            r.end = end;
            return;
        }
        self.out.push((start..end, t));
    }

    /// Adds a token from `start` to the cursor.
    fn emit(&mut self, start: usize, t: Token) {
        self.i = self.i.min(self.b.len());
        self.push(start, self.i, t);
    }

    /// Adds a token from `start` to the end of the line.
    fn rest(&mut self, start: usize, t: Token) {
        self.i = self.b.len();
        self.push(start, self.i, t);
    }

    /// One byte of punctuation.
    fn punct(&mut self) {
        self.i += 1;
        self.emit(self.i - 1, Token::Punctuation);
    }

    /// A block comment from `start`, the cursor past its opening (or at the
    /// line's start when it carries on).
    fn comment(
        &mut self,
        start: usize,
        open: &'static str,
        close: &'static str,
        nested: bool,
        mut depth: u32,
    ) -> State {
        while self.i < self.b.len() {
            if self.at(close) {
                self.i += close.len();
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    self.emit(start, Token::Comment);
                    return State::Normal;
                }
            } else if nested && self.at(open) {
                self.i += open.len();
                depth += 1;
            } else {
                self.i += 1;
            }
        }
        self.rest(start, Token::Comment);
        State::Comment { open, close, nested, depth: depth.max(1) }
    }

    /// Moves the cursor past a string's end; false if the line ends first.
    fn scan_str(&mut self, quote: u8, triple: bool, esc: u8) -> bool {
        while self.i < self.b.len() {
            let c = self.b[self.i];
            if esc != 0 && c == esc {
                self.i += 2;
            } else if c == quote && (!triple || (self.peek(1) == quote && self.peek(2) == quote)) {
                self.i += if triple { 3 } else { 1 };
                return true;
            } else {
                self.i += 1;
            }
        }
        self.i = self.b.len();
        false
    }

    /// A string from `start`, the cursor past its opening quote.
    fn string(&mut self, start: usize, quote: u8, triple: bool, esc: u8, multiline: bool) -> State {
        let closed = self.scan_str(quote, triple, esc);
        self.emit(start, Token::String);
        if closed || !(multiline || triple) { State::Normal } else { State::Str { quote, triple, esc } }
    }

    /// A Rust raw string from `start`, the cursor past its opening quote.
    fn raw(&mut self, start: usize, hashes: usize) -> State {
        while self.i < self.b.len() {
            let after = self.i + 1;
            if self.b[self.i] == b'"' && self.b.get(after..after + hashes).is_some_and(|h| h.iter().all(|c| *c == b'#'))
            {
                self.i = after + hashes;
                self.emit(start, Token::String);
                return State::Normal;
            }
            self.i += 1;
        }
        self.rest(start, Token::String);
        State::Raw { hashes }
    }

    /// Carries on what the previous line left open.
    fn resume(&mut self, st: State) -> State {
        match st {
            State::Normal | State::Tag => st,
            State::Comment { open, close, nested, depth } => self.comment(0, open, close, nested, depth),
            State::Str { quote, triple, esc } => self.string(0, quote, triple, esc, true),
            State::Raw { hashes } => self.raw(0, hashes),
            State::Here { quote } => {
                if self.b.starts_with(&[quote, b'@']) {
                    self.i = 2;
                    self.emit(0, Token::String);
                    State::Normal
                } else {
                    self.rest(0, Token::String);
                    st
                }
            }
        }
    }

    /// A number from the cursor: digits, `0x…`, `1_000`, `1.5e-3`, suffixes.
    fn number(&mut self) {
        let start = self.i;
        let radix = self.peek(0) == b'0' && matches!(self.peek(1), b'x' | b'X' | b'b' | b'B' | b'o' | b'O');
        let mut dot = self.peek(0) == b'.';
        self.i += 1;
        while self.i < self.b.len() {
            let c = self.b[self.i];
            if c.is_ascii_alphanumeric() || c == b'_' {
                self.i += 1;
                if !radix
                    && matches!(c, b'e' | b'E')
                    && matches!(self.peek(0), b'+' | b'-')
                    && self.peek(1).is_ascii_digit()
                {
                    self.i += 1;
                }
            } else if c == b'.' && !dot && !radix && self.peek(1).is_ascii_digit() {
                dot = true;
                self.i += 1;
            } else {
                break;
            }
        }
        self.emit(start, Token::Number);
    }
}

/// What the last keyword says the next word is.
#[derive(Clone, Copy, PartialEq)]
enum Def {
    None,
    Function,
    Type,
}

/// The C-like languages: Rust, JavaScript, Python, Go, the C family, SQL.
fn code(lx: &mut Lx, mut st: State, lang: Lang) -> State {
    let sp = langs::spec(lang);
    let mut def = Def::None;
    while lx.i < lx.len() {
        let start = lx.i;
        let c = lx.b[start];
        if c.is_ascii_whitespace() {
            lx.i += 1;
            continue;
        }
        if sp.line_comments.iter().any(|p| lx.at(p)) {
            lx.rest(start, Token::Comment);
            break;
        }
        if let Some((open, close)) = sp.block_comment
            && lx.at(open)
        {
            lx.i += open.len();
            st = lx.comment(start, open, close, sp.nested, 1);
            continue;
        }
        let ident = |c: u8| is_ident(c) || (c == sp.ident_extra && c != 0);
        if lang == Lang::Rust && c == b'\'' {
            rust_quote(lx);
            def = Def::None;
            continue;
        }
        if lang == Lang::C && c == b'#' && lx.b[..start].iter().all(|c| c.is_ascii_whitespace()) {
            lx.i += 1;
            lx.skip(is_space);
            lx.skip(is_ident);
            lx.emit(start, Token::Keyword);
            continue;
        }
        if sp.decorators && c == b'@' && ident(lx.peek(1)) && !lx.peek(1).is_ascii_digit() {
            lx.i += 1;
            lx.skip(|c| ident(c) || c == b'.');
            lx.emit(start, Token::Function);
            continue;
        }
        if sp.quotes.contains(&c) {
            st = quote(lx, sp, start, c);
            def = Def::None;
            continue;
        }
        let after_word = start > 0 && (ident(lx.b[start - 1]) || matches!(lx.b[start - 1], b')' | b']'));
        if c.is_ascii_digit() || (c == b'.' && lx.peek(1).is_ascii_digit() && !after_word) {
            lx.number();
            def = Def::None;
            continue;
        }
        if ident(c) {
            lx.skip(ident);
            let w = lx.word(start);
            if let Some(s) = prefixed_string(lx, sp, lang, start, w) {
                st = s;
                def = Def::None;
                continue;
            }
            let next = lx.peek(0);
            let token = if sp.is_keyword(w) {
                Some(Token::Keyword)
            } else if sp.is_type(w) || def == Def::Type {
                Some(Token::Type)
            } else if def == Def::Function || next == b'(' {
                Some(Token::Function)
            } else if lang == Lang::Rust && next == b'!' && matches!(lx.peek(1), b'(' | b'[' | b'{') {
                lx.i += 1;
                Some(Token::Function)
            } else if sp.caps_types && w.as_bytes()[0].is_ascii_uppercase() {
                let constant = w.len() > 1 && !w.bytes().any(|c| c.is_ascii_lowercase());
                Some(if constant { Token::Number } else { Token::Type })
            } else {
                None
            };
            def = match (token, w) {
                (Some(Token::Keyword), "fn" | "def" | "func" | "function" | "fun") => Def::Function,
                (Some(Token::Keyword), "class" | "struct" | "enum" | "trait" | "interface" | "union") => Def::Type,
                _ => Def::None,
            };
            if let Some(t) = token {
                lx.emit(start, t);
            }
            continue;
        }
        def = Def::None;
        if c.is_ascii_punctuation() {
            lx.punct();
        } else {
            lx.i += 1;
        }
    }
    st
}

/// A string opened by `c` at `start` (cursor still on it).
fn quote(lx: &mut Lx, sp: &Spec, start: usize, c: u8) -> State {
    let triple = sp.triple.contains(&c) && lx.peek(1) == c && lx.peek(2) == c;
    let esc = if sp.raw.contains(&c) { 0 } else { sp.esc };
    lx.i += if triple { 3 } else { 1 };
    lx.string(start, c, triple, esc, triple || sp.multiline.contains(&c))
}

/// A string with a prefix word: Rust `b"…"`, `r#"…"#`, Python `f"…"`, `rb'…'`.
fn prefixed_string(lx: &mut Lx, sp: &Spec, lang: Lang, start: usize, w: &str) -> Option<State> {
    let next = lx.peek(0);
    match lang {
        Lang::Rust => match (w, next) {
            ("b" | "c", b'"') => {
                lx.i += 1;
                Some(lx.string(start, b'"', false, b'\\', true))
            }
            ("b", b'\'') => {
                lx.i += 1;
                Some(lx.string(start, b'\'', false, b'\\', false))
            }
            ("r" | "br" | "cr", b'"' | b'#') => {
                let hashes = lx.b[lx.i..].iter().take_while(|c| **c == b'#').count();
                if lx.peek(hashes) != b'"' {
                    return None;
                }
                lx.i += hashes + 1;
                Some(lx.raw(start, hashes))
            }
            _ => None,
        },
        Lang::Python if w.len() <= 2 && matches!(next, b'"' | b'\'') && w.bytes().all(|c| b"rbfuRBFU".contains(&c)) => {
            Some(quote(lx, sp, start, next))
        }
        _ => None,
    }
}

/// Rust's `'`: a char literal (`'x'`, `'\n'`, `'é'`) or a lifetime (`'a`).
fn rust_quote(lx: &mut Lx) {
    let start = lx.i;
    if lx.peek(1) == b'\\' {
        lx.i += 1;
        lx.string(start, b'\'', false, b'\\', false);
        return;
    }
    let width = lx.s.get(start + 1..).and_then(|r| r.chars().next()).map_or(0, char::len_utf8);
    if width > 0 && lx.peek(1 + width) == b'\'' {
        lx.i += 2 + width;
        lx.emit(start, Token::String);
    } else if is_ident(lx.peek(1)) {
        lx.i += 1;
        lx.skip(is_ident);
        lx.emit(start, Token::Keyword);
    } else {
        lx.punct();
    }
}

#[cfg(test)]
mod tests;
