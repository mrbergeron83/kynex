//! The palette's state filters: a row of chips under the field, with what each lets through.

use gpui::{AnyElement, Context, prelude::*};
use kynex_core::palette::Filter;

use super::KynexApp;
use crate::theme::Palette;
use crate::ui::px;

/// The chips for `current`, each with its count; clicking one picks it.
pub(super) fn filter_chips(current: Filter, counts: [usize; 5], t: &Palette, cx: &mut Context<KynexApp>) -> AnyElement {
    let chips = Filter::ALL.into_iter().zip(counts).map(|(f, n)| {
        let on = f == current;
        gpui::div()
            .id(("palette-filter", f as usize))
            .flex()
            .items_center()
            .gap_1()
            .h(px(20.))
            .px_2()
            .rounded_md()
            .cursor_pointer()
            .text_xs()
            .text_color(if on { t.text } else { t.text_muted })
            .when(on, |d| d.bg(t.active))
            .when(!on, |d| d.hover(|d| d.bg(t.hover)))
            .child(f.label())
            .child(gpui::div().text_color(t.text_placeholder).child(n.to_string()))
            .on_click(cx.listener(move |this, _, _, cx| this.palette_set_filter(f, cx)))
    });
    gpui::div()
        .flex()
        .items_center()
        .gap_1()
        .px_2()
        .py_1()
        .border_b_1()
        .border_color(t.border_variant)
        .children(chips)
        .into_any_element()
}
