//! How the field draws: its text shaped into wrapped lines, the runs that colour
//! it (IME composition, the owner's marks), the cursor and the selection.

use gpui::{
    App, Bounds, ElementId, ElementInputHandler, Entity, GlobalElementId, LayoutId, PaintQuad, Pixels, Point,
    SharedString, Style, TextRun, UnderlineStyle, Window, WrappedLine, fill, point, prelude::*, px, relative, size,
};

use super::TextInput;
use crate::theme::theme;

/// The shaped text: one wrapped line per `\n`-separated line, with the byte
/// offset each starts at.
pub(super) struct Layout {
    pub(super) lines: Vec<WrappedLine>,
    pub(super) starts: Vec<usize>,
    pub(super) line_height: Pixels,
}

impl Layout {
    pub(super) fn line_for_offset(&self, offset: usize) -> usize {
        self.starts.partition_point(|&s| s <= offset).saturating_sub(1)
    }

    pub(super) fn line_top(&self, ix: usize) -> Pixels {
        self.lines[..ix].iter().map(|l| l.size(self.line_height).height).sum()
    }

    pub(super) fn position_for_offset(&self, offset: usize) -> Point<Pixels> {
        let ix = self.line_for_offset(offset);
        let local = offset - self.starts[ix];
        let p = self.lines[ix].position_for_index(local, self.line_height).unwrap_or_default();
        point(p.x, p.y + self.line_top(ix))
    }

    pub(super) fn offset_for_position(&self, position: Point<Pixels>) -> usize {
        let mut top = px(0.);
        for (ix, line) in self.lines.iter().enumerate() {
            let height = line.size(self.line_height).height;
            if position.y < top + height || ix == self.lines.len() - 1 {
                let local = point(position.x, (position.y - top).max(px(0.)).min(height - px(0.5)));
                let i = match line.closest_index_for_position(local, self.line_height) {
                    Ok(i) | Err(i) => i,
                };
                return self.starts[ix] + i;
            }
            top += height;
        }
        0
    }
}

pub(super) struct TextElement {
    pub(super) input: Entity<TextInput>,
}

pub(super) struct PrepaintState {
    layout: Option<Layout>,
    cursor: Option<PaintQuad>,
    selections: Vec<PaintQuad>,
}

impl IntoElement for TextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

/// What to show and how: the text (or placeholder when empty) and its runs.
fn display_runs(input: &TextInput, window: &Window, cx: &App) -> (SharedString, Vec<TextRun>) {
    let style = window.text_style();
    let (text, color): (SharedString, _) = if input.content.is_empty() {
        (input.placeholder.clone(), theme(cx).text_placeholder)
    } else {
        (input.shown().into(), style.color)
    };
    let run = TextRun {
        len: text.len(),
        font: style.font(),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    if input.content.is_empty() {
        return (text, vec![run]);
    }
    // The owner's marks that still fit the text, and the IME composition underlined.
    let fits = |r: &std::ops::Range<usize>| {
        r.start < r.end && r.end <= text.len() && text.is_char_boundary(r.start) && text.is_char_boundary(r.end)
    };
    let marks: Vec<_> = if input.masked { Vec::new() } else { input.marks.iter().filter(|m| fits(&m.range)).collect() };
    let composing = input.marked_range.clone().filter(fits);
    let mut cuts: Vec<usize> = vec![0, text.len()];
    cuts.extend(marks.iter().flat_map(|m| [m.range.start, m.range.end]));
    cuts.extend(composing.iter().flat_map(|r| [r.start, r.end]));
    cuts.sort_unstable();
    cuts.dedup();
    let runs =
        cuts.windows(2)
            .map(|w| {
                let mark = marks.iter().find(|m| m.range.start <= w[0] && w[0] < m.range.end);
                let underline = composing
                    .as_ref()
                    .filter(|r| r.start <= w[0] && w[0] < r.end)
                    .map(|_| UnderlineStyle { color: Some(run.color), thickness: px(1.0), wavy: false });
                TextRun {
                    len: w[1] - w[0],
                    color: mark.map_or(run.color, |m| m.color),
                    background_color: mark.and_then(|m| m.background),
                    underline,
                    ..run.clone()
                }
            })
            .collect();
    (text, runs)
}

/// The font size and line height in effect, read where the field's text
/// style applies (not inside a layout measure, which runs outside it).
fn metrics(window: &Window) -> (Pixels, Pixels) {
    (window.text_style().font_size.to_pixels(window.rem_size()), window.line_height())
}

fn shape(
    text: SharedString,
    runs: &[TextRun],
    width: Option<Pixels>,
    metrics: (Pixels, Pixels),
    window: &Window,
) -> Layout {
    let (font_size, line_height) = metrics;
    let lines: Vec<WrappedLine> = window
        .text_system()
        .shape_text(text.clone(), font_size, runs, width, None)
        .map(|l| l.into_vec())
        .unwrap_or_default();
    let mut starts = Vec::with_capacity(lines.len());
    let mut at = 0;
    for line in &lines {
        starts.push(at);
        at += line.len() + 1;
    }
    if lines.is_empty() {
        starts.push(0);
    }
    Layout { lines, starts, line_height }
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let input = self.input.read(cx);
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        if !input.multiline {
            style.size.height = window.line_height().into();
            return (window.request_layout(style, [], cx), ());
        }
        let (text, runs) = display_runs(input, window, cx);
        let metrics = metrics(window);
        let layout_id = window.request_measured_layout(style, move |known, available, window, _cx| {
            let width = known.width.or(match available.width {
                gpui::AvailableSpace::Definite(w) => Some(w),
                _ => None,
            });
            let layout = shape(text.clone(), &runs, width, metrics, window);
            let height = layout.line_top(layout.lines.len()).max(layout.line_height);
            size(width.unwrap_or_default(), height)
        });
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let input = self.input.read(cx);
        let (text, runs) = display_runs(input, window, cx);
        let wrap = input.multiline.then_some(bounds.size.width);
        let layout = shape(text, &runs, wrap, metrics(window), window);
        let lh = layout.line_height;
        let selection_color = theme(cx).selection;
        let range = input.selected_range.clone();

        let mut selections = Vec::new();
        let mut cursor = None;
        if input.content.is_empty() || range.is_empty() {
            let offset = if input.content.is_empty() { 0 } else { input.cursor_offset() };
            let p = layout.position_for_offset(offset);
            let width = crate::ui::device_px(1.5, window);
            cursor = Some(fill(Bounds::new(bounds.origin + p, size(width, lh)), theme(cx).accent));
        } else {
            // One rectangle per visual line the selection touches.
            let start = layout.position_for_offset(range.start);
            let end = layout.position_for_offset(range.end);
            let mut y = start.y;
            while y <= end.y {
                let x0 = if y == start.y { start.x } else { px(0.) };
                let x1 = if y == end.y { end.x } else { bounds.size.width };
                if x1 > x0 || y != end.y {
                    let x1 = if x1 <= x0 { x0 + px(4.) } else { x1 };
                    selections.push(fill(
                        Bounds::from_corners(bounds.origin + point(x0, y), bounds.origin + point(x1, y + lh)),
                        selection_color,
                    ));
                }
                y += lh;
            }
        }
        PrepaintState { layout: Some(layout), cursor, selections }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.input.read(cx).focus_handle.clone();
        window.handle_input(&focus_handle, ElementInputHandler::new(bounds, self.input.clone()), cx);
        for selection in prepaint.selections.drain(..) {
            window.paint_quad(selection)
        }
        let layout = prepaint.layout.take().unwrap();
        let mut y = px(0.);
        for line in &layout.lines {
            let _ = line.paint(
                bounds.origin + point(px(0.), y),
                layout.line_height,
                gpui::TextAlign::Left,
                None,
                window,
                cx,
            );
            y += line.size(layout.line_height).height;
        }
        if focus_handle.is_focused(window)
            && let Some(cursor) = prepaint.cursor.take()
        {
            window.paint_quad(cursor);
        }
        self.input.update(cx, |input, _cx| {
            input.reveal_cursor(&layout, bounds, window);
            input.last_layout = Some(layout);
            input.last_bounds = Some(bounds);
        });
    }
}

impl TextInput {
    /// Scrolls the area around the field just enough to show the cursor,
    /// when it or the text changed since the last frame.
    fn reveal_cursor(&mut self, layout: &Layout, bounds: Bounds<Pixels>, window: &mut Window) {
        let key = (self.cursor_offset(), self.content.len());
        if self.revealed == Some(key) {
            return;
        }
        self.revealed = Some(key);
        let view = self.scroll.bounds();
        if view.size.height <= px(0.) {
            return;
        }
        let top = bounds.origin.y + layout.position_for_offset(key.0).y;
        let bottom = top + layout.line_height;
        let mut offset = self.scroll.offset();
        let y = if bottom > view.bottom() {
            offset.y - (bottom - view.bottom())
        } else if top < view.top() {
            offset.y + (view.top() - top)
        } else {
            return;
        };
        offset.y = y.clamp(-self.scroll.max_offset().y, px(0.));
        self.scroll.set_offset(offset);
        window.refresh();
    }
}
