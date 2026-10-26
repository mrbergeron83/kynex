//! Markdown to gpui elements, via pulldown-cmark. Cheap enough to redo on every
//! render of a visible message, which keeps streaming replies simple: the text
//! grows, the next frame parses it again.

use std::ops::Range;
use std::rc::Rc;

use gpui::{
    AnyElement, App, ClipboardItem, ElementId, FontStyle, FontWeight, HighlightStyle, SharedString, StrikethroughStyle,
    UnderlineStyle, Window, div, prelude::*, px,
};
use kynex_core::terminal::ShellKind;
use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use crate::theme::{Palette, mono, theme};
use crate::ui::selectable::{Spot, selectable};
use crate::ui::{Icon, Tooltip, icon, icon_button};

/// Runs a suggested shell command (a ```bash or ```powershell block) somewhere.
pub type RunCommand = Rc<dyn Fn(ShellKind, String, &mut Window, &mut App)>;

/// Renders `src` as a column of blocks. `id` keeps element ids (copy buttons)
/// unique across messages; the text is selectable as item `item` of selection
/// group `group`. With `run`, shell code blocks get a play button that calls it.
pub fn render(
    id: impl Into<ElementId>,
    src: &str,
    group: u64,
    item: usize,
    run: Option<RunCommand>,
    cx: &App,
) -> AnyElement {
    let mut b = Builder::new(id.into(), theme(cx).clone(), cx);
    b.select = (group, item);
    b.run = run;
    let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    for event in Parser::new_ext(src, options) {
        b.event(event);
    }
    b.finish()
}

enum Container {
    Root,
    Quote,
    List { next: Option<u64> },
    Item,
    Table { alignments: Vec<Alignment>, rows: Vec<(bool, Vec<AnyElement>)> },
    Row { head: bool, cells: Vec<AnyElement> },
    Cell,
}

struct Frame {
    kind: Container,
    children: Vec<AnyElement>,
}

#[derive(Default)]
struct Inline {
    text: String,
    highlights: Vec<(Range<usize>, HighlightStyle)>,
    fonts: Vec<(Range<usize>, SharedString)>,
    links: Vec<(Range<usize>, String)>,
}

struct Builder<'a> {
    id: ElementId,
    p: Palette,
    cx: &'a App,
    stack: Vec<Frame>,
    inline: Inline,
    bold: usize,
    italic: usize,
    strike: usize,
    link: Option<String>,
    heading: Option<HeadingLevel>,
    code: Option<(String, String)>,
    counter: usize,
    /// Selection group and item, and the next block's number in it.
    select: (u64, usize),
    blocks: usize,
    run: Option<RunCommand>,
}

impl<'a> Builder<'a> {
    fn new(id: ElementId, p: Palette, cx: &'a App) -> Self {
        Self {
            id,
            p,
            cx,
            stack: vec![Frame { kind: Container::Root, children: Vec::new() }],
            inline: Inline::default(),
            bold: 0,
            italic: 0,
            strike: 0,
            link: None,
            heading: None,
            code: None,
            counter: 0,
            select: (0, 0),
            blocks: 0,
            run: None,
        }
    }

    fn next_id(&mut self, what: &'static str) -> ElementId {
        self.counter += 1;
        ElementId::NamedChild(std::sync::Arc::new(self.id.clone()), format!("{what}-{}", self.counter).into())
    }

    /// Where the next block of text sits in the selection order.
    fn next_spot(&mut self) -> Spot {
        self.blocks += 1;
        Spot::new(self.select.1, self.blocks)
    }

    fn push_block(&mut self, el: AnyElement) {
        self.stack.last_mut().unwrap().children.push(el);
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
                let rule = div().my_2().h(px(1.)).w_full().bg(self.p.border_variant).into_any_element();
                self.push_block(rule);
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
            Tag::List(start) => {
                self.flush();
                self.open(Container::List { next: start });
            }
            Tag::Item => self.open(Container::Item),
            Tag::Table(alignments) => {
                self.flush();
                self.open(Container::Table { alignments, rows: Vec::new() });
            }
            Tag::TableHead => self.open(Container::Row { head: true, cells: Vec::new() }),
            Tag::TableRow => self.open(Container::Row { head: false, cells: Vec::new() }),
            Tag::TableCell => self.open(Container::Cell),
            Tag::Emphasis => self.italic += 1,
            Tag::Strong => self.bold += 1,
            Tag::Strikethrough => self.strike += 1,
            Tag::Link { dest_url, .. } => self.link = Some(dest_url.to_string()),
            Tag::Image { dest_url, .. } => self.link = Some(dest_url.to_string()),
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
                let el = self.code_block(lang, code);
                self.push_block(el);
            }
            TagEnd::BlockQuote(_) => {
                let children = self.close();
                let quote = div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .pl_3()
                    .border_l_2()
                    .border_color(self.p.border)
                    .text_color(self.p.text_muted)
                    .children(children)
                    .into_any_element();
                self.push_block(quote);
            }
            TagEnd::List(_) => {
                let children = self.close();
                let list = div().flex().flex_col().gap_1().children(children).into_any_element();
                self.push_block(list);
            }
            TagEnd::Item => {
                self.flush();
                let children = self.close();
                let marker = match &mut self.stack.last_mut().unwrap().kind {
                    Container::List { next: Some(n) } => {
                        *n += 1;
                        format!("{}.", *n - 1)
                    }
                    _ => "•".to_string(),
                };
                let item = div()
                    .flex()
                    .gap_2()
                    .child(div().flex_none().min_w(px(14.)).text_color(self.p.text_muted).child(marker))
                    .child(div().flex().flex_col().gap_1().flex_1().min_w_0().children(children))
                    .into_any_element();
                self.push_block(item);
            }
            TagEnd::TableCell => {
                self.flush();
                let children = self.close();
                if let Some(Frame { kind: Container::Row { cells, .. }, .. }) = self.stack.last_mut() {
                    cells.push(div().flex().flex_col().children(children).into_any_element());
                }
            }
            TagEnd::TableHead | TagEnd::TableRow => {
                let frame = self.stack.pop().unwrap();
                if let (Container::Row { head, cells }, Some(Frame { kind: Container::Table { rows, .. }, .. })) =
                    (frame.kind, self.stack.last_mut())
                {
                    rows.push((head, cells));
                }
            }
            TagEnd::Table => {
                let frame = self.stack.pop().unwrap();
                if let Container::Table { alignments, rows } = frame.kind {
                    let el = self.table(alignments, rows);
                    self.push_block(el);
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
        self.stack.push(Frame { kind, children: Vec::new() });
    }

    fn close(&mut self) -> Vec<AnyElement> {
        self.flush();
        if self.stack.len() > 1 { self.stack.pop().unwrap().children } else { Vec::new() }
    }

    fn text(&mut self, t: &str, code: bool) {
        if t.is_empty() {
            return;
        }
        let start = self.inline.text.len();
        self.inline.text.push_str(t);
        let range = start..self.inline.text.len();
        let mut style = HighlightStyle::default();
        let mut styled = false;
        if self.bold > 0 || self.heading.is_some() {
            style.font_weight = Some(FontWeight::SEMIBOLD);
            styled = true;
        }
        if self.italic > 0 {
            style.font_style = Some(FontStyle::Italic);
            styled = true;
        }
        if self.strike > 0 {
            style.strikethrough = Some(StrikethroughStyle { thickness: px(1.), color: None });
            styled = true;
        }
        if code {
            style.background_color = Some(self.p.element);
            self.inline.fonts.push((range.clone(), mono()));
            styled = true;
        }
        if let Some(url) = &self.link {
            style.color = Some(self.p.accent);
            style.underline = Some(UnderlineStyle { thickness: px(1.), color: Some(self.p.accent), wavy: false });
            self.inline.links.push((range.clone(), url.clone()));
            styled = true;
        }
        if styled {
            self.inline.highlights.push((range, style));
        }
    }

    /// Turns the gathered inline text into one wrapped paragraph block.
    fn flush(&mut self) {
        if self.inline.text.is_empty() {
            return;
        }
        let inline = std::mem::take(&mut self.inline);
        let spot = self.next_spot();
        let (highlights, fonts) = (inline.highlights, inline.fonts);
        let body = selectable(self.select.0, spot, inline.text, self.p.selection)
            .styled(|t| t.with_highlights(highlights).with_font_family_overrides(fonts))
            .links(inline.links)
            .into_any_element();
        let block = match self.heading {
            Some(level) => {
                let size = match level {
                    HeadingLevel::H1 => px(20.),
                    HeadingLevel::H2 => px(17.),
                    HeadingLevel::H3 => px(15.),
                    _ => px(14.),
                };
                div().pt_1().text_size(size).text_color(self.p.text).child(body).into_any_element()
            }
            None => div().child(body).into_any_element(),
        };
        self.push_block(block);
    }

    fn code_block(&mut self, lang: String, code: String) -> AnyElement {
        let code = code.trim_end_matches('\n').to_string();
        let copy_id = self.next_id("copy");
        let scroll_id = self.next_id("code");
        let to_copy = code.clone();
        // Suggested shell commands get a play button; it stays visible so it's found.
        let play = ShellKind::for_lang(&lang).zip(self.run.clone()).map(|(shell, run)| {
            let command = code.clone();
            let success = self.p.success;
            div()
                .id(self.next_id("run"))
                .absolute()
                .top_1()
                .right_1()
                .flex()
                .items_center()
                .justify_center()
                .size(px(24.))
                .rounded_md()
                .cursor_pointer()
                .hover(move |s| s.bg(success.opacity(0.15)))
                .child(icon(Icon::Play, success.opacity(0.8)).size_3())
                .tooltip(Tooltip::with_meta(format!("Run in {}", shell.label()), "In this project's terminal, below"))
                .on_click(move |_, window, cx| run(shell, command.clone(), window, cx))
        });
        let has_play = play.is_some();
        let text = selectable(self.select.0, self.next_spot(), code, self.p.selection);
        let p = &self.p;
        div()
            .group("code")
            .relative()
            .flex()
            .flex_col()
            .rounded_md()
            .border_1()
            .border_color(p.border_variant)
            .bg(p.element)
            .child(
                div()
                    .id(scroll_id)
                    .overflow_x_scroll()
                    .px_3()
                    .py_2()
                    .font_family(mono())
                    .text_size(px(13.))
                    .child(div().whitespace_nowrap().child(text)),
            )
            .child(
                div()
                    .absolute()
                    .top_1()
                    .map(|d| if has_play { d.right(px(30.)) } else { d.right_1() })
                    .flex()
                    .items_center()
                    .gap_1()
                    .invisible()
                    .group_hover("code", |s| s.visible())
                    .when(!lang.is_empty(), |d| d.child(div().text_xs().text_color(p.text_placeholder).child(lang)))
                    .child(
                        icon_button(copy_id, Icon::Copy, self.cx).tooltip(Tooltip::text("Copy")).on_click(
                            move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(to_copy.clone())),
                        ),
                    ),
            )
            .children(play)
            .into_any_element()
    }

    fn table(&mut self, alignments: Vec<Alignment>, rows: Vec<(bool, Vec<AnyElement>)>) -> AnyElement {
        let id = self.next_id("table");
        let p = &self.p;
        let columns = alignments.len().max(1);
        div()
            .id(id)
            .overflow_x_scroll()
            .child(div().flex().flex_col().rounded_md().border_1().border_color(p.border_variant).children(
                rows.into_iter().enumerate().map(|(i, (head, cells))| {
                    div()
                        .flex()
                        .when(i > 0, |d| d.border_t_1().border_color(p.border_variant))
                        .when(head, |d| d.bg(p.element).font_weight(FontWeight::SEMIBOLD))
                        .children(
                            cells
                                .into_iter()
                                .take(columns)
                                .map(|c| div().flex_1().min_w(px(80.)).px_2().py_1().child(c)),
                        )
                }),
            ))
            .into_any_element()
    }

    fn finish(mut self) -> AnyElement {
        self.flush();
        while self.stack.len() > 1 {
            let frame = self.stack.pop().unwrap();
            self.stack.last_mut().unwrap().children.extend(frame.children);
        }
        let root = self.stack.pop().unwrap();
        div().flex().flex_col().gap_2().children(root.children).into_any_element()
    }
}
