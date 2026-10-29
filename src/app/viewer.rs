//! Images opened over the window: a sent message's, a composer attachment, a
//! tool result's (a screenshot, a `Read` of a picture). The viewer itself is
//! `ui::image_viewer`; this opens and closes it, and draws the thumbnails
//! that open it.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use gpui::{AnyElement, Context, DismissEvent, ElementId, Focusable, Image, Window, div, img, prelude::*, px};
use kynex_core::attach;
use kynex_core::session::Preview;

use super::KynexApp;
use crate::theme::theme;
use crate::ui::Tooltip;
use crate::ui::image_viewer::ImageViewer;
use crate::ui::motion::{self, fade};

impl KynexApp {
    pub(super) fn open_image(&mut self, image: Arc<Image>, window: &mut Window, cx: &mut Context<Self>) {
        let viewer = cx.new(|cx| ImageViewer::new(image, window, cx));
        cx.subscribe_in(&viewer, window, |this, _, _: &DismissEvent, window, cx| this.close_image(window, cx)).detach();
        self.viewer = Some(viewer);
        cx.notify();
    }

    pub(super) fn close_image(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.viewer.take().is_some() {
            self.composer.read(cx).focus_handle(cx).focus(window, cx);
            cx.notify();
        }
    }

    pub(super) fn render_image_viewer(&self, leaving: Option<f32>) -> Option<AnyElement> {
        let viewer = self.viewer.clone()?;
        let id = ("viewer-in", viewer.entity_id().as_u64() as usize);
        Some(fade(id, div().absolute().inset_0().child(viewer), motion::QUICK, leaving))
    }

    /// Thumbnails of `images` that open in the viewer, `max_h` tall at most.
    pub(super) fn thumbnails(
        &self,
        id: ElementId,
        images: &[Preview],
        max_h: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let t = theme(cx).clone();
        // Previews are `gpui::Image`s (the composer's, or `ui::image_viewer::preview`).
        let images = images.iter().filter_map(|i| i.clone().downcast::<Image>().ok());
        div().id(id).flex().flex_wrap().gap_2().children(images.enumerate().map(|(n, image)| {
            // A known size, not the decoded one: an image still decoding has
            // none, and the transcript would keep its row measured without it,
            // so the picture vanished once a later row came in (#106).
            let sized = match size_of(&image) {
                Some(size) => {
                    let (w, h) = attach::fit(size, max_h * 2., max_h);
                    img(image.clone()).w(px(w)).h(px(h))
                }
                None => img(image.clone()).max_h(px(max_h)).max_w(px(max_h * 2.)),
            };
            div()
                .id(n)
                .rounded_md()
                .border_1()
                .border_color(t.border_variant)
                .overflow_hidden()
                .cursor_pointer()
                .hover(|s| s.opacity(0.85))
                .child(sized)
                .tooltip(Tooltip::text("Click to view"))
                .on_click(cx.listener(move |this, _, window, cx| this.open_image(image.clone(), window, cx)))
        }))
    }
}

thread_local! {
    /// Image sizes by `Image::id`, so frames don't read headers again.
    static SIZES: RefCell<HashMap<u64, Option<(u32, u32)>>> = RefCell::default();
}

/// An image's size in pixels, from its header.
fn size_of(image: &Image) -> Option<(u32, u32)> {
    SIZES.with_borrow_mut(|sizes| *sizes.entry(image.id).or_insert_with(|| attach::dimensions(&image.bytes)))
}
