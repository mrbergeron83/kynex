//! Text you can select with the mouse and copy.
//!
//! Each [`Selectable`] is one block of text (a paragraph, a code block, a
//! message) at a [`Spot`] in a group, e.g. one transcript. A drag can start in
//! one block and end in another; every block in between is selected whole. The
//! selection lives in a global, so the blocks need no state of their own and
//! the transcript can re-render (or stream) under it.

use std::collections::BTreeMap;
use std::ops::Range;

use gpui::{
    App, Bounds, CursorStyle, DispatchPhase, Element, ElementId, GlobalElementId, Hitbox, HitboxBehavior, Hsla,
    InspectorElementId, IntoElement, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels,
    SharedString, StyledText, Window, fill, point,
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

/// One selectable block of styled text. Links in it open on a plain click.
pub struct Selectable {
    group: u64,
    spot: Spot,
    text: StyledText,
    content: SharedString,
    links: Vec<(Range<usize>, String)>,
    color: Hsla,
}

/// `content` must be the same string `text` was built from.
pub fn selectable(group: u64, spot: Spot, content: impl Into<SharedString>, color: Hsla) -> Selectable {
    let content = content.into();
    Selectable { group, spot, text: StyledText::new(content.clone()), content, links: Vec::new(), color }
}

impl Selectable {
    /// Styles the text, e.g. `|t| t.with_highlights(…)`.
    pub fn styled(mut self, f: impl FnOnce(StyledText) -> StyledText) -> Self {
        self.text = f(self.text);
        self
    }

    pub fn links(mut self, links: Vec<(Range<usize>, String)>) -> Self {
        self.links = links;
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

        // The highlight goes under the glyphs.
        let range = cx.try_global::<Selection>().and_then(|s| s.range_in(group, spot, self.content.len()));
        if let Some(range) = range {
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
        self.text.paint(None, inspector_id, bounds, state, &mut (), window, cx);

        let over_link = layout
            .index_for_position(window.mouse_position())
            .is_ok_and(|ix| self.links.iter().any(|(r, _)| r.contains(&ix)));
        window.set_cursor_style(if over_link { CursorStyle::PointingHand } else { CursorStyle::IBeam }, hitbox);

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
                    && let Some((_, url)) = links.iter().find(|(r, _)| r.contains(&ix))
                {
                    cx.open_url(url);
                }
            }
        });
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
