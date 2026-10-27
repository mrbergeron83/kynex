//! A list of completions for what's being typed, e.g. slash commands in the
//! composer. The owner keeps the items and the selection and handles the keys;
//! this only draws them and reports clicks.

use std::rc::Rc;

use gpui::{App, SharedString, Window, div, prelude::*, px};

use super::popover;
use crate::theme::{mono, theme};

/// One row: what's inserted, what it takes after it, and what it does.
pub struct Completion {
    pub label: SharedString,
    pub hint: SharedString,
    pub detail: SharedString,
}

/// Rows shown at once; the window scrolls to keep the selection in view.
const ROWS: usize = 8;

/// The list, with `selected` highlighted, or `empty` when there are no items.
pub fn completions(
    items: &[Completion],
    selected: usize,
    empty: &str,
    cx: &App,
    on_pick: impl Fn(usize, &mut Window, &mut App) + 'static,
) -> gpui::Div {
    let t = theme(cx).clone();
    let on_pick = Rc::new(on_pick);
    let start = (selected + 1).saturating_sub(ROWS);
    let rows = items.iter().enumerate().skip(start).take(ROWS).map(|(i, c)| {
        let on_pick = on_pick.clone();
        let hover = t.hover;
        div()
            .id(("completion", i))
            .flex()
            .items_center()
            .gap_2()
            .h(px(26.))
            .px_2()
            .rounded_md()
            .text_sm()
            .cursor_pointer()
            .hover(move |s| s.bg(hover))
            .when(i == selected, |d| d.bg(t.active))
            .child(div().flex_none().font_family(mono()).text_color(t.text).child(c.label.clone()))
            .when(!c.hint.is_empty(), |d| {
                d.child(
                    div()
                        .flex_none()
                        .font_family(mono())
                        .text_xs()
                        .text_color(t.text_placeholder)
                        .child(c.hint.clone()),
                )
            })
            .child(div().flex_1().min_w_0().truncate().text_xs().text_color(t.text_muted).child(c.detail.clone()))
            .on_click(move |_, window, cx| on_pick(i, window, cx))
    });
    let more = items.len().saturating_sub(start + ROWS);
    popover(cx)
        .when(items.is_empty(), |d| {
            d.child(div().px_2().py_1().text_sm().text_color(t.text_muted).child(empty.to_string()))
        })
        .children(rows)
        .when(more > 0, |d| {
            d.child(div().px_2().pt_1().text_xs().text_color(t.text_placeholder).child(format!("{more} more")))
        })
}
