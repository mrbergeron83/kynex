//! What follows the pointer while a pane is dragged by its handle: onto another pane to
//! move it there, or onto a pane tab. Here rather than in the app so views with a bar of
//! their own (the Git pane, terminals) can make that bar the handle.

use gpui::{Context, SharedString, Window, prelude::*};
use kynex_core::layout::PaneId;

use crate::theme::{theme, ui_font};
use crate::ui::{popover, px};

#[derive(Clone)]
pub struct DraggedPane {
    pub pane: PaneId,
    pub label: SharedString,
}

impl Render for DraggedPane {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        // Drawn outside the window's root, so it doesn't inherit the font.
        popover(cx)
            .font_family(ui_font())
            .flex_row()
            .items_center()
            .h(px(26.))
            .px_2p5()
            .text_sm()
            .text_color(t.text)
            .child(self.label.clone())
    }
}

/// Makes `el` the handle that drags pane `pane`, shown as `label`; nothing without a pane.
pub fn pane_handle<E: StatefulInteractiveElement>(el: E, pane: Option<PaneId>, label: &str) -> E {
    match pane {
        Some(pane) => {
            let drag = DraggedPane { pane, label: label.to_string().into() };
            el.on_drag(drag, |d, _, _, cx| cx.new(|_| d.clone()))
        }
        None => el,
    }
}
