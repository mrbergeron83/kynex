//! The file viewer's text: the diff's rows with a caret and a selection over them, moved
//! with the keys and dragged with the mouse, copied without the +/- signs. Read-only:
//! nothing types into it. The caret's rules are `kynex_core::file_view::caret`.

use gpui::{
    AnyElement, ClipboardItem, Context, Hsla, KeyDownEvent, ListHorizontalSizingBehavior, MouseButton, MouseDownEvent,
    MouseMoveEvent, Pixels, Point, ScrollStrategy, ShapedLine, SharedString, TextRun, Window, anchored, deferred, div,
    font, prelude::*, uniform_list,
};
use kynex_core::file_view::caret::{Caret, Lines, Move, Pos, selected_text};
use kynex_core::git::LineKind;

use super::file_modal::FileDiffView;
use crate::git::diff_view::{GUTTER, ROW_HEIGHT, Row, TEXT_SIZE, render_row};
use crate::theme::{mono, theme};
use crate::ui::{menu_item, popover, px, scrollbar};

/// The diff's rows as lines for the caret: only the file's own lines are text.
pub(super) struct RowLines<'a>(pub &'a [Row]);

impl Lines for RowLines<'_> {
    fn count(&self) -> usize {
        self.0.len()
    }

    fn text(&self, row: usize) -> Option<&str> {
        match self.0.get(row)? {
            Row::Line { kind, text, .. } if *kind != LineKind::NoNewline => Some(text.as_ref()),
            _ => None,
        }
    }
}

/// The right-click menu, where it was opened.
pub(super) struct TextMenu {
    at: Point<Pixels>,
}

/// `n` layout pixels (at 100% UI zoom) as measured pixels at the window's zoom.
fn real(n: f32, window: &Window) -> Pixels {
    px(n).to_pixels(window.rem_size())
}

/// A row's text laid out as it's drawn, for finding columns under the pointer.
fn shape(text: &SharedString, window: &Window) -> ShapedLine {
    let run = TextRun {
        len: text.len(),
        font: font(mono()),
        color: Hsla::default(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    window.text_system().shape_line(text.clone(), real(TEXT_SIZE, window), &[run], None)
}

impl FileDiffView {
    fn lines_do<R>(&self, f: impl FnOnce(&RowLines) -> R) -> R {
        let rows = self.state.rows();
        f(&RowLines(&rows))
    }

    fn edit_caret(&mut self, f: impl FnOnce(&RowLines, &mut Caret)) {
        let rows = self.state.rows();
        f(&RowLines(&rows), &mut self.caret);
    }

    /// The row and column under a window position.
    fn pos_at(&self, at: Point<Pixels>, window: &Window) -> Pos {
        let handle = self.scroll.0.borrow().base_handle.clone();
        let (bounds, offset) = (handle.bounds(), handle.offset());
        let rows = self.state.rows();
        let y = f32::from(at.y - bounds.top() - offset.y);
        let row =
            ((y / f32::from(real(ROW_HEIGHT, window))).floor().max(0.) as usize).min(rows.len().saturating_sub(1));
        let x = at.x - bounds.left() - offset.x - real(GUTTER, window);
        let col = match rows.get(row) {
            Some(Row::Line { text, .. }) => shape(text, window).closest_index_for_x(x.max(Pixels::ZERO)),
            _ => 0,
        };
        Pos::new(row, col)
    }

    fn mouse_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        let pos = self.pos_at(ev.position, window);
        let (count, shift) = (ev.click_count, ev.modifiers.shift);
        self.edit_caret(|l, c| match count {
            1 if shift => c.drag_to(l, pos),
            1 => {
                c.drag_to(l, pos);
                *c = Caret::at(c.head);
            }
            2 => *c = Caret::select_word(l, pos),
            _ => *c = Caret::select_line(l, pos.row),
        });
        self.dragging = ev.click_count == 1;
        cx.notify();
    }

    fn mouse_move(&mut self, ev: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.dragging {
            return;
        }
        if ev.pressed_button != Some(MouseButton::Left) {
            self.dragging = false;
            return;
        }
        let pos = self.pos_at(ev.position, window);
        let before = self.caret.head;
        self.edit_caret(|l, c| c.drag_to(l, pos));
        // Dragged past the top or bottom: the list follows.
        let bounds = self.scroll.0.borrow().base_handle.bounds();
        if ev.position.y < bounds.top() || ev.position.y > bounds.bottom() {
            self.scroll.scroll_to_item(self.caret.head.row, ScrollStrategy::Nearest);
        }
        if self.caret.head != before {
            cx.notify();
        }
    }

    fn copy(&self, cx: &mut Context<Self>) {
        if let Some(text) = self.lines_do(|l| selected_text(l, &self.caret)) {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    /// A key in the text: caret moves, select all, copy. True if it was one.
    pub(super) fn text_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let k = &ev.keystroke;
        let m = &k.modifiers;
        let secondary = m.secondary(cx);
        let mac = cfg!(target_os = "macos");
        // Word moves: Alt on macOS, Ctrl elsewhere.
        let word = if mac { m.alt } else { m.control };
        let height = self.scroll.0.borrow().base_handle.bounds().size.height;
        let page = (f32::from(height) / f32::from(real(ROW_HEIGHT, window))) as usize;
        let mv = match k.key.as_str() {
            "c" if secondary => {
                self.copy(cx);
                return true;
            }
            "insert" if m.control => {
                self.copy(cx);
                return true;
            }
            "a" if secondary => {
                self.edit_caret(|l, c| c.select_all(l));
                cx.notify();
                return true;
            }
            "left" if mac && m.platform => Move::LineStart,
            "right" if mac && m.platform => Move::LineEnd,
            "up" if mac && m.platform => Move::DocStart,
            "down" if mac && m.platform => Move::DocEnd,
            "left" if word => Move::WordLeft,
            "right" if word => Move::WordRight,
            "left" => Move::Left,
            "right" => Move::Right,
            "up" if !m.alt => Move::Up,
            "down" if !m.alt => Move::Down,
            "pageup" => Move::PageUp(page.saturating_sub(1).max(1)),
            "pagedown" => Move::PageDown(page.saturating_sub(1).max(1)),
            "home" if m.control => Move::DocStart,
            "end" if m.control => Move::DocEnd,
            "home" => Move::LineStart,
            "end" => Move::LineEnd,
            _ => return false,
        };
        let extend = m.shift;
        self.edit_caret(|l, c| c.move_by(l, mv, extend));
        self.reveal_caret(window);
        cx.notify();
        true
    }

    /// Scrolls so the caret is on screen, up and down and sideways.
    fn reveal_caret(&mut self, window: &Window) {
        let head = self.caret.head;
        self.scroll.scroll_to_item(head.row, ScrollStrategy::Nearest);
        let rows = self.state.rows();
        let Some(Row::Line { text, .. }) = rows.get(head.row) else { return };
        let x = shape(text, window).x_for_index(head.col);
        let handle = self.scroll.0.borrow().base_handle.clone();
        let (width, mut offset) = (handle.bounds().size.width, handle.offset());
        let margin = real(24., window);
        let room = width - real(GUTTER, window) - margin;
        if x < -offset.x {
            offset.x = -(x - margin).max(Pixels::ZERO);
        } else if x > -offset.x + room {
            offset.x = -(x - room);
        }
        handle.set_offset(offset);
    }

    pub(super) fn render_text(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx).clone();
        let rows = self.state.rows();
        let scroll = self.scroll.clone();
        let caret = self.caret;
        let focused = self.focus.is_focused(window);
        let list = uniform_list("file-diff-rows", rows.len(), move |range, window, _| {
            // The gutter stays put by moving back as far as the list moved sideways.
            let scrolled = -f32::from(scroll.0.borrow().base_handle.offset().x);
            range
                .map(|i| {
                    let row = render_row(&rows[i], scrolled, &t);
                    let Row::Line { text, .. } = &rows[i] else { return row };
                    let selected = caret.row_range(i, text.len());
                    let on_caret = focused && caret.head.row == i;
                    if selected.is_none() && !on_caret {
                        return row;
                    }
                    let line = shape(text, window);
                    let gutter = f32::from(real(GUTTER, window));
                    let at = |ix: usize| gutter + f32::from(line.x_for_index(ix));
                    let band = selected.map(|r| {
                        // A selection that runs on to the next row shows past the line's end.
                        let end = at(r.end) + if caret.takes_newline(i) { f32::from(real(6., window)) } else { 0. };
                        div()
                            .absolute()
                            .top_0()
                            .h_full()
                            .left(gpui::px(at(r.start)))
                            .w(gpui::px((end - at(r.start)).max(0.)))
                            .bg(t.selection)
                    });
                    let bar = (on_caret && at(caret.head.col) >= scrolled + gutter).then(|| {
                        div()
                            .absolute()
                            .top(px(2.))
                            .h(px(ROW_HEIGHT - 4.))
                            .left(gpui::px(at(caret.head.col)))
                            .w(gpui::px(1.5))
                            .bg(t.accent)
                    });
                    // The band goes under the row (its gutter covers it when scrolled), the caret over it.
                    div().relative().children(band).child(row).children(bar).into_any_element()
                })
                .collect()
        })
        .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
        .with_width_from_item(Some(self.state.widest()))
        .track_scroll(&self.scroll)
        .flex_1();
        div()
            .id("file-diff-text")
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .cursor_text()
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| {
                    this.dragging = false;
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                    // A right click outside the selection moves the caret there first.
                    let pos = this.pos_at(ev.position, window);
                    let inside = this.caret.range().is_some_and(|(lo, hi)| lo <= pos && pos <= hi);
                    if !inside {
                        this.caret = Caret::at(pos);
                    }
                    this.menu = Some(TextMenu { at: ev.position });
                    cx.notify();
                }),
            )
            .child(list)
            .child(scrollbar::vertical("file-diff-bar", self.scroll.clone()))
            .child(scrollbar::scrollbar("file-diff-hbar", self.scroll.clone(), gpui::Axis::Horizontal))
            .into_any_element()
    }

    /// Copy / Select all, at the pointer.
    pub(super) fn render_text_menu(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let at = self.menu.as_ref()?.at;
        let has = self.caret.range().is_some();
        let copy = menu_item("file-text-copy", "Copy", false, cx).when(!has, |d| d.opacity(0.5)).on_click(cx.listener(
            |this, _, _, cx| {
                this.copy(cx);
                this.menu = None;
                cx.notify();
            },
        ));
        let all = menu_item("file-text-all", "Select all", false, cx).on_click(cx.listener(|this, _, _, cx| {
            this.edit_caret(|l, c| c.select_all(l));
            this.menu = None;
            cx.notify();
        }));
        Some(
            deferred(
                anchored().position(at).snap_to_window_with_margin(gpui::px(8.)).child(
                    popover(cx)
                        .id("file-text-menu")
                        .w(px(160.))
                        .occlude()
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                            this.menu = None;
                            cx.notify();
                        }))
                        .child(copy)
                        .child(all),
                ),
            )
            .into_any_element(),
        )
    }
}
