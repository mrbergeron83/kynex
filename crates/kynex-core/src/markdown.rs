//! Markdown parsed into blocks, via pulldown-cmark. The app parses a message
//! once per change and draws the blocks on every frame, so a streaming reply or
//! an animation doesn't parse it again each time.

use std::ops::Range;

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

/// A parsed message: its blocks, top to bottom.
pub type Doc = Vec<Block>;

#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Paragraph(Inline),
    /// Level 1 to 6.
    Heading(u8, Inline),
    Code {
        /// The fence's language, or empty.
        lang: String,
        code: String,
    },
    Quote(Doc),
    List(Vec<ListItem>),
    Table {
        columns: usize,
        rows: Vec<TableRow>,
    },
    Rule,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ListItem {
    /// `•`, or `3.` in a numbered list.
    pub marker: String,
    pub blocks: Doc,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TableRow {
    pub head: bool,
    pub cells: Vec<Doc>,
}

/// A run of text and the styled parts of it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Inline {
    pub text: String,
    /// Byte ranges of `text` with a style or a link, in order.
    pub spans: Vec<Span>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Span {
    pub range: Range<usize>,
    pub style: Style,
    pub link: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub code: bool,
}

/// Parses `src` (CommonMark with tables, strikethrough and task lists).
pub fn parse(src: &str) -> Doc {
    let mut b = Builder::default();
    let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    for event in Parser::new_ext(src, options) {
        b.event(event);
    }
    b.finish()
}

enum Container {
    Root,
    Quote,
    List { next: Option<u64>, items: Vec<ListItem> },
    Item,
    Table { columns: usize, rows: Vec<TableRow> },
    Row { head: bool, cells: Vec<Doc> },
    Cell,
}

struct Frame {
    kind: Container,
    blocks: Doc,
}

struct Builder {
    stack: Vec<Frame>,
    inline: Inline,
    bold: usize,
    italic: usize,
    strike: usize,
    link: Option<String>,
    heading: Option<HeadingLevel>,
    code: Option<(String, String)>,
}

impl Default for Builder {
    fn default() -> Self {
        Self {
            stack: vec![Frame { kind: Container::Root, blocks: Vec::new() }],
            inline: Inline::default(),
            bold: 0,
            italic: 0,
            strike: 0,
            link: None,
            heading: None,
            code: None,
        }
    }
}

impl Builder {
    fn push(&mut self, block: Block) {
        if let Some(frame) = self.stack.last_mut() {
            frame.blocks.push(block);
        }
    }

    fn event(&mut self, event: Event) {
        if let Some((_, code)) = &mut self.code {
            match event {
                Event::Text(t) => return code.push_str(&t),
                Event::End(TagEnd::CodeBlock) => {}
                _ => return,
            }
        }
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(t) => self.text(&t, false),
            Event::Code(t) => self.text(&t, true),
            Event::InlineHtml(t) | Event::Html(t) => self.text(&t, false),
            Event::SoftBreak => self.text(" ", false),
            Event::HardBreak => self.text("\n", false),
            Event::Rule => {
                self.flush();
                self.push(Block::Rule);
            }
            Event::TaskListMarker(done) => self.text(if done { "☑ " } else { "☐ " }, false),
            Event::FootnoteReference(t) => self.text(&format!("[{t}]"), false),
            _ => {}
        }
    }

    fn start(&mut self, tag: Tag) {
        match tag {
            Tag::Heading { level, .. } => {
                self.flush();
                self.heading = Some(level);
            }
            Tag::CodeBlock(kind) => {
                self.flush();
                let lang = match kind {
                    CodeBlockKind::Fenced(l) => l.split([',', ' ']).next().unwrap_or("").to_string(),
                    CodeBlockKind::Indented => String::new(),
                };
                self.code = Some((lang, String::new()));
            }
            Tag::BlockQuote(_) => self.open(Container::Quote),
            Tag::List(start) => self.open(Container::List { next: start, items: Vec::new() }),
            Tag::Item => self.open(Container::Item),
            Tag::Table(alignments) => {
                self.open(Container::Table { columns: alignments.len().max(1), rows: Vec::new() })
            }
            Tag::TableHead => self.open(Container::Row { head: true, cells: Vec::new() }),
            Tag::TableRow => self.open(Container::Row { head: false, cells: Vec::new() }),
            Tag::TableCell => self.open(Container::Cell),
            Tag::Emphasis => self.italic += 1,
            Tag::Strong => self.bold += 1,
            Tag::Strikethrough => self.strike += 1,
            Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. } => self.link = Some(dest_url.to_string()),
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => self.flush(),
            TagEnd::Heading(_) => {
                self.flush();
                self.heading = None;
            }
            TagEnd::CodeBlock => {
                let (lang, code) = self.code.take().unwrap_or_default();
                let code = code.trim_end_matches('\n').to_string();
                self.push(Block::Code { lang, code });
            }
            TagEnd::BlockQuote(_) => {
                if let Some(Frame { blocks, .. }) = self.close() {
                    self.push(Block::Quote(blocks));
                }
            }
            TagEnd::List(_) => {
                if let Some(Frame { kind: Container::List { items, .. }, .. }) = self.close() {
                    self.push(Block::List(items));
                }
            }
            TagEnd::Item => {
                let Some(Frame { blocks, .. }) = self.close() else { return };
                if let Some(Frame { kind: Container::List { next, items }, .. }) = self.stack.last_mut() {
                    let marker = match next {
                        Some(n) => {
                            *n += 1;
                            format!("{}.", *n - 1)
                        }
                        None => "•".to_string(),
                    };
                    items.push(ListItem { marker, blocks });
                }
            }
            TagEnd::TableCell => {
                let Some(Frame { blocks, .. }) = self.close() else { return };
                if let Some(Frame { kind: Container::Row { cells, .. }, .. }) = self.stack.last_mut() {
                    cells.push(blocks);
                }
            }
            TagEnd::TableHead | TagEnd::TableRow => {
                if let Some(Frame { kind: Container::Row { head, cells }, .. }) = self.close()
                    && let Some(Frame { kind: Container::Table { rows, .. }, .. }) = self.stack.last_mut()
                {
                    rows.push(TableRow { head, cells });
                }
            }
            TagEnd::Table => {
                if let Some(Frame { kind: Container::Table { columns, rows }, .. }) = self.close() {
                    self.push(Block::Table { columns, rows });
                }
            }
            TagEnd::Emphasis => self.italic = self.italic.saturating_sub(1),
            TagEnd::Strong => self.bold = self.bold.saturating_sub(1),
            TagEnd::Strikethrough => self.strike = self.strike.saturating_sub(1),
            TagEnd::Link | TagEnd::Image => self.link = None,
            _ => {}
        }
    }

    fn open(&mut self, kind: Container) {
        self.flush();
        self.stack.push(Frame { kind, blocks: Vec::new() });
    }

    /// Ends the innermost container; never the root.
    fn close(&mut self) -> Option<Frame> {
        self.flush();
        if self.stack.len() > 1 { self.stack.pop() } else { None }
    }

    fn text(&mut self, t: &str, code: bool) {
        if t.is_empty() {
            return;
        }
        let start = self.inline.text.len();
        self.inline.text.push_str(t);
        let style = Style {
            bold: self.bold > 0 || self.heading.is_some(),
            italic: self.italic > 0,
            strike: self.strike > 0,
            code,
        };
        if style != Style::default() || self.link.is_some() {
            let range = start..self.inline.text.len();
            self.inline.spans.push(Span { range, style, link: self.link.clone() });
        }
    }

    /// Turns the gathered inline text into one paragraph or heading.
    fn flush(&mut self) {
        if self.inline.text.is_empty() {
            return;
        }
        let inline = std::mem::take(&mut self.inline);
        let block = match self.heading {
            Some(level) => Block::Heading(level as u8, inline),
            None => Block::Paragraph(inline),
        };
        self.push(block);
    }

    fn finish(mut self) -> Doc {
        self.flush();
        // pulldown-cmark closes what it opens; anything left still shows.
        while self.stack.len() > 1 {
            let frame = self.stack.pop().unwrap();
            self.stack.last_mut().unwrap().blocks.extend(frame.blocks);
        }
        self.stack.pop().map(|f| f.blocks).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn para(text: &str) -> Block {
        Block::Paragraph(Inline { text: text.into(), spans: Vec::new() })
    }

    #[test]
    fn paragraphs_headings_and_styles() {
        let doc = parse("# Title\n\nSome **bold** and `code` [here](https://x.y).\n\n---");
        assert_eq!(doc.len(), 3);
        let Block::Heading(1, h) = &doc[0] else { panic!("{doc:?}") };
        assert_eq!(h.text, "Title");
        assert!(h.spans[0].style.bold);
        let Block::Paragraph(p) = &doc[1] else { panic!("{doc:?}") };
        assert_eq!(p.text, "Some bold and code here.");
        let parts: Vec<_> = p.spans.iter().map(|s| (&p.text[s.range.clone()], s.style, s.link.as_deref())).collect();
        assert_eq!(
            parts,
            [
                ("bold", Style { bold: true, ..Style::default() }, None),
                ("code", Style { code: true, ..Style::default() }, None),
                ("here", Style::default(), Some("https://x.y")),
            ]
        );
        assert_eq!(doc[2], Block::Rule);
    }

    #[test]
    fn lists_number_on_from_their_start() {
        let doc = parse("3. a\n4. b\n   - c\n");
        let Block::List(items) = &doc[0] else { panic!("{doc:?}") };
        assert_eq!(items.iter().map(|i| i.marker.as_str()).collect::<Vec<_>>(), ["3.", "4."]);
        assert_eq!(items[0].blocks, [para("a")]);
        let Block::List(inner) = &items[1].blocks[1] else { panic!("{items:?}") };
        assert_eq!(inner[0].marker, "•");
        assert_eq!(inner[0].blocks, [para("c")]);
    }

    #[test]
    fn code_blocks_quotes_and_tables() {
        let doc = parse("```bash,ignore\nls -la\n\n```\n\n> quoted\n\n| a | b |\n|---|---|\n| 1 | 2 |\n");
        assert_eq!(doc[0], Block::Code { lang: "bash".into(), code: "ls -la".into() });
        assert_eq!(doc[1], Block::Quote(vec![para("quoted")]));
        let Block::Table { columns: 2, rows } = &doc[2] else { panic!("{doc:?}") };
        assert!(rows[0].head && !rows[1].head);
        assert_eq!(rows[1].cells, [vec![para("1")], vec![para("2")]]);
    }

    #[test]
    fn half_written_markdown_still_parses() {
        // A reply mid-stream: an open fence, an unfinished table and list.
        for src in ["```rust\nfn main() {", "| a |\n|--", "1. one\n2.", "**bold", "> > ", "- [x] done\n- [ ] not"] {
            let doc = parse(src);
            assert!(!doc.is_empty(), "{src:?}");
        }
    }
}
