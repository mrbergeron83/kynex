//! The file viewer for files that aren't text: a picture before and after the change
//! (a click opens it in the image viewer), a PDF's pages with zoom, and a placeholder
//! with Open / Show in folder for any other binary. Pictures are decoded and PDF pages
//! drawn off the UI thread (`kynex_core::file_view`); nothing in a file is run.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui::{AnyElement, Context, KeyDownEvent, ObjectFit, RenderImage, SharedString, div, img, prelude::*};
use kynex_core::file_view::{self, Loaded, Picture, Side, pdf};

use super::file_modal::{Body, FileDiffEvent, FileDiffView, note};
use crate::theme::theme;
use crate::ui::image_viewer::render_image;
use crate::ui::{Icon, Tooltip, ghost_button, icon, icon_button, outline_button, px, spinner};

/// PDF pages are drawn this many pixels wide: sharp at the base width on a HiDPI screen.
const PDF_RENDER_WIDTH: u32 = 1200;
/// A PDF page's width on screen at zoom 1.
const PDF_PAGE_WIDTH: f32 = 760.;
const PDF_ZOOMS: [f32; 9] = [0.5, 0.67, 0.8, 1., 1.25, 1.5, 2., 2.5, 3.];

/// One side of a picture, ready to draw.
pub(super) enum Shown {
    Missing,
    Note(SharedString),
    Ready { image: Arc<RenderImage>, size: (u32, u32) },
}

impl From<Side<Picture>> for Shown {
    fn from(side: Side<Picture>) -> Self {
        match side {
            Side::Missing => Shown::Missing,
            Side::TooLarge(n) => Shown::Note(format!("Too large to show ({})", file_view::size_label(n)).into()),
            Side::Failed(e) => Shown::Note(e.into()),
            Side::Ready(p) => Shown::Ready { size: p.size, image: render_image(p.rgba) },
        }
    }
}

impl Shown {
    /// Takes the picture out, to hand its texture back.
    pub(super) fn image(&mut self) -> Option<Arc<RenderImage>> {
        match std::mem::replace(self, Shown::Missing) {
            Shown::Ready { image, .. } => Some(image),
            other => {
                *self = other;
                None
            }
        }
    }
}

/// A loaded file with its pictures made ready to draw, off the UI thread.
pub(super) enum Prepared {
    Pictures(Shown, Shown),
    Other(Loaded),
}

pub(super) fn prepare(loaded: Loaded) -> Prepared {
    match loaded {
        Loaded::Pictures { before, after } => Prepared::Pictures(before.into(), after.into()),
        other => Prepared::Other(other),
    }
}

enum PdfMsg {
    Page(Arc<RenderImage>, (u32, u32)),
    Done(Result<usize, String>),
}

/// A PDF's pages as they come from the renderer process.
pub(super) struct PdfPages {
    pages: Vec<(Arc<RenderImage>, (u32, u32))>,
    /// The document's page count, once drawn.
    total: Option<usize>,
    error: Option<String>,
    done: bool,
    /// Stops the renderer; set when these pages are dropped.
    cancel: Arc<AtomicBool>,
}

impl Drop for PdfPages {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl PdfPages {
    /// Starts drawing `bytes`' pages; they arrive in the view's body as they're drawn.
    pub(super) fn start(bytes: Vec<u8>, cx: &mut Context<FileDiffView>) -> Self {
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = async_channel::unbounded::<PdfMsg>();
        let stop = cancel.clone();
        cx.background_spawn(async move {
            let result = match std::env::current_exe() {
                Ok(exe) => pdf::render(&exe, bytes, PDF_RENDER_WIDTH, &stop, |page| {
                    let size = page.rgba.dimensions();
                    let _ = tx.send_blocking(PdfMsg::Page(render_image(page.rgba), size));
                }),
                Err(e) => Err(format!("Couldn't start the PDF renderer: {e}")),
            };
            let _ = tx.send_blocking(PdfMsg::Done(result));
        })
        .detach();
        let mine = cancel.clone();
        cx.spawn(async move |this, cx| {
            while let Ok(msg) = rx.recv().await {
                let alive = this.update(cx, |this, cx| {
                    // Still these pages: not another side or file loaded since.
                    let Body::Pdf(p) = &mut this.body else { return };
                    if !Arc::ptr_eq(&p.cancel, &mine) {
                        return;
                    }
                    match msg {
                        PdfMsg::Page(image, size) => p.pages.push((image, size)),
                        PdfMsg::Done(Ok(total)) => {
                            p.total = Some(total);
                            p.done = true;
                        }
                        PdfMsg::Done(Err(e)) => {
                            if e != kynex_core::git::CANCELLED {
                                p.error = Some(e);
                            }
                            p.done = true;
                        }
                    }
                    cx.notify();
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
        Self { pages: Vec::new(), total: None, error: None, done: false, cancel }
    }

    pub(super) fn take_images(&mut self) -> Vec<Arc<RenderImage>> {
        std::mem::take(&mut self.pages).into_iter().map(|(image, _)| image).collect()
    }
}

impl FileDiffView {
    fn set_pdf_zoom(&mut self, zoom: f32, cx: &mut Context<Self>) {
        self.pdf_zoom = zoom.clamp(PDF_ZOOMS[0], PDF_ZOOMS[PDF_ZOOMS.len() - 1]);
        cx.notify();
    }

    fn step_pdf_zoom(&mut self, up: bool, cx: &mut Context<Self>) {
        let z = self.pdf_zoom;
        let next = if up {
            PDF_ZOOMS.iter().copied().find(|&s| s > z + 0.01)
        } else {
            PDF_ZOOMS.iter().rev().copied().find(|&s| s < z - 0.01)
        };
        if let Some(next) = next {
            self.set_pdf_zoom(next, cx);
        }
    }

    /// + / − / 0 zoom a PDF.
    pub(super) fn pdf_key(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        let m = &ev.keystroke.modifiers;
        if m.alt {
            return false;
        }
        match ev.keystroke.key.as_str() {
            "=" | "+" => self.step_pdf_zoom(true, cx),
            "-" => self.step_pdf_zoom(false, cx),
            "0" => self.set_pdf_zoom(1., cx),
            _ => return false,
        }
        true
    }

    /// The header's zoom buttons, for a PDF.
    pub(super) fn pdf_controls(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let Body::Pdf(p) = &self.body else { return None };
        let t = theme(cx).clone();
        let pages = match (p.total, p.done) {
            (Some(n), _) if n > p.pages.len() => format!("first {} of {n} pages", p.pages.len()),
            (Some(n), _) => format!("{n} page{}", if n == 1 { "" } else { "s" }),
            (None, _) => format!("{} pages", p.pages.len()),
        };
        Some(
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap_1()
                .child(div().text_xs().text_color(t.text_muted).mr_2().child(pages))
                .child(
                    icon_button("pdf-zoom-out", Icon::Minus, cx)
                        .tooltip(Tooltip::with_meta("Zoom out", "−"))
                        .on_click(cx.listener(|this, _, _, cx| this.step_pdf_zoom(false, cx))),
                )
                .child(
                    div()
                        .w(px(40.))
                        .text_center()
                        .text_xs()
                        .text_color(t.text_muted)
                        .child(format!("{}%", (self.pdf_zoom * 100.).round())),
                )
                .child(
                    icon_button("pdf-zoom-in", Icon::Plus, cx)
                        .tooltip(Tooltip::with_meta("Zoom in", "+"))
                        .on_click(cx.listener(|this, _, _, cx| this.step_pdf_zoom(true, cx))),
                )
                .child(
                    ghost_button("pdf-zoom-fit", cx)
                        .text_xs()
                        .child("100%")
                        .tooltip(Tooltip::with_meta("Actual size", "0"))
                        .on_click(cx.listener(|this, _, _, cx| this.set_pdf_zoom(1., cx))),
                )
                .into_any_element(),
        )
    }

    pub(super) fn render_pdf(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx).clone();
        let Body::Pdf(p) = &self.body else { return div().into_any_element() };
        if p.pages.is_empty()
            && let Some(e) = &p.error
        {
            return note(e.clone(), t.error).into_any_element();
        }
        let width = PDF_PAGE_WIDTH * self.pdf_zoom;
        let pages = p.pages.iter().enumerate().map(|(i, (image, (w, h)))| {
            let height = width * *h as f32 / (*w).max(1) as f32;
            let (image, size) = (image.clone(), (*w, *h));
            div()
                .id(("pdf-page", i))
                .flex_none()
                .w(px(width))
                .h(px(height))
                .border_1()
                .border_color(t.border_variant)
                .shadow_sm()
                .cursor_pointer()
                .child(img(image.clone()).size_full().object_fit(ObjectFit::Fill))
                .on_click(cx.listener(move |_, _, _, cx| cx.emit(FileDiffEvent::ViewImage(image.clone(), size))))
        });
        let tail = if !p.done {
            Some(div().py_4().child(spinner("pdf-drawing", t.text_placeholder, px(16.))).into_any_element())
        } else {
            p.error.clone().map(|e| div().py_2().text_sm().text_color(t.error).child(e).into_any_element())
        };
        div()
            .id("pdf-pages")
            .flex_1()
            .min_h_0()
            .overflow_scroll()
            .bg(t.panel)
            .child(div().flex().flex_col().items_center().min_w_full().gap_4().p_4().children(pages).children(tail))
            .into_any_element()
    }

    pub(super) fn render_pictures(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx).clone();
        let Body::Pictures { before, after } = &self.body else { return div().into_any_element() };
        let both = !before.is_missing_shown() && !after.is_missing_shown();
        let panel = |id: &'static str, label: &'static str, shown: &Shown, cx: &mut Context<Self>| {
            let body = match shown {
                Shown::Missing => return None,
                Shown::Note(text) => note(text.clone(), t.text_placeholder).into_any_element(),
                Shown::Ready { image, size } => {
                    let (image, size) = (image.clone(), *size);
                    div()
                        .id(id)
                        .flex_1()
                        .min_h_0()
                        .p_3()
                        .cursor_pointer()
                        .child(img(image.clone()).size_full().object_fit(ObjectFit::ScaleDown))
                        .tooltip(Tooltip::text("Click to zoom"))
                        .on_click(
                            cx.listener(move |_, _, _, cx| cx.emit(FileDiffEvent::ViewImage(image.clone(), size))),
                        )
                        .into_any_element()
                }
            };
            let dims = match shown {
                Shown::Ready { size: (w, h), .. } => format!("{w} × {h}"),
                _ => String::new(),
            };
            Some(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .rounded_md()
                    .border_1()
                    .border_color(t.border_variant)
                    .bg(t.panel)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .h(px(28.))
                            .px_3()
                            .border_b_1()
                            .border_color(t.border_variant)
                            .text_xs()
                            .text_color(t.text_muted)
                            .when(both, |d| d.child(label))
                            .child(div().flex_1())
                            .child(dims),
                    )
                    .child(body),
            )
        };
        let left = panel("picture-before", "Before", before, cx);
        let right = panel("picture-after", "After", after, cx);
        div().flex().flex_1().min_h_0().gap_3().p_3().children(left).children(right).into_any_element()
    }

    /// A file that isn't shown: what it is, its size, and Open / Show in folder.
    pub(super) fn render_binary(&mut self, what: &'static str, size: u64, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx).clone();
        let file = self.root.join(&self.path);
        let exists = file.is_file();
        let runnable = file_view::could_run(&self.path);
        let (open, reveal) = (file.clone(), file);
        div()
            .flex_1()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_3()
            .child(icon(Icon::File, t.text_placeholder).size_8())
            .child(div().text_sm().text_color(t.text_muted).child(format!("{what}, {}", file_view::size_label(size))))
            .when(exists, |d| {
                d.child(
                    div()
                        .flex()
                        .gap_2()
                        .when(!runnable, |d| {
                            d.child(
                                outline_button("binary-open", "Open with default app", cx)
                                    .tooltip(Tooltip::text("Open it in the app your system uses for this kind of file"))
                                    .on_click(move |_, _, cx| {
                                        let f = open.clone();
                                        cx.background_spawn(async move { kynex_core::open::open_default(&f) }).detach();
                                    }),
                            )
                        })
                        .child(outline_button("binary-reveal", "Show in folder", cx).on_click(move |_, _, cx| {
                            let f = reveal.clone();
                            cx.background_spawn(async move { kynex_core::open::reveal(&f) }).detach();
                        })),
                )
            })
            .into_any_element()
    }
}

impl Shown {
    fn is_missing_shown(&self) -> bool {
        matches!(self, Shown::Missing)
    }
}
