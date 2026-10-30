//! Draws a terminal grid straight onto gpui's GPU scene: cell backgrounds as
//! quads, each styled run as one shaped line pinned to the cell grid, then the
//! cursor. Sizing happens here too: the grid follows the element's bounds.

use gpui::{
    App, Bounds, Element, ElementId, ElementInputHandler, Entity, FocusHandle, Font, FontStyle, FontWeight,
    GlobalElementId, Hsla, IntoElement, LayoutId, PaintQuad, Pixels, Rgba, ShapedLine, SharedString,
    StrikethroughStyle, Style, TextRun, UnderlineStyle, Window, fill, font, outline, point, px, relative, size,
};
use kynex_core::terminal::{CursorStyle, Palette, Rgb, Size};

use super::view::TerminalView;
use crate::theme::{Palette as Theme, mono, theme};
use crate::ui::device_px;

/// The terminal font size at normal size; it follows the UI zoom like the rest.
const FONT_SIZE: gpui::Rems = gpui::Rems(13. / 16.);
/// Space between the grid and the panel's edges, at normal size.
const PADDING: gpui::Rems = gpui::Rems(10. / 16.);

fn font_size(window: &Window) -> Pixels {
    FONT_SIZE.to_pixels(window.rem_size())
}

pub struct TerminalElement {
    view: Entity<TerminalView>,
    focus: FocusHandle,
    focused: bool,
}

impl TerminalElement {
    pub fn new(view: Entity<TerminalView>, focus: FocusHandle, focused: bool) -> Self {
        Self { view, focus, focused }
    }
}

pub struct Prepaint {
    backgrounds: Vec<PaintQuad>,
    lines: Vec<(gpui::Point<Pixels>, ShapedLine)>,
    cursor: Option<PaintQuad>,
    line_height: Pixels,
}

/// The cell size for the terminal font. Rows are a whole number of device
/// pixels tall, so at 125% or 175% every line sits the same distance apart.
pub fn cell_size(window: &mut Window) -> gpui::Size<Pixels> {
    let font_id = window.text_system().resolve_font(&font(mono()));
    let font_size = font_size(window);
    let width = window.text_system().advance(font_id, font_size, 'm').map_or(px(8.), |s| s.width);
    size(width, device_px(f32::from(font_size) * 1.4, window))
}

impl IntoElement for TerminalElement {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for TerminalElement {
    type RequestLayoutState = ();
    type PrepaintState = Prepaint;

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
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Prepaint {
        let cell = cell_size(window);
        let (font_size, padding) = (font_size(window), PADDING.to_pixels(window.rem_size()));
        let inner = size(bounds.size.width - padding * 2., bounds.size.height - padding * 2.);
        let grid = Size {
            columns: (inner.width / cell.width).floor().max(2.) as usize,
            lines: (inner.height / cell.height).floor().max(1.) as usize,
        };
        let origin = bounds.origin + point(padding, padding);
        let t = theme(cx).clone();
        let palette = palette(&t);
        let snapshot = self.view.update(cx, |view, _| {
            view.layout = Some((origin, cell));
            view.resize(grid);
            view.snapshot(&palette)
        });
        let Some(snapshot) = snapshot else {
            return Prepaint { backgrounds: Vec::new(), lines: Vec::new(), cursor: None, line_height: cell.height };
        };

        let base = font(mono());
        let mut backgrounds = Vec::new();
        let mut lines = Vec::new();
        for (row_ix, row) in snapshot.rows.iter().enumerate() {
            let y = origin.y + cell.height * row_ix as f32;
            let mut at = 0;
            for run in &row.runs {
                let text = &row.text[at..at + run.len];
                at += run.len;
                let x = origin.x + cell.width * run.column as f32;
                if let Some(bg) = run.bg {
                    backgrounds.push(fill(
                        Bounds::new(point(x, y), size(cell.width * run.cells as f32, cell.height)),
                        hsla(bg),
                    ));
                }
                if text.trim().is_empty() && !run.underline && !run.strike {
                    continue;
                }
                let color = hsla(run.fg);
                let style = TextRun {
                    len: text.len(),
                    font: Font {
                        weight: if run.bold { FontWeight::BOLD } else { FontWeight::NORMAL },
                        style: if run.italic { FontStyle::Italic } else { FontStyle::Normal },
                        ..base.clone()
                    },
                    color,
                    background_color: None,
                    underline: run.underline.then_some(UnderlineStyle {
                        color: Some(color),
                        thickness: px(1.),
                        wavy: false,
                    }),
                    strikethrough: run.strike.then_some(StrikethroughStyle { color: Some(color), thickness: px(1.) }),
                };
                // Pin every glyph to the grid; a wide character spans two cells.
                let width = if run.wide { cell.width * 2. } else { cell.width };
                let shaped = window.text_system().shape_line(
                    SharedString::from(text.to_string()),
                    font_size,
                    &[style],
                    Some(width),
                );
                lines.push((point(x, y), shaped));
            }
        }

        let cursor = snapshot.cursor.map(|c| {
            let at = origin + point(cell.width * c.column as f32, cell.height * c.line as f32);
            let color = hsla(palette.cursor);
            let block = Bounds::new(at, cell);
            let bar = device_px(2., window);
            match (c.style, self.focused) {
                (CursorStyle::Beam, _) => fill(Bounds::new(at, size(bar, cell.height)), color),
                (CursorStyle::Underline, _) => {
                    fill(Bounds::new(at + point(px(0.), cell.height - bar), size(cell.width, bar)), color)
                }
                (CursorStyle::Block, true) => fill(block, color.opacity(0.75)),
                _ => outline(block, color, gpui::BorderStyle::Solid),
            }
        });
        Prepaint { backgrounds, lines, cursor, line_height: cell.height }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        prepaint: &mut Prepaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        window.handle_input(&self.focus, ElementInputHandler::new(bounds, self.view.clone()), cx);
        window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
            for quad in prepaint.backgrounds.drain(..) {
                window.paint_quad(quad);
            }
            // The block cursor sits under the text so the character stays readable.
            if let Some(cursor) = prepaint.cursor.take() {
                window.paint_quad(cursor);
            }
            for (origin, line) in &prepaint.lines {
                let _ = line.paint(*origin, prepaint.line_height, gpui::TextAlign::Left, None, window, cx);
            }
        });
    }
}

fn hsla(c: Rgb) -> Hsla {
    Rgba { r: c.r as f32 / 255., g: c.g as f32 / 255., b: c.b as f32 / 255., a: 1. }.into()
}

fn rgb(c: Hsla) -> Rgb {
    let c = c.to_rgb();
    let v = |f: f32| (f.clamp(0., 1.) * 255.).round() as u8;
    Rgb { r: v(c.r), g: v(c.g), b: v(c.b) }
}

/// The terminal's colours, from the app theme: its surfaces and accent, with
/// ANSI colours tuned for a dark or light background.
pub fn palette(t: &Theme) -> Palette {
    let background = rgb(t.terminal_background());
    Palette {
        ansi: t.ansi().map(Rgb::hex),
        foreground: rgb(t.text),
        background,
        cursor: rgb(t.accent),
        // The theme's selection is translucent; flatten it onto the background.
        selection: background.mix(rgb(t.selection), t.selection.a.max(0.35)),
    }
}

pub fn background(t: &Theme) -> Hsla {
    t.terminal_background()
}
