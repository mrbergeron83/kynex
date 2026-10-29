//! The languages that aren't C-like: shells, data files, HTML and CSS.

use super::langs::{PS_KEYWORDS, SHELL_KEYWORDS, find};
use super::{Lx, State, Token, is_ident, is_space};

/// Bash and friends (`ps` false) or PowerShell: comments, strings, variables,
/// and the command word of each pipeline step as a function.
pub(super) fn shell(lx: &mut Lx, mut st: State, ps: bool) -> State {
    let mut cmd = true;
    while lx.i < lx.len() {
        let start = lx.i;
        let c = lx.b[start];
        if c.is_ascii_whitespace() {
            lx.i += 1;
            continue;
        }
        let token_start = start == 0 || matches!(lx.b[start - 1], b' ' | b'\t' | b';' | b'|' | b'&' | b'(' | b'{');
        if ps && lx.at("<#") {
            lx.i += 2;
            st = lx.comment(start, "<#", "#>", false, 1);
            continue;
        }
        if c == b'#' && token_start {
            lx.rest(start, Token::Comment);
            break;
        }
        if ps && c == b'@' && matches!(lx.peek(1), b'\'' | b'"') && lx.b[start + 2..].iter().all(|c| is_space(*c)) {
            let quote = lx.peek(1);
            lx.rest(start, Token::String);
            st = State::Here { quote };
            break;
        }
        match c {
            b'\'' | b'"' => {
                let esc = match (c, ps) {
                    (b'\'', _) => 0,
                    (_, true) => b'`',
                    _ => b'\\',
                };
                lx.i += 1;
                st = lx.string(start, c, false, esc, true);
                cmd = false;
            }
            b'$' => {
                let n = lx.peek(1);
                if n == b'{' {
                    lx.i += 2;
                    lx.skip(|c| c != b'}');
                    lx.i += 1;
                    lx.emit(start, Token::Type);
                    cmd = false;
                } else if n == b'(' {
                    lx.i += 2;
                    lx.emit(start, Token::Punctuation);
                    cmd = true;
                } else if is_ident(n) {
                    lx.i += 1;
                    lx.skip(|c| is_ident(c) || (ps && c == b':'));
                    let w = lx.word(start + 1);
                    let constant = ps && find(&["true", "false", "null"], w, true);
                    lx.emit(start, if constant { Token::Keyword } else { Token::Type });
                    cmd = false;
                } else if matches!(n, b'?' | b'#' | b'@' | b'*' | b'!' | b'$') || (!ps && n == b'-') {
                    lx.i += 2;
                    lx.emit(start, Token::Type);
                    cmd = false;
                } else {
                    lx.punct();
                }
            }
            b'|' | b';' | b'&' | b'(' | b'{' | b'}' => {
                lx.punct();
                cmd = true;
            }
            b'=' if ps => {
                lx.punct();
                cmd = true;
            }
            b'[' if ps && lx.peek(1).is_ascii_alphabetic() => {
                lx.punct();
                let name = lx.i;
                lx.skip(|c| is_ident(c) || c == b'.');
                lx.emit(name, Token::Type);
            }
            _ if c.is_ascii_punctuation() && !word_byte(c, ps) => {
                lx.punct();
                if c == b')' {
                    cmd = false;
                }
            }
            _ => {
                if ps && c == b'.' {
                    lx.punct();
                }
                let w_start = lx.i;
                lx.skip(|c| word_byte(c, ps));
                if lx.i == w_start {
                    lx.i += 1;
                    continue;
                }
                cmd = shell_word(lx, w_start, ps, cmd);
            }
        }
    }
    st
}

/// A byte that belongs to a bare shell word.
fn word_byte(c: u8, ps: bool) -> bool {
    !(c.is_ascii_whitespace()
        || b"|;&()<>\"'$`{}".contains(&c)
        || (ps && b",=[]".contains(&c))
        || (!ps && b"[]".contains(&c)))
}

/// Colours a bare word; returns whether the next word is a command.
fn shell_word(lx: &mut Lx, start: usize, ps: bool, cmd: bool) -> bool {
    let w = lx.word(start);
    let keywords = if ps { PS_KEYWORDS } else { SHELL_KEYWORDS };
    if w.bytes().all(|c| c.is_ascii_digit()) {
        lx.emit(start, Token::Number);
        false
    } else if find(keywords, w, ps) {
        lx.emit(start, Token::Keyword);
        !ps && matches!(w, "if" | "then" | "else" | "elif" | "do" | "while" | "until")
    } else if !ps
        && cmd
        && let Some(eq) = w.find('=')
        && eq > 0
        && w.as_bytes()[..eq].iter().all(|c| is_ident(*c))
    {
        lx.push(start, start + eq, Token::Type);
        lx.push(start + eq, start + eq + 1, Token::Punctuation);
        true
    } else if (ps && verb_noun(w)) || (cmd && !w.starts_with('-')) {
        lx.emit(start, Token::Function);
        matches!(w, "sudo" | "doas" | "env" | "exec" | "nohup" | "xargs" | "command" | "builtin" | "time" | "watch")
    } else if ps && lx.peek(0) == b'(' {
        lx.emit(start, Token::Function);
        false
    } else {
        false
    }
}

/// A PowerShell cmdlet name: `Get-ChildItem`.
fn verb_noun(w: &str) -> bool {
    let Some((verb, noun)) = w.split_once('-') else { return false };
    !verb.is_empty()
        && verb.bytes().all(|c| c.is_ascii_alphabetic())
        && noun.bytes().next().is_some_and(|c| c.is_ascii_alphabetic())
        && noun.bytes().all(|c| c.is_ascii_alphanumeric())
}

/// JSON (comments allowed): keys as types, values as strings.
pub(super) fn json(lx: &mut Lx, mut st: State) -> State {
    while lx.i < lx.len() {
        let start = lx.i;
        let c = lx.b[start];
        if c.is_ascii_whitespace() {
            lx.i += 1;
        } else if lx.at("//") {
            lx.rest(start, Token::Comment);
        } else if lx.at("/*") {
            lx.i += 2;
            st = lx.comment(start, "/*", "*/", false, 1);
        } else if c == b'"' {
            lx.i += 1;
            lx.scan_str(b'"', false, b'\\');
            let key = lx.next_solid() == b':';
            lx.emit(start, if key { Token::Type } else { Token::String });
        } else if c.is_ascii_digit() || (c == b'-' && lx.peek(1).is_ascii_digit()) {
            lx.i += 1;
            lx.skip(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'+' | b'-'));
            lx.emit(start, Token::Number);
        } else if is_ident(c) {
            lx.skip(is_ident);
            if matches!(lx.word(start), "true" | "false" | "null") {
                lx.emit(start, Token::Keyword);
            }
        } else if c.is_ascii_punctuation() {
            lx.punct();
        } else {
            lx.i += 1;
        }
    }
    st
}

/// TOML: table headers and keys as types, values by kind. `depth` counts the
/// brackets of a value still open from earlier lines (multi-line arrays).
pub(super) fn toml(lx: &mut Lx, mut st: State, depth: &mut u32) -> State {
    let first = lx.b.get(lx.i..).and_then(|r| r.iter().find(|c| !c.is_ascii_whitespace())).copied().unwrap_or(0);
    if *depth > 0 && first.is_ascii_alphabetic() && lx.b.contains(&b'=') {
        *depth = 0; // a key line: whatever was open was never closed
    }
    let header = lx.i == 0 && *depth == 0 && first == b'[';
    let mut key = lx.i == 0 && *depth == 0 && !header;
    // Brackets opened on this line, innermost in the low bit: 1 for `{`.
    let (mut stack, mut open) = (0u64, 0u32);
    while lx.i < lx.len() {
        let start = lx.i;
        let c = lx.b[start];
        match c {
            _ if c.is_ascii_whitespace() => lx.i += 1,
            b'#' => lx.rest(start, Token::Comment),
            b'"' | b'\'' => {
                let triple = lx.peek(1) == c && lx.peek(2) == c;
                lx.i += if triple { 3 } else { 1 };
                let esc = if c == b'"' { b'\\' } else { 0 };
                let closed = lx.scan_str(c, triple, esc);
                lx.emit(start, if key || header { Token::Type } else { Token::String });
                if !closed && triple {
                    st = State::Str { quote: c, triple, esc };
                }
            }
            b'=' => {
                lx.punct();
                key = false;
            }
            b'[' | b'{' if !header => {
                lx.punct();
                if open < 64 {
                    stack = (stack << 1) | u64::from(c == b'{');
                    open += 1;
                }
                key = c == b'{';
            }
            b']' | b'}' if !header => {
                lx.punct();
                if open > 0 {
                    stack >>= 1;
                    open -= 1;
                } else {
                    *depth = depth.saturating_sub(1);
                }
            }
            b',' => {
                lx.punct();
                key = open > 0 && stack & 1 == 1;
            }
            _ if !key
                && !header
                && (c.is_ascii_digit() || (matches!(c, b'+' | b'-') && lx.peek(1).is_ascii_digit())) =>
            {
                lx.i += 1;
                lx.skip(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b':' | b'-' | b'+'));
                lx.emit(start, Token::Number);
            }
            _ if is_ident(c) || c == b'-' => {
                lx.skip(|c| is_ident(c) || c == b'-');
                if key || header {
                    lx.emit(start, Token::Type);
                } else if matches!(lx.word(start), "true" | "false" | "inf" | "nan") {
                    lx.emit(start, Token::Keyword);
                }
            }
            _ if c.is_ascii_punctuation() => lx.punct(),
            _ => lx.i += 1,
        }
    }
    *depth = depth.saturating_add(open);
    st
}

/// YAML, a line at a time: keys as types, scalars by kind.
pub(super) fn yaml(lx: &mut Lx, st: State) -> State {
    let mut flow = 0u32;
    if (lx.at("---") || lx.at("...")) && matches!(lx.peek(3), 0 | b' ' | b'\t') {
        lx.i += 3;
        lx.emit(0, Token::Punctuation);
    }
    while lx.i < lx.len() {
        let start = lx.i;
        let c = lx.b[start];
        let ends = |c: u8| matches!(c, 0 | b' ' | b'\t');
        if c.is_ascii_whitespace() {
            lx.i += 1;
        } else if c == b'#' && (start == 0 || is_space(lx.b[start - 1])) {
            lx.rest(start, Token::Comment);
        } else if c == b'%' && start == 0 {
            lx.rest(start, Token::Keyword);
        } else if matches!(c, b'-' | b'?' | b':') && ends(lx.peek(1)) {
            lx.punct();
        } else if matches!(c, b'[' | b'{' | b']' | b'}' | b',') {
            match c {
                b'[' | b'{' => flow += 1,
                b']' | b'}' => flow = flow.saturating_sub(1),
                _ => {}
            }
            lx.punct();
        } else if c == b'"' || c == b'\'' {
            lx.i += 1;
            lx.scan_str(c, false, if c == b'"' { b'\\' } else { 0 });
            let key = lx.next_solid() == b':';
            lx.emit(start, if key { Token::Type } else { Token::String });
        } else if matches!(c, b'&' | b'*' | b'!') && !ends(lx.peek(1)) {
            lx.i += 1;
            lx.skip(|c| !c.is_ascii_whitespace() && !b",[]{}".contains(&c));
            lx.emit(start, if c == b'!' { Token::Keyword } else { Token::Function });
        } else if matches!(c, b'|' | b'>') {
            lx.i += 1;
            lx.skip(|c| c.is_ascii_digit() || matches!(c, b'+' | b'-'));
            lx.emit(start, Token::Punctuation);
        } else {
            let mut j = start;
            while j < lx.len() {
                let bj = lx.b[j];
                let next = lx.b.get(j + 1).copied().unwrap_or(0);
                if (bj == b':' && (ends(next) || (flow > 0 && b",]}".contains(&next))))
                    || (bj == b'#' && j > start && is_space(lx.b[j - 1]))
                    || (flow > 0 && b",[]{}".contains(&bj))
                {
                    break;
                }
                j += 1;
            }
            let key = lx.b.get(j) == Some(&b':');
            while j > start && is_space(lx.b[j - 1]) {
                j -= 1;
            }
            if j <= start {
                lx.i += 1;
                continue;
            }
            lx.i = j;
            let w = lx.word(start);
            let t = if key {
                Token::Type
            } else if yaml_number(w) {
                Token::Number
            } else if find(&["true", "false", "null", "yes", "no", "on", "off", "~"], w, true) {
                Token::Keyword
            } else {
                Token::String
            };
            lx.emit(start, t);
        }
    }
    st
}

fn yaml_number(w: &str) -> bool {
    let b = w.as_bytes();
    let digit_at = |i: usize| b.get(i).is_some_and(u8::is_ascii_digit);
    (digit_at(0) || (matches!(b.first(), Some(b'+' | b'-' | b'.')) && digit_at(1)))
        && b.iter().all(|c| c.is_ascii_alphanumeric() || b"._+-:".contains(c))
}

/// HTML and XML: tag names as keywords, attributes as types, values as strings.
pub(super) fn html(lx: &mut Lx, mut st: State) -> State {
    let mut after_eq = false;
    while lx.i < lx.len() {
        let start = lx.i;
        let c = lx.b[start];
        if st == State::Tag {
            if c.is_ascii_whitespace() {
                lx.i += 1;
            } else if c == b'>' {
                lx.punct();
                st = State::Normal;
            } else if (c == b'/' || c == b'?') && lx.peek(1) == b'>' {
                lx.i += 2;
                lx.emit(start, Token::Punctuation);
                st = State::Normal;
            } else if c == b'"' || c == b'\'' {
                lx.i += 1;
                lx.scan_str(c, false, 0);
                lx.emit(start, Token::String);
                after_eq = false;
            } else if c == b'=' {
                lx.punct();
                after_eq = true;
            } else if c == b'<' {
                st = State::Normal; // a tag never closed
            } else {
                lx.skip(|c| !c.is_ascii_whitespace() && !b">\"'=<".contains(&c));
                if lx.i == start {
                    lx.i += 1;
                }
                lx.emit(start, if after_eq { Token::String } else { Token::Type });
                after_eq = false;
            }
        } else if lx.at("<!--") {
            lx.i += 4;
            st = lx.comment(start, "<!--", "-->", false, 1);
        } else if c == b'<' && (lx.peek(1).is_ascii_alphabetic() || matches!(lx.peek(1), b'/' | b'!' | b'?')) {
            lx.i += if matches!(lx.peek(1), b'/' | b'!' | b'?') { 2 } else { 1 };
            lx.emit(start, Token::Punctuation);
            let name = lx.i;
            lx.skip(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b':' | b'.'));
            lx.emit(name, Token::Keyword);
            st = State::Tag;
            after_eq = false;
        } else if c == b'&' {
            lx.i += 1;
            lx.skip(|c| c.is_ascii_alphanumeric() || c == b'#');
            if lx.peek(0) == b';' && lx.i - start <= 12 {
                lx.i += 1;
                lx.emit(start, Token::Number);
            }
        } else {
            lx.i += 1;
        }
    }
    st
}

/// CSS (and SCSS / Less): selectors, properties, values, at-rules. `depth`
/// is the brace depth, carried across lines.
pub(super) fn css(lx: &mut Lx, mut st: State, depth: &mut u32) -> State {
    let name = |c: u8| is_ident(c) || c == b'-';
    while lx.i < lx.len() {
        let start = lx.i;
        let c = lx.b[start];
        let n = lx.peek(1);
        if c.is_ascii_whitespace() {
            lx.i += 1;
        } else if lx.at("/*") {
            lx.i += 2;
            st = lx.comment(start, "/*", "*/", false, 1);
        } else if lx.at("//") && (start == 0 || is_space(lx.b[start - 1])) {
            lx.rest(start, Token::Comment);
        } else if c == b'"' || c == b'\'' {
            lx.i += 1;
            lx.string(start, c, false, b'\\', false);
        } else if c == b'{' || c == b'}' {
            *depth = if c == b'{' { depth.saturating_add(1) } else { depth.saturating_sub(1) };
            lx.punct();
        } else if matches!(c, b'@' | b'!' | b'#' | b'$') && name(n) {
            lx.i += 1;
            lx.skip(name);
            let t = match c {
                b'@' | b'!' => Token::Keyword,
                b'#' if *depth > 0 => Token::Number,
                _ => Token::Type,
            };
            lx.emit(start, t);
        } else if c.is_ascii_digit() || (matches!(c, b'.' | b'-' | b'+') && n.is_ascii_digit()) {
            lx.i += 1;
            lx.skip(|c| c.is_ascii_alphanumeric() || c == b'.' || c == b'%');
            lx.emit(start, Token::Number);
        } else if c == b'.' && *depth == 0 && (is_ident(n) || n == b'-') {
            lx.i += 1;
            lx.skip(name);
            lx.emit(start, Token::Type);
        } else if is_ident(c) || (c == b'-' && (n.is_ascii_alphabetic() || matches!(n, b'-' | b'_'))) {
            lx.skip(name);
            if lx.peek(0) == b'(' {
                lx.emit(start, Token::Function);
            } else if *depth > 0 && lx.next_solid() == b':' {
                lx.emit(start, Token::Type);
            } else if *depth == 0 {
                lx.emit(start, Token::Keyword);
            }
        } else if c.is_ascii_punctuation() {
            lx.punct();
        } else {
            lx.i += 1;
        }
    }
    st
}
