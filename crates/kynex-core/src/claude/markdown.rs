//! Fast, panic-free CommonMark/GFM block and inline Markdown parser for Claude responses.
//!
//! Produces a clean AST suitable for virtualized UI rendering and plain-text extraction for
//! transcript search, with automatic `file:line` detection via [`crate::links`].

/// Maximum nesting depth for recursive blocks (blockquotes, lists) and inline formatting.
const MAX_DEPTH: usize = 16;

/// Table column alignment parsed from GFM delimiter rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Alignment {
    None,
    Left,
    Center,
    Right,
}

/// One item inside a [`Block::List`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListItem {
    /// `Some(true)` for `[x]` / `[X]`, `Some(false)` for `[ ]`, `None` for regular items.
    pub checked: Option<bool>,
    pub blocks: Vec<Block>,
}

/// Block-level Markdown AST node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    Heading { level: u8, inlines: Vec<Inline> },
    Paragraph(Vec<Inline>),
    CodeBlock { lang: Option<String>, code: String },
    BlockQuote(Vec<Block>),
    List { ordered: bool, start: u64, items: Vec<ListItem> },
    Table { headers: Vec<Vec<Inline>>, alignments: Vec<Alignment>, rows: Vec<Vec<Vec<Inline>>> },
    Rule,
    Image { alt: String, src: String },
}

/// Inline-level Markdown AST node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Inline {
    Text(String),
    Code(String),
    Bold(Vec<Inline>),
    Italic(Vec<Inline>),
    Strike(Vec<Inline>),
    Link { label: String, url: String },
    FileRef { raw: String, path: String, line: Option<u32> },
}

/// Parses Markdown text into a list of [`Block`]s. Never panics on untrusted input.
pub fn parse(input: &str) -> Vec<Block> {
    let lines: Vec<&str> = input.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)).collect();
    parse_blocks(&lines, 0)
}

/// Parses a single Markdown inline span into [`Inline`] nodes. Never panics on untrusted input.
pub fn parse_inline(input: &str) -> Vec<Inline> {
    inject_file_refs(parse_inline_inner(input, 0))
}

/// Extracts searchable plain text from parsed [`Block`]s (for transcript `Ctrl+F` find).
pub fn extract_plain_text(blocks: &[Block]) -> String {
    let mut out = String::new();
    append_blocks_text(blocks, &mut out);
    out
}

fn append_blocks_text(blocks: &[Block], out: &mut String) {
    for (idx, block) in blocks.iter().enumerate() {
        if idx > 0 && !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        match block {
            Block::Heading { inlines, .. } | Block::Paragraph(inlines) => append_inlines_text(inlines, out),
            Block::CodeBlock { code, .. } => out.push_str(code),
            Block::BlockQuote(inner) => append_blocks_text(inner, out),
            Block::List { items, .. } => {
                for (i, item) in items.iter().enumerate() {
                    if i > 0 && !out.ends_with('\n') {
                        out.push('\n');
                    }
                    append_blocks_text(&item.blocks, out);
                }
            }
            Block::Table { headers, rows, .. } => {
                append_row_text(headers, out);
                for row in rows {
                    out.push('\n');
                    append_row_text(row, out);
                }
            }
            Block::Rule => {}
            Block::Image { alt, src } => out.push_str(if alt.is_empty() { src } else { alt }),
        }
    }
}

fn append_row_text(cells: &[Vec<Inline>], out: &mut String) {
    for (i, cell) in cells.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        append_inlines_text(cell, out);
    }
}

fn append_inlines_text(inlines: &[Inline], out: &mut String) {
    for inline in inlines {
        match inline {
            Inline::Text(s) | Inline::Code(s) => out.push_str(s),
            Inline::Bold(in_) | Inline::Italic(in_) | Inline::Strike(in_) => append_inlines_text(in_, out),
            Inline::Link { label, .. } => out.push_str(label),
            Inline::FileRef { raw, .. } => out.push_str(raw),
        }
    }
}

fn parse_blocks(lines: &[&str], depth: usize) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut i = 0usize;

    while i < lines.len() {
        let line = lines[i];
        if line.trim().is_empty() {
            i += 1;
            continue;
        }
        if let Some((fc, flen, lang)) = parse_fence_open(line) {
            i += 1;
            let start = i;
            while i < lines.len() && !is_fence_close(lines[i], fc, flen) {
                i += 1;
            }
            blocks.push(Block::CodeBlock { lang, code: lines[start..i].join("\n") });
            if i < lines.len() {
                i += 1;
            }
            continue;
        }
        if let Some((level, text)) = parse_heading(line) {
            blocks.push(Block::Heading { level, inlines: parse_inline(text) });
            i += 1;
            continue;
        }
        if is_horizontal_rule(line) {
            blocks.push(Block::Rule);
            i += 1;
            continue;
        }
        if depth < MAX_DEPTH && is_blockquote_line(line) {
            let mut quoted = Vec::new();
            while i < lines.len() && is_blockquote_line(lines[i]) {
                quoted.push(strip_blockquote_prefix(lines[i]));
                i += 1;
            }
            blocks.push(Block::BlockQuote(parse_blocks(&quoted, depth + 1)));
            continue;
        }
        if let Some((table, next_i)) = try_parse_table(lines, i) {
            blocks.push(table);
            i = next_i;
            continue;
        }
        if depth < MAX_DEPTH
            && let Some((list, next_i)) = try_parse_list(lines, i, depth)
        {
            blocks.push(list);
            i = next_i;
            continue;
        }
        if let Some((alt, src)) = parse_standalone_image(line.trim()) {
            blocks.push(Block::Image { alt, src });
            i += 1;
            continue;
        }

        let start = i;
        while i < lines.len() && !lines[i].trim().is_empty() {
            if i > start
                && (is_block_starter(lines[i])
                    || (i + 1 < lines.len() && lines[i].contains('|') && parse_table_delimiter(lines[i + 1]).is_some()))
            {
                break;
            }
            i += 1;
        }
        let joined = join_paragraph_lines(&lines[start..i]);
        if let Some((alt, src)) = parse_standalone_image(joined.trim()) {
            blocks.push(Block::Image { alt, src });
        } else {
            blocks.push(Block::Paragraph(parse_inline(&joined)));
        }
    }

    blocks
}

fn try_parse_table(lines: &[&str], i: usize) -> Option<(Block, usize)> {
    if i + 1 >= lines.len() || !lines[i].contains('|') {
        return None;
    }
    let alignments = parse_table_delimiter(lines[i + 1])?;
    let header_cells = split_table_row(lines[i]);
    if header_cells.is_empty() || header_cells.len() != alignments.len() {
        return None;
    }
    let headers = header_cells.into_iter().map(|c| parse_inline(&c)).collect();
    let mut j = i + 2;
    let mut rows = Vec::new();
    while j < lines.len() && !lines[j].trim().is_empty() && lines[j].contains('|') {
        let mut cells: Vec<Vec<Inline>> = split_table_row(lines[j]).into_iter().map(|c| parse_inline(&c)).collect();
        cells.resize_with(alignments.len(), Vec::new);
        cells.truncate(alignments.len());
        rows.push(cells);
        j += 1;
    }
    Some((Block::Table { headers, alignments, rows }, j))
}

fn try_parse_list(lines: &[&str], mut i: usize, depth: usize) -> Option<(Block, usize)> {
    let first = parse_list_marker(lines[i])?;
    let (ordered, start, base_indent) = (first.ordered, first.start, first.indent);
    let mut items = Vec::new();
    while i < lines.len() {
        let Some(marker) = parse_list_marker(lines[i]) else { break };
        if marker.ordered != ordered || marker.indent > base_indent + 1 {
            break;
        }
        let content_indent = marker.content_indent;
        let (checked, first_rest) = strip_task_checkbox(marker.rest);
        let mut item_lines = vec![first_rest];
        i += 1;
        while i < lines.len() {
            let next = lines[i];
            if next.trim().is_empty() {
                if i + 1 < lines.len() && leading_spaces(lines[i + 1]) >= content_indent {
                    item_lines.push("");
                    i += 1;
                    continue;
                }
                break;
            }
            if parse_list_marker(next).is_some_and(|m| m.indent <= base_indent + 1) {
                break;
            }
            if leading_spaces(next) >= content_indent {
                item_lines.push(strip_leading_spaces(next, content_indent));
            } else if is_block_starter(next) {
                break;
            } else {
                item_lines.push(next.trim_start());
            }
            i += 1;
        }
        items.push(ListItem { checked, blocks: parse_blocks(&item_lines, depth + 1) });
    }
    Some((Block::List { ordered, start, items }, i))
}

fn is_block_starter(line: &str) -> bool {
    parse_fence_open(line).is_some()
        || parse_heading(line).is_some()
        || is_horizontal_rule(line)
        || is_blockquote_line(line)
        || parse_list_marker(line).is_some()
}

fn join_paragraph_lines(lines: &[&str]) -> String {
    let mut out = String::new();
    for (idx, line) in lines.iter().enumerate() {
        if idx > 0 {
            let prev = lines[idx - 1];
            if prev.ends_with("  ") || prev.ends_with('\\') {
                if prev.ends_with('\\') {
                    out.pop();
                }
                out.push('\n');
            } else {
                out.push(' ');
            }
        }
        out.push_str(if idx == 0 { line.trim_start() } else { line.trim() });
    }
    out
}

fn parse_fence_open(line: &str) -> Option<(char, usize, Option<String>)> {
    if leading_spaces(line) > 3 {
        return None;
    }
    let trimmed = line.trim_start();
    let ch = trimmed.chars().next().filter(|&c| c == '`' || c == '~')?;
    let count = trimmed.chars().take_while(|&c| c == ch).count();
    let rest = (count >= 3).then(|| &trimmed[count..])?;
    if ch == '`' && rest.contains('`') {
        return None;
    }
    Some((ch, count, rest.split_whitespace().next().map(str::to_string)))
}

fn is_fence_close(line: &str, fc: char, min_len: usize) -> bool {
    let trimmed = line.trim();
    leading_spaces(line) <= 3 && trimmed.len() >= min_len && trimmed.chars().all(|c| c == fc)
}

fn parse_heading(line: &str) -> Option<(u8, &str)> {
    if leading_spaces(line) > 3 {
        return None;
    }
    let trimmed = line.trim_start();
    let hashes = trimmed.bytes().take_while(|&b| b == b'#').count();
    let rest = (1..=6).contains(&hashes).then(|| &trimmed[hashes..])?;
    if !rest.is_empty() && !rest.starts_with([' ', '\t']) {
        return None;
    }
    Some((hashes as u8, rest.trim().trim_end_matches('#').trim_end()))
}

fn is_horizontal_rule(line: &str) -> bool {
    if leading_spaces(line) > 3 {
        return false;
    }
    let chars: Vec<char> = line.chars().filter(|&c| c != ' ' && c != '\t').collect();
    chars.len() >= 3 && matches!(chars[0], '-' | '*' | '_') && chars.iter().all(|&c| c == chars[0])
}

fn is_blockquote_line(line: &str) -> bool {
    leading_spaces(line) <= 3 && line.trim_start().starts_with('>')
}

fn strip_blockquote_prefix(line: &str) -> &str {
    let rest = line.trim_start().strip_prefix('>').unwrap_or(line);
    rest.strip_prefix(' ').unwrap_or(rest)
}

struct ListMarker<'a> {
    ordered: bool,
    start: u64,
    indent: usize,
    content_indent: usize,
    rest: &'a str,
}

fn parse_list_marker(line: &str) -> Option<ListMarker<'_>> {
    let indent = leading_spaces(line);
    let trimmed = &line[indent..];
    let bytes = trimmed.as_bytes();
    if bytes.len() >= 2 && matches!(bytes[0], b'-' | b'*' | b'+') && matches!(bytes[1], b' ' | b'\t') {
        return Some(ListMarker {
            ordered: false,
            start: 1,
            indent,
            content_indent: indent + 2,
            rest: trimmed[2..].trim_start(),
        });
    }
    let digits = bytes.iter().take_while(|b| b.is_ascii_digit()).count();
    if (1..=9).contains(&digits)
        && bytes.len() > digits + 1
        && matches!(bytes[digits], b'.' | b')')
        && matches!(bytes[digits + 1], b' ' | b'\t')
    {
        let start = trimmed[..digits].parse::<u64>().ok()?;
        return Some(ListMarker {
            ordered: true,
            start,
            indent,
            content_indent: indent + digits + 2,
            rest: trimmed[digits + 2..].trim_start(),
        });
    }
    None
}

fn strip_task_checkbox(rest: &str) -> (Option<bool>, &str) {
    if let Some(after) = rest.strip_prefix("[ ] ") {
        (Some(false), after)
    } else if let Some(after) = rest.strip_prefix("[x] ").or_else(|| rest.strip_prefix("[X] ")) {
        (Some(true), after)
    } else {
        (None, rest)
    }
}

fn parse_table_delimiter(line: &str) -> Option<Vec<Alignment>> {
    if !line.contains('-') {
        return None;
    }
    let cells = split_table_row(line);
    if cells.is_empty() {
        return None;
    }
    cells
        .into_iter()
        .map(|cell| {
            let t = cell.trim();
            let (left, right) = (t.starts_with(':'), t.ends_with(':'));
            let inner = t.strip_prefix(':').unwrap_or(t);
            let inner = inner.strip_suffix(':').unwrap_or(inner);
            if inner.is_empty() || !inner.bytes().all(|b| b == b'-') {
                return None;
            }
            Some(match (left, right) {
                (true, true) => Alignment::Center,
                (true, false) => Alignment::Left,
                (false, true) => Alignment::Right,
                (false, false) => Alignment::None,
            })
        })
        .collect()
}

fn split_table_row(line: &str) -> Vec<String> {
    let trimmed = line.trim();
    let trimmed = trimmed.strip_prefix('|').unwrap_or(trimmed);
    let trimmed = trimmed.strip_suffix('|').unwrap_or(trimmed);
    let (mut cells, mut cur, mut in_code) = (Vec::new(), String::new(), false);
    let mut chars = trimmed.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' && !in_code && chars.peek() == Some(&'|') {
            cur.push('|');
            chars.next();
        } else if c == '`' {
            in_code = !in_code;
            cur.push(c);
        } else if c == '|' && !in_code {
            cells.push(cur.trim().to_string());
            cur.clear();
        } else {
            cur.push(c);
        }
    }
    cells.push(cur.trim().to_string());
    cells
}

fn parse_standalone_image(s: &str) -> Option<(String, String)> {
    let (alt, after_bracket) = split_balanced(s.strip_prefix("![")?, '[', ']')?;
    let (src, trailing) = split_balanced(after_bracket.strip_prefix('(')?, '(', ')')?;
    if !trailing.is_empty() || src.trim().is_empty() {
        return None;
    }
    Some((alt.to_string(), strip_link_title(src).to_string()))
}

fn leading_spaces(s: &str) -> usize {
    s.bytes().take_while(|&b| b == b' ').count()
}

fn strip_leading_spaces(s: &str, max_strip: usize) -> &str {
    &s[leading_spaces(s).min(max_strip)..]
}

// ---------------------------------------------------------------------------
// Inline parser
// ---------------------------------------------------------------------------

fn parse_inline_inner(input: &str, depth: usize) -> Vec<Inline> {
    if depth >= MAX_DEPTH || input.is_empty() {
        return if input.is_empty() { Vec::new() } else { vec![Inline::Text(input.to_string())] };
    }
    let (mut out, mut text_buf, mut i) = (Vec::new(), String::new(), 0usize);

    while i < input.len() {
        let rest = &input[i..];
        let b = rest.as_bytes()[0];

        if b == b'\\' && rest.len() >= 2 && rest.as_bytes()[1].is_ascii_punctuation() {
            text_buf.push(rest.as_bytes()[1] as char);
            i += 2;
            continue;
        }
        if b == b'`' {
            let ticks = rest.bytes().take_while(|&c| c == b'`').count();
            if let Some((code, consumed)) = parse_code_span(rest, ticks) {
                flush_text(&mut text_buf, &mut out);
                out.push(Inline::Code(code));
                i += consumed;
            } else {
                text_buf.push_str(&rest[..ticks]);
                i += ticks;
            }
            continue;
        }
        if let Some((node, consumed)) = parse_inline_special(input, i, rest, b, depth) {
            flush_text(&mut text_buf, &mut out);
            out.push(node);
            i += consumed;
            continue;
        }

        let ch = rest.chars().next().unwrap();
        text_buf.push(ch);
        i += ch.len_utf8();
    }

    flush_text(&mut text_buf, &mut out);
    out
}

fn parse_inline_special(input: &str, i: usize, rest: &str, b: u8, depth: usize) -> Option<(Inline, usize)> {
    if b == b'!'
        && rest.starts_with("![")
        && let Some((alt, url, consumed)) = parse_link_at(&rest[1..])
    {
        return Some((Inline::Link { label: if alt.is_empty() { url.clone() } else { alt }, url }, 1 + consumed));
    }
    if b == b'['
        && let Some((label, url, consumed)) = parse_link_at(rest)
    {
        return Some((Inline::Link { label, url }, consumed));
    }
    if b == b'<'
        && let Some((url, consumed)) = parse_angle_autolink(rest)
    {
        return Some((Inline::Link { label: url.clone(), url }, consumed));
    }
    if b == b'h'
        && (rest.starts_with("https://") || rest.starts_with("http://"))
        && is_word_boundary_before(input, i)
        && let Some((url, consumed)) = parse_bare_url(rest)
    {
        return Some((Inline::Link { label: url.clone(), url }, consumed));
    }
    if rest.starts_with("~~")
        && let Some((inner, consumed)) = parse_delimited(rest, "~~")
    {
        return Some((Inline::Strike(parse_inline_inner(inner, depth + 1)), consumed));
    }
    if (rest.starts_with("**") || (rest.starts_with("__") && is_word_boundary_before(input, i)))
        && let Some((inner, consumed)) = parse_delimited(rest, &rest[..2])
    {
        return Some((Inline::Bold(parse_inline_inner(inner, depth + 1)), consumed));
    }
    if (b == b'*' || (b == b'_' && is_word_boundary_before(input, i)))
        && !rest.starts_with("**")
        && !rest.starts_with("__")
        && let Some((inner, consumed)) = parse_single_emphasis(rest, b as char)
    {
        return Some((Inline::Italic(parse_inline_inner(inner, depth + 1)), consumed));
    }
    None
}

fn flush_text(buf: &mut String, out: &mut Vec<Inline>) {
    if !buf.is_empty() {
        out.push(Inline::Text(std::mem::take(buf)));
    }
}

fn is_word_boundary_before(input: &str, idx: usize) -> bool {
    idx == 0 || !input[..idx].chars().next_back().is_some_and(char::is_alphanumeric)
}

fn parse_code_span(rest: &str, ticks: usize) -> Option<(String, usize)> {
    let mut pos = ticks;
    while pos < rest.len() {
        let at = pos + rest[pos..].find('`')?;
        let run = rest[at..].bytes().take_while(|&c| c == b'`').count();
        if run == ticks {
            let mut inner = &rest[ticks..at];
            if inner.len() >= 2 && inner.starts_with(' ') && inner.ends_with(' ') && inner.bytes().any(|b| b != b' ') {
                inner = &inner[1..inner.len() - 1];
            }
            return Some((inner.to_string(), at + ticks));
        }
        pos = at + run;
    }
    None
}

fn parse_link_at(rest: &str) -> Option<(String, String, usize)> {
    let (label, after_bracket) = split_balanced(rest.strip_prefix('[')?, '[', ']')?;
    let (raw_url, trailing) = split_balanced(after_bracket.strip_prefix('(')?, '(', ')')?;
    let url = strip_link_title(raw_url).trim();
    (!url.is_empty()).then(|| (label.to_string(), url.to_string(), rest.len() - trailing.len()))
}

fn split_balanced(s: &str, open: char, close: char) -> Option<(&str, &str)> {
    let (mut depth, mut escape) = (0usize, false);
    for (idx, ch) in s.char_indices() {
        if escape {
            escape = false;
        } else if ch == '\\' {
            escape = true;
        } else if ch == open {
            depth += 1;
        } else if ch == close {
            if depth == 0 {
                return Some((&s[..idx], &s[idx + close.len_utf8()..]));
            }
            depth -= 1;
        } else if ch == '\n' {
            return None;
        }
    }
    None
}

fn strip_link_title(raw: &str) -> &str {
    let trimmed = raw.trim();
    trimmed
        .strip_prefix('<')
        .and_then(|s| s.strip_suffix('>'))
        .unwrap_or_else(|| trimmed.split_whitespace().next().unwrap_or(trimmed))
}

fn parse_angle_autolink(rest: &str) -> Option<(String, usize)> {
    let inner = rest.strip_prefix('<')?;
    let end = inner.find('>')?;
    let url = &inner[..end];
    ((url.starts_with("https://") || url.starts_with("http://")) && !url.chars().any(char::is_whitespace))
        .then(|| (url.to_string(), end + 2))
}

fn parse_bare_url(rest: &str) -> Option<(String, usize)> {
    let mut end = rest
        .char_indices()
        .find(|&(_, c)| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '\'' | '`'))
        .map_or(rest.len(), |(i, _)| i);
    while end > 0 {
        let candidate = &rest[..end];
        let last = candidate.chars().next_back()?;
        if matches!(last, '.' | ',' | ';' | ':' | '!' | '?') {
            end -= last.len_utf8();
        } else if last == ')' && candidate.matches(')').count() > candidate.matches('(').count() {
            end -= 1;
        } else {
            break;
        }
    }
    (end > "http://".len()).then(|| (rest[..end].to_string(), end))
}

fn parse_delimited<'a>(rest: &'a str, delim: &str) -> Option<(&'a str, usize)> {
    let body = rest.strip_prefix(delim)?;
    if body.is_empty() || body.starts_with(char::is_whitespace) {
        return None;
    }
    let mut pos = 0usize;
    while pos < body.len() {
        let close = pos + body[pos..].find(delim)?;
        let inner = &body[..close];
        let after = &body[close + delim.len()..];
        let boundary_ok = delim != "__" || after.chars().next().is_none_or(|c| !c.is_alphanumeric());
        if !inner.is_empty() && !inner.ends_with(char::is_whitespace) && boundary_ok {
            return Some((inner, close + 2 * delim.len()));
        }
        pos = close + delim.len();
    }
    None
}

fn parse_single_emphasis(rest: &str, ch: char) -> Option<(&str, usize)> {
    let body = &rest[1..];
    if body.is_empty() || body.starts_with(char::is_whitespace) || body.starts_with(ch) {
        return None;
    }
    let mut escape = false;
    for (idx, c) in body.char_indices() {
        if escape {
            escape = false;
        } else if c == '\\' {
            escape = true;
        } else if c == ch {
            let (inner, after) = (&body[..idx], &body[idx + 1..]);
            if !after.starts_with(ch)
                && !inner.is_empty()
                && !inner.ends_with(char::is_whitespace)
                && (ch != '_' || after.chars().next().is_none_or(|n| !n.is_alphanumeric()))
            {
                return Some((inner, idx + 2));
            }
        }
    }
    None
}

fn inject_file_refs(inlines: Vec<Inline>) -> Vec<Inline> {
    let mut out = Vec::with_capacity(inlines.len());
    for inline in inlines {
        match inline {
            Inline::Text(text) => {
                let refs = crate::links::find(&text);
                if refs.is_empty() {
                    out.push(Inline::Text(text));
                    continue;
                }
                let mut cursor = 0usize;
                for r in refs {
                    if r.range.start < cursor || r.range.end > text.len() {
                        continue;
                    }
                    if r.range.start > cursor {
                        out.push(Inline::Text(text[cursor..r.range.start].to_string()));
                    }
                    out.push(Inline::FileRef { raw: text[r.range.clone()].to_string(), path: r.path, line: r.line });
                    cursor = r.range.end;
                }
                if cursor < text.len() {
                    out.push(Inline::Text(text[cursor..].to_string()));
                }
            }
            Inline::Bold(inner) => out.push(Inline::Bold(inject_file_refs(inner))),
            Inline::Italic(inner) => out.push(Inline::Italic(inject_file_refs(inner))),
            Inline::Strike(inner) => out.push(Inline::Strike(inject_file_refs(inner))),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_headings_rules_code_and_quotes() {
        let md = "# Title\n\n> Quoted **bold**\n\n---\n\n```rust\nfn main() {}\n```";
        assert_eq!(
            parse(md),
            vec![
                Block::Heading { level: 1, inlines: vec![Inline::Text("Title".into())] },
                Block::BlockQuote(vec![Block::Paragraph(vec![
                    Inline::Text("Quoted ".into()),
                    Inline::Bold(vec![Inline::Text("bold".into())]),
                ])]),
                Block::Rule,
                Block::CodeBlock { lang: Some("rust".into()), code: "fn main() {}".into() },
            ]
        );
    }

    #[test]
    fn parses_lists_with_task_checkboxes_and_tables() {
        let blocks = parse("- [x] Done\n- [ ] Todo\n\n| Name | Value |\n| :--- | ---: |\n| a | 1 |");
        assert_eq!(blocks.len(), 2);
        match &blocks[0] {
            Block::List { ordered: false, items, .. } => {
                assert_eq!((items.len(), items[0].checked, items[1].checked), (2, Some(true), Some(false)));
            }
            other => panic!("expected list, got {other:?}"),
        }
        match &blocks[1] {
            Block::Table { headers, alignments, rows } => {
                assert_eq!(
                    (headers.len(), alignments.as_slice(), rows.len()),
                    (2, &[Alignment::Left, Alignment::Right][..], 1)
                );
            }
            other => panic!("expected table, got {other:?}"),
        }
    }

    #[test]
    fn parses_inline_formatting_links_images_and_file_refs() {
        let inlines = parse_inline("See src/app/mod.rs:42, *em*, ~~old~~, `code`, and [docs](https://example.com).");
        assert!(inlines.iter().any(|i| matches!(
            i,
            Inline::FileRef { raw, path, line: Some(42) } if raw == "src/app/mod.rs:42" && path == "src/app/mod.rs"
        )));
        assert!(inlines.iter().any(|i| matches!(i, Inline::Italic(_))));
        assert!(inlines.iter().any(|i| matches!(i, Inline::Strike(_))));
        assert!(inlines.iter().any(|i| matches!(i, Inline::Code(c) if c == "code")));
        assert!(
            inlines
                .iter()
                .any(|i| matches!(i, Inline::Link { label, url } if label == "docs" && url == "https://example.com"))
        );
        assert_eq!(
            parse("![diagram](assets/arch.png)"),
            vec![Block::Image { alt: "diagram".into(), src: "assets/arch.png".into() }]
        );
    }

    #[test]
    fn does_not_italicize_snake_case_identifiers() {
        assert_eq!(parse_inline("foo_bar_baz"), vec![Inline::Text("foo_bar_baz".into())]);
    }

    #[test]
    fn extracts_plain_text_for_find() {
        let text =
            extract_plain_text(&parse("## Summary\n\nEdited src/main.rs:10 with **care**.\n\n```sh\necho hi\n```"));
        assert!(
            text.contains("Summary") && text.contains("Edited src/main.rs:10 with care.") && text.contains("echo hi")
        );
    }

    #[test]
    fn survives_pathological_nesting_and_unicode() {
        let _ = parse(&("> ".repeat(200) + "hi"));
        let _ = parse_inline(&("*".repeat(300) + "x" + &"*".repeat(300)));
        let _ = parse("日本語 `コード` **太字** ![絵](a.png)\n| 項目 |\n|---|\n| 値 |");
    }
}
