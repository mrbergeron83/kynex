//! Text you can select with the mouse and copy.
//!
//! Each [`Selectable`] is one block of text (a paragraph, a code block, a
//! message) at a [`Spot`] in a group, e.g. one transcript. A drag can start in
//! one block and end in another; every block in between is selected whole. The
//! selection lives in a global, so the blocks need no state of their own and
//! the transcript can re-render (or stream) under it.

use std::collections::BTreeMap;
use std::ops::Range;
use std::path::PathBuf;

use gpui::{
    App, Bounds, CursorStyle, DispatchPhase, Element, ElementId, GlobalElementId, HighlightStyle, Hitbox,
    HitboxBehavior, Hsla, InspectorElementId, IntoElement, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Pixels, SharedString, StyledText, Window, fill, point,
};

/// Where a block sits: blocks are ordered by item, then by block within it.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Spot {
    pub item: usize,
    pub block: usize,
}

impl Spot {
    pub fn new(item: usize, block: usize) -> Self {
        Self { item, block }
    }
}

#[derive(Default)]
struct Selection {
    group: u64,
    anchor: Option<(Spot, usize)>,
    head: Option<(Spot, usize)>,
    dragging: bool,
    /// The last-painted text of each block in the group, so a copy can take
    /// in the blocks between the two ends.
    texts: BTreeMap<Spot, SharedString>,
}

impl gpui::Global for Selection {}

impl Selection {
    /// Both ends in order, if something is selected.
    fn ends(&self) -> Option<((Spot, usize), (Spot, usize))> {
        let (a, h) = (self.anchor?, self.head?);
        if a == h {
            return None;
        }
        Some(if a <= h { (a, h) } else { (h, a) })
    }

    /// The selected bytes of one block.
    fn range_in(&self, group: u64, spot: Spot, len: usize) -> Option<Range<usize>> {
        if group != self.group {
            return None;
        }
        let (start, end) = self.ends()?;
        if spot < start.0 || spot > end.0 {
            return None;
        }
        let lo = if spot == start.0 { start.1.min(len) } else { 0 };
        let hi = if spot == end.0 { end.1.min(len) } else { len };
        (lo < hi).then_some(lo..hi)
    }
}

/// The selected text, blocks separated by blank lines.
pub fn selected_text(cx: &App) -> Option<String> {
    let sel = cx.try_global::<Selection>()?;
    let (start, end) = sel.ends()?;
    let parts: Vec<&str> = sel
        .texts
        .range(start.0..=end.0)
        .filter_map(|(spot, text)| {
            let range = sel.range_in(sel.group, *spot, text.len())?;
            text.get(range)
        })
        .collect();
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

/// A drag-selection is under way that started in block `spot` of `group`.
pub fn dragging_from(group: u64, spot: Spot, cx: &App) -> bool {
    cx.try_global::<Selection>()
        .is_some_and(|s| s.dragging && s.group == group && s.anchor.is_some_and(|a| a.0 == spot))
}

/// Drops the selection, e.g. when the transcript under it changes.
pub fn clear(cx: &mut App) {
    if let Some(sel) = cx.try_global::<Selection>()
        && sel.anchor.is_none()
    {
        return;
    }
    let sel = cx.default_global::<Selection>();
    sel.anchor = None;
    sel.head = None;
    sel.dragging = false;
}

/// Where a link in the text goes.
#[derive(Clone, Debug)]
pub enum Link {
    Url(String),
    /// A file in the editor, at a line when known. Underlined only on hover,
    /// so prose with many paths doesn't fill with lines.
    File {
        path: PathBuf,
        line: Option<u32>,
        col: Option<u32>,
    },
}

impl Link {
    fn follow(&self, cx: &mut App) {
        match self {
            Link::Url(url) => cx.open_url(url),
            Link::File { path, line, col } => crate::file_links::open(path.clone(), *line, *col, cx),
        }
    }
}

/// One selectable block of styled text. Links in it open on a plain click.
pub struct Selectable {
    group: u64,
    spot: Spot,
    /// Built at layout, with the selection laid over `highlights`.
    text: StyledText,
    content: SharedString,
    highlights: Vec<(Range<usize>, HighlightStyle)>,
    fonts: Vec<(Range<usize>, SharedString)>,
    links: Vec<(Range<usize>, Link)>,
    color: Hsla,
    /// The colour of the underline under a hovered file link.
    underline: Hsla,
    /// Find-in-transcript matches to wash, found at prepaint.
    marks: Vec<(Range<usize>, Hsla)>,
}

pub fn selectable(group: u64, spot: Spot, content: impl Into<SharedString>, color: Hsla) -> Selectable {
    let content = content.into();
    Selectable {
        group,
        spot,
        text: StyledText::new(content.clone()),
        content,
        highlights: Vec::new(),
        fonts: Vec::new(),
        links: Vec::new(),
        color,
        underline: color,
        marks: Vec::new(),
    }
}

impl Selectable {
    /// Styles ranges of the text: colours, weights, backgrounds.
    pub fn highlights(mut self, highlights: Vec<(Range<usize>, HighlightStyle)>) -> Self {
        self.highlights = highlights;
        self
    }

    /// Other font families for ranges of the text (inline code).
    pub fn fonts(mut self, fonts: Vec<(Range<usize>, SharedString)>) -> Self {
        self.fonts = fonts;
        self
    }

    /// Links over ranges of the text; file links underline in `underline` on hover.
    pub fn links(mut self, links: Vec<(Range<usize>, Link)>, underline: Hsla) -> Self {
        self.links = links;
        self.underline = underline;
        self
    }
}

impl IntoElement for Selectable {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for Selectable {
    type RequestLayoutState = ();
    type PrepaintState = Hitbox;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        // Text with a background of its own (inline code) takes the selection's
        // colour where selected, so the selection reads as one band.
        let selected = cx.try_global::<Selection>().and_then(|s| s.range_in(self.group, self.spot, self.content.len()));
        let mut highlights = std::mem::take(&mut self.highlights);
        if let Some(sel) = selected {
            highlights = recolor_selected(highlights, sel, self.color);
        }
        self.text = StyledText::new(self.content.clone())
            .with_highlights(highlights)
            .with_font_family_overrides(std::mem::take(&mut self.fonts));
        self.text.request_layout(None, inspector_id, window, cx)
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Hitbox {
        self.text.prepaint(None, inspector_id, bounds, state, window, cx);
        let sel = cx.default_global::<Selection>();
        if sel.group == self.group {
            sel.texts.insert(self.spot, self.content.clone());
        }
        self.marks = super::find_marks::in_block(self.group, self.spot, &self.content, cx);
        window.insert_hitbox(bounds, HitboxBehavior::Normal)
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut (),
        hitbox: &mut Hitbox,
        window: &mut Window,
        cx: &mut App,
    ) {
        let layout = self.text.layout().clone();
        let (group, spot, color) = (self.group, self.spot, self.color);

        // Find matches, then the selection, go under the glyphs.
        for (range, wash) in &self.marks {
            paint_band(&layout, bounds, range.clone(), *wash, window);
        }
        let range = cx.try_global::<Selection>().and_then(|s| s.range_in(group, spot, self.content.len()));
        if let Some(range) = range {
            paint_band(&layout, bounds, range, color, window);
        }
        self.text.paint(None, inspector_id, bounds, state, &mut (), window, cx);

        let link_at = |layout: &gpui::TextLayout, links: &[(Range<usize>, Link)], pos| {
            layout.index_for_position(pos).ok().and_then(|ix| links.iter().position(|(r, _)| r.contains(&ix)))
        };
        let hovered = link_at(&layout, &self.links, window.mouse_position()).filter(|_| hitbox.is_hovered(window));
        window.set_cursor_style(if hovered.is_some() { CursorStyle::PointingHand } else { CursorStyle::IBeam }, hitbox);
        // A hovered file link underlines, line by line if it wraps.
        if let Some((range, Link::File { .. })) = hovered.map(|i| &self.links[i]) {
            let line = layout.line_height();
            if let (Some(start), Some(end)) =
                (layout.position_for_index(range.start), layout.position_for_index(range.end))
            {
                let mut y = start.y;
                while y <= end.y + px_eps() {
                    let left = if (y - start.y).abs() < px_eps() { start.x } else { bounds.left() };
                    let right = if (y - end.y).abs() < px_eps() { end.x } else { bounds.right() };
                    if right > left {
                        let under = Bounds::from_corners(
                            point(left, y + line - gpui::px(2.)),
                            point(right, y + line - gpui::px(1.)),
                        );
                        window.paint_quad(fill(under, self.underline));
                    }
                    y += line;
                }
            }
        }
        // Redraw when the pointer moves onto or off a link.
        if !self.links.is_empty() {
            let (hitbox, layout, links) = (hitbox.clone(), layout.clone(), self.links.clone());
            window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, _| {
                if phase == DispatchPhase::Bubble {
                    let now = link_at(&layout, &links, e.position).filter(|_| hitbox.is_hovered(window));
                    if now != hovered {
                        window.refresh();
                    }
                }
            });
        }

        // Any left click drops the old selection first; the block under the
        // pointer then starts a new one.
        window.on_mouse_event(|e: &MouseDownEvent, phase, _, cx| {
            if phase == DispatchPhase::Capture && e.button == MouseButton::Left {
                clear(cx);
            }
        });
        window.on_mouse_event({
            let (hitbox, layout, content) = (hitbox.clone(), layout.clone(), self.content.clone());
            move |e: &MouseDownEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble || e.button != MouseButton::Left || !hitbox.is_hovered(window) {
                    return;
                }
                let ix = layout.index_for_position(e.position).unwrap_or_else(|ix| ix).min(content.len());
                let (from, to) = match e.click_count {
                    1 => (ix, ix),
                    2 => word_at(&content, ix),
                    _ => (0, content.len()),
                };
                let sel = cx.default_global::<Selection>();
                if sel.group != group {
                    sel.group = group;
                    sel.texts.clear();
                    sel.texts.insert(spot, content.clone());
                }
                sel.anchor = Some((spot, from));
                sel.head = Some((spot, to));
                sel.dragging = e.click_count == 1;
                window.refresh();
            }
        });
        window.on_mouse_event({
            let layout = layout.clone();
            let len = self.content.len();
            move |e: &MouseMoveEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble || e.position.y < bounds.top() || e.position.y >= bounds.bottom() {
                    return;
                }
                let Some(sel) = cx.try_global::<Selection>() else { return };
                if !sel.dragging || sel.group != group || !e.dragging() {
                    return;
                }
                let ix = layout.index_for_position(e.position).unwrap_or_else(|ix| ix).min(len);
                if sel.head != Some((spot, ix)) {
                    cx.default_global::<Selection>().head = Some((spot, ix));
                    window.refresh();
                }
            }
        });
        window.on_mouse_event({
            let (hitbox, links) = (hitbox.clone(), std::mem::take(&mut self.links));
            move |e: &MouseUpEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble || e.button != MouseButton::Left {
                    return;
                }
                let Some(sel) = cx.try_global::<Selection>() else { return };
                let clicked =
                    sel.group == group && sel.anchor == sel.head && sel.anchor.is_some_and(|(s, _)| s == spot);
                if sel.dragging {
                    cx.default_global::<Selection>().dragging = false;
                }
                // A click that didn't drag follows a link under it.
                if clicked
                    && hitbox.is_hovered(window)
                    && let Ok(ix) = layout.index_for_position(e.position)
                    && let Some((_, link)) = links.iter().find(|(r, _)| r.contains(&ix))
                {
                    link.follow(cx);
                }
            }
        });
    }
}

/// `highlights` with the selected part of any that has a background split off
/// and given the selection's colour instead. Ranges stay in order.
fn recolor_selected(
    highlights: Vec<(Range<usize>, HighlightStyle)>,
    sel: Range<usize>,
    color: Hsla,
) -> Vec<(Range<usize>, HighlightStyle)> {
    let mut out = Vec::with_capacity(highlights.len());
    for (r, style) in highlights {
        let (lo, hi) = (r.start.max(sel.start), r.end.min(sel.end));
        if style.background_color.is_none() || lo >= hi {
            out.push((r, style));
            continue;
        }
        if r.start < lo {
            out.push((r.start..lo, style));
        }
        out.push((lo..hi, HighlightStyle { background_color: Some(color), ..style }));
        if hi < r.end {
            out.push((hi..r.end, style));
        }
    }
    out
}

/// Fills behind the text in `range`, line by line where it wraps.
fn paint_band(
    layout: &gpui::TextLayout,
    bounds: Bounds<Pixels>,
    range: Range<usize>,
    color: Hsla,
    window: &mut Window,
) {
    let line = layout.line_height();
    let start = layout.position_for_index(range.start).unwrap_or(bounds.origin);
    let end = layout.position_for_index(range.end).unwrap_or(point(bounds.right(), bounds.bottom() - line));
    let mut y = start.y;
    while y <= end.y + px_eps() {
        let first = (y - start.y).abs() < px_eps();
        let last = (y - end.y).abs() < px_eps();
        let left = if first { start.x } else { bounds.left() };
        let right = if last { end.x } else { bounds.right() };
        if right > left {
            window.paint_quad(fill(Bounds::from_corners(point(left, y), point(right, y + line)), color));
        }
        y += line;
    }
}

fn px_eps() -> Pixels {
    gpui::px(0.5)
}

/// The word (or run of punctuation, or of spaces) around byte `ix`.
fn word_at(text: &str, ix: usize) -> (usize, usize) {
    fn class(c: char) -> u8 {
        if c.is_alphanumeric() || c == '_' {
            0
        } else if c.is_whitespace() {
            1
        } else {
            2
        }
    }
    let ix = (0..=ix.min(text.len())).rev().find(|i| text.is_char_boundary(*i)).unwrap_or(0);
    let Some(c) = text[ix..].chars().next().or_else(|| text[..ix].chars().next_back()) else { return (ix, ix) };
    let k = class(c);
    let start = text[..ix].char_indices().rev().take_while(|(_, c)| class(*c) == k).last().map_or(ix, |(i, _)| i);
    let end = text[ix..].char_indices().find(|(_, c)| class(*c) != k).map_or(text.len(), |(i, _)| ix + i);
    (start, end.max(start))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words() {
        let t = "let foo_bar = baz();";
        assert_eq!(word_at(t, 5), (4, 11));
        assert_eq!(word_at(t, 4), (4, 11));
        assert_eq!(word_at(t, 0), (0, 3));
        assert_eq!(word_at(t, t.len()), (t.len() - 3, t.len()));
    }

    #[test]
    fn selection_recolors_backgrounds_it_covers() {
        let code = HighlightStyle { background_color: Some(gpui::red()), ..Default::default() };
        let bold = HighlightStyle { font_weight: Some(gpui::FontWeight::BOLD), ..Default::default() };
        let out = recolor_selected(vec![(0..4, bold), (6..12, code)], 8..20, gpui::blue());
        let ranges: Vec<_> = out.iter().map(|(r, s)| (r.clone(), s.background_color)).collect();
        assert_eq!(ranges, [(0..4, None), (6..8, Some(gpui::red())), (8..12, Some(gpui::blue()))]);
        assert_eq!(out[0].1.font_weight, Some(gpui::FontWeight::BOLD));
    }

    #[test]
    fn ranges_span_blocks() {
        let mut s = Selection { group: 1, ..Default::default() };
        s.anchor = Some((Spot::new(2, 1), 3));
        s.head = Some((Spot::new(0, 4), 5));
        assert_eq!(s.range_in(1, Spot::new(0, 4), 10), Some(5..10));
        assert_eq!(s.range_in(1, Spot::new(1, 0), 10), Some(0..10));
        assert_eq!(s.range_in(1, Spot::new(2, 1), 10), Some(0..3));
        assert_eq!(s.range_in(1, Spot::new(2, 2), 10), None);
        assert_eq!(s.range_in(2, Spot::new(1, 0), 10), None);
    }
}
