//! Markdown to gpui elements. The text is parsed into blocks by
//! `kynex_core::markdown` once per change and cached; every render (a streaming
//! reply, each frame of a scroll or an animation) only draws the blocks.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{BuildHasher, RandomState};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::ui::px;
use gpui::{
    AnyElement, App, Axis, Bounds, ClipboardItem, DispatchPhase, ElementId, FontStyle, FontWeight, HighlightStyle,
    MouseMoveEvent, Pixels, ScrollHandle, SharedString, StrikethroughStyle, UnderlineStyle, Window, canvas, div,
    prelude::*,
};
use kynex_core::highlight::Lang;
use kynex_core::markdown::{self, Block, Doc, Inline, ListItem, TableRow};
use kynex_core::terminal::ShellKind;

use crate::file_links;
use crate::theme::{Palette, mono, theme};
use crate::ui::selectable::{self, Link, Spot, selectable};
use crate::ui::{Icon, Tooltip, code, icon, icon_button, scrollbar};

/// Runs a suggested shell command (a ```bash or ```powershell block) somewhere.
pub type RunCommand = Rc<dyn Fn(ShellKind, String, &mut Window, &mut App)>;

/// Renders `src` as a column of blocks. `id` keeps element ids (copy buttons)
/// unique across messages; the text is selectable as item `item` of selection
/// group `group`. With `run`, shell code blocks get a play button that calls it;
/// with `cwd`, paths to files under it link to them.
pub fn render(
    id: impl Into<ElementId>,
    src: &str,
    group: u64,
    item: usize,
    run: Option<RunCommand>,
    cwd: Option<&Path>,
    cx: &App,
) -> AnyElement {
    let doc = parsed(group, item, src);
    let mut b = Builder {
        id: id.into(),
        p: theme(cx).clone(),
        cx,
        counter: 0,
        select: (group, item),
        blocks: 0,
        run,
        cwd: cwd.map(Path::to_path_buf),
    };
    let blocks = b.blocks(&doc);
    div().flex().flex_col().gap_2().children(blocks).into_any_element()
}

/// Parsed messages by selection group and item, with a hash of their text.
type Cache = HashMap<(u64, usize), (u64, Rc<Doc>)>;

thread_local! {
    static PARSED: RefCell<(RandomState, Cache)> = RefCell::default();
}

/// `src` parsed, from the cache while it's unchanged.
fn parsed(group: u64, item: usize, src: &str) -> Rc<Doc> {
    PARSED.with_borrow_mut(|(hasher, cache)| {
        let hash = hasher.hash_one(src);
        if let Some((h, doc)) = cache.get(&(group, item))
            && *h == hash
        {
            return doc.clone();
        }
        // Old sessions' messages: dropped wholesale now and then, parsed again if shown.
        if cache.len() >= 4096 {
            cache.clear();
        }
        let doc = Rc::new(markdown::parse(src));
        cache.insert((group, item), (hash, doc.clone()));
        doc
    })
}

thread_local! {
    /// Code blocks' sideways scroll, by selection group, item and element number.
    static CODE_SCROLL: RefCell<HashMap<(u64, usize, usize), ScrollHandle>> = RefCell::default();
}

/// The scroll handle of a code block, kept across frames.
fn code_scroll((group, item): (u64, usize), n: usize) -> ScrollHandle {
    CODE_SCROLL.with_borrow_mut(|all| {
        if all.len() >= 4096 && !all.contains_key(&(group, item, n)) {
            all.clear();
        }
        all.entry((group, item, n)).or_default().clone()
    })
}

/// Pixels from either side of a code block where a drag-selection scrolls it.
const EDGE: f32 = 32.;

/// While a selection is dragged out of a code block's sides, scrolls the code
/// after it, so a line longer than the block can be selected whole.
fn follow_drag(scroll: ScrollHandle, bounds: Bounds<Pixels>, group: u64, spot: Spot, window: &mut Window) {
    window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, cx| {
        if phase != DispatchPhase::Bubble || !e.dragging() || !selectable::dragging_from(group, spot, cx) {
            return;
        }
        let x = f32::from(e.position.x);
        let (left, right) = (f32::from(bounds.left()) + EDGE, f32::from(bounds.right()) - EDGE);
        let step = if x > right {
            x - right
        } else if x < left {
            x - left
        } else {
            return;
        };
        let mut o = scroll.offset();
        let max = f32::from(scroll.max_offset().x);
        let to = (-f32::from(o.x) + step).clamp(0., max.max(0.));
        if (to + f32::from(o.x)).abs() > 0.5 {
            o.x = gpui::px(-to);
            scroll.set_offset(o);
            window.refresh();
        }
    });
}

struct Builder<'a> {
    id: ElementId,
    p: Palette,
    cx: &'a App,
    counter: usize,
    /// Selection group and item, and the last block's number in it.
    select: (u64, usize),
    blocks: usize,
    run: Option<RunCommand>,
    /// The session's folder, for linking file paths; none links nothing.
    cwd: Option<PathBuf>,
}

impl Builder<'_> {
    fn next_id(&mut self, what: &'static str) -> ElementId {
        self.counter += 1;
        ElementId::NamedChild(std::sync::Arc::new(self.id.clone()), format!("{what}-{}", self.counter).into())
    }

    /// Where the next block of text sits in the selection order.
    fn next_spot(&mut self) -> Spot {
        self.blocks += 1;
        Spot::new(self.select.1, self.blocks)
    }

    fn blocks(&mut self, doc: &[Block]) -> Vec<AnyElement> {
        doc.iter().map(|b| self.block(b)).collect()
    }

    fn block(&mut self, block: &Block) -> AnyElement {
        match block {
            Block::Paragraph(inline) => div().child(self.inline(inline)).into_any_element(),
            Block::Heading(level, inline) => {
                let size = match level {
                    1 => px(20.),
                    2 => px(17.),
                    3 => px(15.),
                    _ => px(14.),
                };
                let body = self.inline(inline);
                div().pt_1().text_size(size).text_color(self.p.text).child(body).into_any_element()
            }
            Block::Code { lang, code } => self.code_block(lang, code),
            Block::Quote(doc) => {
                let children = self.blocks(doc);
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .pl_3()
                    .border_l_2()
                    .border_color(self.p.border)
                    .text_color(self.p.text_muted)
                    .children(children)
                    .into_any_element()
            }
            Block::List(items) => {
                let items: Vec<_> = items.iter().map(|i| self.list_item(i)).collect();
                div().flex().flex_col().gap_1().children(items).into_any_element()
            }
            Block::Table { columns, rows } => self.table(*columns, rows),
            Block::Rule => div().my_2().h(px(1.)).w_full().bg(self.p.border_variant).into_any_element(),
        }
    }

    fn list_item(&mut self, item: &ListItem) -> AnyElement {
        let children = self.blocks(&item.blocks);
        div()
            .flex()
            .gap_2()
            .child(div().flex_none().min_w(px(14.)).text_color(self.p.text_muted).child(item.marker.clone()))
            .child(div().flex().flex_col().gap_1().flex_1().min_w_0().children(children))
            .into_any_element()
    }

    /// One wrapped run of styled, selectable text.
    fn inline(&mut self, inline: &Inline) -> AnyElement {
        let spot = self.next_spot();
        let mut highlights = Vec::new();
        let mut fonts: Vec<(_, SharedString)> = Vec::new();
        let mut links = Vec::new();
        for span in &inline.spans {
            let mut style = HighlightStyle::default();
            if span.style.bold {
                style.font_weight = Some(FontWeight::SEMIBOLD);
            }
            if span.style.italic {
                style.font_style = Some(FontStyle::Italic);
            }
            if span.style.strike {
                style.strikethrough = Some(StrikethroughStyle { thickness: gpui::px(1.), color: None });
            }
            if span.style.code {
                style.background_color = Some(self.p.element);
                fonts.push((span.range.clone(), mono()));
            }
            if let Some(url) = &span.link {
                style.color = Some(self.p.accent);
                style.underline =
                    Some(UnderlineStyle { thickness: gpui::px(1.), color: Some(self.p.accent), wavy: false });
                links.push((span.range.clone(), Link::Url(url.clone())));
            }
            highlights.push((span.range.clone(), style));
        }
        // Paths to files in the project (`src/app/mod.rs:123`) open in the editor.
        if let Some(cwd) = &self.cwd {
            let files: Vec<_> = file_links::find(cwd, &inline.text, self.cx)
                .into_iter()
                .filter(|(r, _)| !links.iter().any(|(u, _)| u.start < r.end && r.start < u.end))
                .collect();
            if !files.is_empty() {
                let accent = HighlightStyle { color: Some(self.p.accent), ..Default::default() };
                let over = files.iter().map(|(r, _)| (r.clone(), accent)).collect::<Vec<_>>();
                highlights = gpui::combine_highlights(highlights, over).collect();
                links.extend(files);
            }
        }
        selectable(self.select.0, spot, inline.text.clone(), self.p.text_selection)
            .highlights(highlights)
            .fonts(fonts)
            .links(links, self.p.accent)
            .into_any_element()
    }

    /// A code block: a header row with the language, Run and Copy above the code,
    /// so they never cover it; lines stay unwrapped and the code scrolls sideways
    /// (a scrollbar, Shift+wheel, or dragging a selection past its edge).
    fn code_block(&mut self, lang: &str, code: &str) -> AnyElement {
        let copy_id = self.next_id("copy");
        let scroll_id = self.next_id("code");
        let bar_id = self.next_id("code-bar");
        let scroll = code_scroll(self.select, self.counter);
        let to_copy = code.to_string();
        // Suggested shell commands get a play button; it stays visible so it's found.
        let play = ShellKind::for_lang(lang).zip(self.run.clone()).map(|(shell, run)| {
            let command = code.to_string();
            let success = self.p.success;
            div()
                .id(self.next_id("run"))
                .flex()
                .items_center()
                .justify_center()
                .size(px(22.))
                .rounded_md()
                .cursor_pointer()
                .hover(move |s| s.bg(success.opacity(0.15)))
                .child(icon(Icon::Play, success.opacity(0.8)).size_3())
                .tooltip(Tooltip::with_meta(format!("Run in {}", shell.label()), "In this project's terminal, below"))
                .on_click(move |_, window, cx| run(shell, command.clone(), window, cx))
        });
        let (group, spot) = (self.select.0, self.next_spot());
        let styles = code::highlighted(Lang::from_fence(lang), code, &self.p.syntax);
        let text = selectable(group, spot, code.to_string(), self.p.text_selection).highlights(styles);
        let p = &self.p;
        let header = div()
            .flex()
            .items_center()
            .justify_end()
            .gap_1()
            .h(px(28.))
            .pl_3()
            .pr_1()
            .when(!lang.is_empty(), |d| {
                d.child(div().flex_1().text_size(px(11.)).text_color(p.text_placeholder).child(lang.to_string()))
            })
            .child(
                div().invisible().group_hover("code", |s| s.visible()).child(
                    icon_button(copy_id, Icon::Copy, self.cx)
                        .tooltip(Tooltip::text("Copy"))
                        .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(to_copy.clone()))),
                ),
            )
            .children(play);
        let follow = scroll.clone();
        div()
            .group("code")
            .flex()
            .flex_col()
            .rounded_md()
            .border_1()
            .border_color(p.border_variant)
            .bg(p.element)
            .child(header)
            .child(
                div()
                    .relative()
                    .child(
                        div()
                            .id(scroll_id)
                            .overflow_x_scroll()
                            // A wheel turned over it scrolls the transcript, not the code sideways.
                            .restrict_scroll_to_axis()
                            .track_scroll(&scroll)
                            .px_3()
                            .pb_3()
                            .font_family(mono())
                            .text_size(px(13.))
                            // As wide as its longest line, so there is something to scroll.
                            .flex()
                            .child(div().flex_none().whitespace_nowrap().child(text)),
                    )
                    .child(
                        canvas(
                            |_, _, _| {},
                            move |bounds, _, window, _| follow_drag(follow, bounds, group, spot, window),
                        )
                        .absolute()
                        .inset_0(),
                    )
                    .child(scrollbar::scrollbar(bar_id, scroll, Axis::Horizontal)),
            )
            .into_any_element()
    }

    fn table(&mut self, columns: usize, rows: &[TableRow]) -> AnyElement {
        let id = self.next_id("table");
        let rows: Vec<_> = rows
            .iter()
            .map(|row| (row.head, row.cells.iter().take(columns).map(|c| self.blocks(c)).collect::<Vec<_>>()))
            .collect();
        let p = &self.p;
        div()
            .id(id)
            .overflow_x_scroll()
            .restrict_scroll_to_axis()
            .child(div().flex().flex_col().rounded_md().border_1().border_color(p.border_variant).children(
                rows.into_iter().enumerate().map(|(i, (head, cells))| {
                    div()
                        .flex()
                        .when(i > 0, |d| d.border_t_1().border_color(p.border_variant))
                        .when(head, |d| d.bg(p.element).font_weight(FontWeight::SEMIBOLD))
                        .children(cells.into_iter().map(|c| {
                            div().flex_1().min_w(px(80.)).px_2().py_1().child(div().flex().flex_col().children(c))
                        }))
                }),
            ))
            .into_any_element()
    }
}
