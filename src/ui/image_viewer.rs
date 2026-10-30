//! A picture over the whole window, to look at closely: fitted at first,
//! zoomed with the wheel or + / − about the pointer, dragged to pan; 0 fits
//! it again, 1 shows it pixel for pixel. Esc or a click beside it closes it
//! (a [`DismissEvent`]). The zoom and pan rules are `kynex_core::zoom`.

use std::sync::Arc;

use gpui::{
    App, Context, CursorStyle, DismissEvent, EventEmitter, FocusHandle, Focusable, Image, ImageFormat, KeyDownEvent,
    MouseButton, MouseDownEvent, MouseMoveEvent, ObjectFit, Pixels, Point, ScrollEvent, Size, Window, div, img,
    prelude::*, px,
};
use kynex_core::session::Preview;
use kynex_core::zoom::{STEP, Zoom};

use super::{Icon, Tooltip, ghost_button, icon_button, popover};
use crate::theme::theme;

/// Room left around the picture, and under it for the toolbar.
const MARGIN: f32 = 32.;
const TOOLBAR: f32 = 48.;
/// Wheel pixels per doubling of the zoom: a 120px notch is about ×1.3.
const WHEEL_PER_DOUBLING: f32 = 320.;

/// Makes the previews `kynex_core::session` keeps for images it finds in
/// messages (`set_preview_maker`): `gpui::Image`s, like the composer's.
pub fn preview(media_type: &str, bytes: Vec<u8>) -> Option<Preview> {
    let format = ImageFormat::from_mime_type(media_type)?;
    Some(Arc::new(Image::from_bytes(format, bytes)))
}

pub struct ImageViewer {
    image: Arc<Image>,
    /// `None` if the image's size can't be read: then it's only fitted.
    zoom: Option<Zoom>,
    /// Where the pointer was, while the picture is being dragged.
    drag: Option<Point<Pixels>>,
    focus: FocusHandle,
}

impl EventEmitter<DismissEvent> for ImageViewer {}

impl Focusable for ImageViewer {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// The part of the window the picture is shown in.
fn area(viewport: Size<Pixels>) -> (f32, f32) {
    let (w, h) = (f32::from(viewport.width), f32::from(viewport.height));
    ((w - 2. * MARGIN).max(1.), (h - 2. * MARGIN - TOOLBAR).max(1.))
}

impl ImageViewer {
    pub fn new(image: Arc<Image>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // 100 % is one image pixel to one screen pixel.
        let scale = window.scale_factor();
        let zoom = kynex_core::attach::dimensions(&image.bytes)
            .map(|(w, h)| Zoom::new((w as f32 / scale, h as f32 / scale), area(window.viewport_size())));
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        Self { image, zoom, drag: None, focus }
    }

    fn update_zoom(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut Zoom)) {
        if let Some(z) = &mut self.zoom {
            f(z);
            cx.notify();
        }
    }

    /// A window position, from the centre of the picture's area.
    fn centred(&self, p: Point<Pixels>) -> (f32, f32) {
        let (aw, ah) = self.zoom.map_or((0., 0.), |z| z.area);
        (f32::from(p.x) - MARGIN - aw / 2., f32::from(p.y) - MARGIN - ah / 2.)
    }

    fn on_picture(&self, p: Point<Pixels>) -> bool {
        let Some(z) = self.zoom else { return true };
        let (x, y, w, h) = z.rect();
        let (px_, py) = (f32::from(p.x) - MARGIN, f32::from(p.y) - MARGIN);
        px_ >= x && px_ <= x + w && py >= y && py <= y + h
    }

    fn key_down(&mut self, ev: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let k = &ev.keystroke;
        if k.modifiers.control || k.modifiers.platform || k.modifiers.alt {
            return;
        }
        match k.key.as_str() {
            "escape" => cx.emit(DismissEvent),
            "=" | "+" => self.update_zoom(cx, |z| z.zoom_at(STEP, (0., 0.))),
            "-" => self.update_zoom(cx, |z| z.zoom_at(1. / STEP, (0., 0.))),
            "0" => self.update_zoom(cx, Zoom::fit),
            "1" => self.update_zoom(cx, Zoom::actual),
            _ => return,
        }
        cx.stop_propagation();
    }

    fn wheel(&mut self, ev: &ScrollEvent, _: &mut Window, cx: &mut Context<Self>) {
        let dy = f32::from(ev.delta.pixel_delta(px(40.)).y);
        let at = self.centred(ev.position);
        // Wheel up (towards the top) zooms in.
        self.update_zoom(cx, |z| z.zoom_at((dy / WHEEL_PER_DOUBLING).exp2(), at));
        cx.stop_propagation();
    }

    fn mouse_down(&mut self, ev: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.on_picture(ev.position) {
            self.drag = Some(ev.position);
        } else {
            cx.emit(DismissEvent);
        }
    }

    fn mouse_move(&mut self, ev: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(from) = self.drag else { return };
        if ev.pressed_button != Some(MouseButton::Left) {
            self.drag = None;
            return;
        }
        self.drag = Some(ev.position);
        let by = (f32::from(ev.position.x - from.x), f32::from(ev.position.y - from.y));
        self.update_zoom(cx, |z| z.pan_by(by));
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = theme(cx).clone();
        let percent = self.zoom.map_or_else(|| "Fit".to_string(), |z| format!("{}%", z.percent()));
        let zoomable = self.zoom.is_some();
        popover(cx)
            .id("viewer-toolbar")
            .flex_row()
            .items_center()
            .gap_1()
            .p_1()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .when(zoomable, |d| {
                d.child(
                    icon_button("viewer-out", Icon::Minus, cx).tooltip(Tooltip::with_meta("Zoom out", "−")).on_click(
                        cx.listener(|this, _, _, cx| this.update_zoom(cx, |z| z.zoom_at(1. / STEP, (0., 0.)))),
                    ),
                )
                .child(div().w(super::px(44.)).text_center().text_xs().text_color(t.text_muted).child(percent))
                .child(
                    icon_button("viewer-in", Icon::Plus, cx)
                        .tooltip(Tooltip::with_meta("Zoom in", "+"))
                        .on_click(cx.listener(|this, _, _, cx| this.update_zoom(cx, |z| z.zoom_at(STEP, (0., 0.))))),
                )
                .child(
                    ghost_button("viewer-fit", cx)
                        .text_xs()
                        .child("Fit")
                        .tooltip(Tooltip::with_meta("Fit to window", "0"))
                        .on_click(cx.listener(|this, _, _, cx| this.update_zoom(cx, Zoom::fit))),
                )
                .child(
                    ghost_button("viewer-actual", cx)
                        .text_xs()
                        .child("1:1")
                        .tooltip(Tooltip::with_meta("Actual size", "1"))
                        .on_click(cx.listener(|this, _, _, cx| this.update_zoom(cx, Zoom::actual))),
                )
                .child(div().w_px().h(super::px(16.)).mx_1().bg(t.border_variant))
            })
            .child(
                icon_button("viewer-close", Icon::Close, cx)
                    .tooltip(Tooltip::with_meta("Close", "Esc"))
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(DismissEvent))),
            )
    }
}

impl Render for ImageViewer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (aw, ah) = area(window.viewport_size());
        if let Some(z) = &mut self.zoom
            && z.area != (aw, ah)
        {
            z.resize((aw, ah));
        }
        let picture = img(self.image.clone()).absolute();
        let picture = match self.zoom {
            Some(z) => {
                let (x, y, w, h) = z.rect();
                picture.left(px(x)).top(px(y)).w(px(w)).h(px(h)).object_fit(ObjectFit::Fill)
            }
            None => picture.size_full().object_fit(ObjectFit::ScaleDown),
        };
        // Bigger than the window: it can be dragged.
        let draggable = self.zoom.is_some_and(|z| {
            let (_, _, w, h) = z.rect();
            w > aw || h > ah
        });
        let cursor = match () {
            _ if self.drag.is_some() && draggable => CursorStyle::ClosedHand,
            _ if draggable => CursorStyle::OpenHand,
            _ => CursorStyle::Arrow,
        };
        div()
            .id("image-viewer")
            .key_context("ImageViewer")
            .track_focus(&self.focus)
            .absolute()
            .inset_0()
            .occlude()
            .bg(gpui::black().opacity(0.8))
            .on_key_down(cx.listener(Self::key_down))
            .on_scroll(cx.listener(Self::wheel))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.drag = None;
                    cx.notify();
                }),
            )
            .child(
                div()
                    .absolute()
                    .left(px(MARGIN))
                    .top(px(MARGIN))
                    .w(px(aw))
                    .h(px(ah))
                    .overflow_hidden()
                    .cursor(cursor)
                    .child(picture),
            )
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom(px(MARGIN / 2.))
                    .flex()
                    .justify_center()
                    .child(self.render_toolbar(cx)),
            )
    }
}
