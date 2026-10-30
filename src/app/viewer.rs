//! Images opened over the window: a sent message's, a composer attachment, a
//! tool result's (a screenshot, a `Read` of a picture). The viewer itself is
//! `ui::image_viewer`; this opens and closes it, and draws the thumbnails
//! that open it.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::ui::px;
use gpui::{
    AnyElement, Context, DismissEvent, ElementId, Focusable, Image, ObjectFit, RenderImage, Window, div, img,
    prelude::*,
};
use kynex_core::attach;
use kynex_core::lru::Lru;
use kynex_core::session::Preview;
use kynex_core::session_log::LoggedImage;
use kynex_core::toasts::Toast;

use super::KynexApp;
use crate::theme::theme;
use crate::ui::Tooltip;
use crate::ui::image_viewer::{ImageViewer, preview};
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

    /// Opens an image that's still in its session log, once it's read.
    pub(super) fn open_logged(&mut self, image: LoggedImage, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            let media_type = image.media_type.clone();
            let bytes = gpui::AppContext::background_spawn(cx, async move { image.load() }).await;
            let image = bytes.ok().and_then(|b| preview(&media_type, b)).and_then(|p| p.downcast::<Image>().ok());
            this.update_in(cx, |this, window, cx| match image {
                Some(image) => this.open_image(image, window, cx),
                None => {
                    this.toast(Toast::error("Couldn't read that image from the session log."));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Thumbnails of `images` that open in the viewer, `max_h` tall at most.
    /// Images still in a session log are read and scaled down off the UI
    /// thread as they're drawn; until then their box holds their place.
    pub(super) fn thumbnails(
        &self,
        id: ElementId,
        images: &[Preview],
        max_h: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let t = theme(cx).clone();
        let frame = |n: usize| {
            div()
                .id(n)
                .rounded_md()
                .border_1()
                .border_color(t.border_variant)
                .overflow_hidden()
                .cursor_pointer()
                .hover(|s| s.opacity(0.85))
                .tooltip(Tooltip::text("Click to view"))
        };
        let thumbs = images.iter().enumerate().filter_map(|(n, p)| {
            // Previews are `gpui::Image`s (the composer's, or `ui::image_viewer::preview`),
            // or `LoggedImage`s for those read back from a log.
            if let Ok(image) = p.clone().downcast::<Image>() {
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
                let open = cx.listener(move |this, _, window, cx| this.open_image(image.clone(), window, cx));
                return Some(frame(n).child(sized).on_click(open));
            }
            let logged = p.downcast_ref::<LoggedImage>()?.clone();
            // Its box is the same size before and after it's read, for the same reason.
            let size = logged.size.unwrap_or(((max_h * 1.6) as u32, max_h as u32));
            let (w, h) = attach::fit(size, max_h * 2., max_h);
            let key = (logged.clone(), max_h as u32);
            let ready = self.logged.borrow_mut().cache.get(&key).cloned();
            let body = match ready {
                Some(thumb) => img(thumb).w(px(w)).h(px(h)).object_fit(ObjectFit::Contain).into_any_element(),
                None => {
                    let failed = self.load_thumbnail(key, (w, h), cx);
                    div()
                        .w(px(w))
                        .h(px(h))
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(t.panel)
                        .text_xs()
                        .text_color(t.text_placeholder)
                        .when(failed, |d| d.child("Image unavailable"))
                        .into_any_element()
                }
            };
            let open = cx.listener(move |this, _, window, cx| this.open_logged(logged.clone(), window, cx));
            Some(frame(n).child(body).on_click(open))
        });
        div().id(id).flex().flex_wrap().gap_2().children(thumbs)
    }

    /// Reads a logged image and scales it to a `box_size` thumbnail (twice
    /// that in pixels, for sharp HiDPI) off the UI thread, unless it's on its
    /// way already. True if it can't be read.
    fn load_thumbnail(&self, key: (LoggedImage, u32), (w, h): (f32, f32), cx: &mut Context<Self>) -> bool {
        {
            let mut logged = self.logged.borrow_mut();
            if logged.failed.contains(&key.0) {
                return true;
            }
            if !logged.loading.insert(key.clone()) {
                return false;
            }
        }
        let (tw, th) = ((w * 2.).round() as u32, (h * 2.).round() as u32);
        let image = key.0.clone();
        cx.spawn(async move |this, cx| {
            let thumb =
                cx.background_spawn(async move { attach::thumbnail(&image.load().ok()?, tw.max(1), th.max(1)) }).await;
            this.update(cx, |this, cx| {
                let gone = {
                    let mut logged = this.logged.borrow_mut();
                    logged.loading.remove(&key);
                    match thumb {
                        Some(rgba) => {
                            let size = rgba.len();
                            logged.cache.insert(key, render_image(rgba), size)
                        }
                        None => {
                            logged.failed.insert(key.0);
                            Vec::new()
                        }
                    }
                };
                // Thumbnails scrolled far away give their textures back.
                for thumb in gone {
                    cx.drop_image(thumb, None);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        false
    }
}

/// Screenshots read back from session logs, scaled down to thumbnails as
/// they're shown, the most recently drawn kept.
pub(super) struct LoggedImages {
    /// By image and the thumbnail height it was made for.
    cache: Lru<(LoggedImage, u32), Arc<RenderImage>>,
    loading: HashSet<(LoggedImage, u32)>,
    failed: HashSet<LoggedImage>,
}

/// Bytes of thumbnails kept decoded: a few screens' worth.
const THUMBNAILS_CAP: usize = 64 * 1024 * 1024;

impl Default for LoggedImages {
    fn default() -> Self {
        Self { cache: Lru::new(THUMBNAILS_CAP), loading: HashSet::new(), failed: HashSet::new() }
    }
}

/// An RGBA picture as gpui draws it (BGRA).
fn render_image(mut rgba: image::RgbaImage) -> Arc<RenderImage> {
    for pixel in rgba.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    Arc::new(RenderImage::new(vec![image::Frame::new(rgba)]))
}

thread_local! {
    /// Image sizes by `Image::id`, so frames don't read headers again.
    static SIZES: RefCell<HashMap<u64, Option<(u32, u32)>>> = RefCell::default();
}

/// An image's size in pixels, from its header.
fn size_of(image: &Image) -> Option<(u32, u32)> {
    SIZES.with_borrow_mut(|sizes| *sizes.entry(image.id).or_insert_with(|| attach::dimensions(&image.bytes)))
}
