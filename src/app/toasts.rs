//! The toast stack: one corner for every note (a background task ending in
//! another session, a new version, an error), the window's corner picked in
//! Settings › Appearance, clear of the bars and the composer. The rules (how
//! long each stays, pausing, merging, how many show, where) are
//! `kynex_core::toasts`; here they draw, tick, and do what their button says.

use std::time::{Duration, Instant};

use crate::ui::px;
use gpui::{AnyElement, Context, Window, canvas, div, prelude::*};
use kynex_core::toasts::{Action, Kind, Toast};

use super::KynexApp;
use super::footer::FOOTER_HEIGHT;
use crate::chrome::TITLE_BAR_HEIGHT;
use crate::theme::{Palette, theme};
use crate::ui::motion::{self, pop};
use crate::ui::{Icon, Tooltip, ghost_button, icon, icon_button};

/// How often the stack's time moves: smooth enough for a toast fading out.
const TICK: Duration = Duration::from_millis(16);

impl KynexApp {
    /// Puts a toast up (or merges it into a like one).
    pub(super) fn toast(&mut self, toast: Toast) -> &mut Toast {
        self.toasts.push(toast)
    }

    /// Before drawing: the shown session's own toasts go, and the stack's
    /// clock runs while there's anything in it, paused while the pointer is on
    /// it or the window is in the background.
    pub(super) fn sync_toasts(&mut self, window: &Window, cx: &mut Context<Self>) {
        if let Some(key) = self.active_key() {
            self.toasts.seen(key);
        }
        self.toasts_paused = self.toast_hover || !window.is_window_active();
        if self.toasts.is_empty() || self.toasts_ticking {
            return;
        }
        self.toasts_ticking = true;
        cx.spawn(async move |this, cx| {
            let mut last = Instant::now();
            loop {
                cx.background_executor().timer(TICK).await;
                let now = Instant::now();
                let dt = now - last;
                last = now;
                let going = this.update(cx, |this, cx| {
                    let exit = if cx.reduce_motion() { Duration::ZERO } else { motion::QUICK };
                    if this.toasts.tick(dt, this.toasts_paused, exit) {
                        cx.notify();
                    }
                    this.toasts_ticking = !this.toasts.is_empty();
                    this.toasts_ticking
                });
                if !matches!(going, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    /// Does what a toast's button says, and closes it.
    fn toast_act(&mut self, id: u64, action: Action, window: &mut Window, cx: &mut Context<Self>) {
        self.toasts.dismiss(id);
        match action {
            Action::GoToSession(key) => self.go_to_session(key, window, cx),
            Action::ViewUpdate => self.open_about(cx),
        }
        cx.notify();
    }

    /// Measures the conversation's free area, laid over it, for the toasts to
    /// stay clear of its header and composer; repaints if it moved.
    pub(super) fn toast_room_probe(&self) -> impl IntoElement + use<> {
        let room = self.toast_room.clone();
        canvas(
            move |bounds, window, _| {
                if room.get() != Some(bounds) {
                    room.set(Some(bounds));
                    window.refresh();
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0()
    }

    /// The stack in its corner of the window, newest nearest the edge; `None`
    /// with nothing to show. Only the toasts take the mouse, not the space
    /// around them.
    pub(super) fn render_toasts(&mut self, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.toasts.is_empty() {
            return None;
        }
        let t = theme(cx).clone();
        let exit = if cx.reduce_motion() { Duration::ZERO } else { motion::QUICK };
        let corner = self.saved.toast_corner;
        // Clear of the title bar and footer, and in the conversation of its
        // header above and the composer below.
        let height = f32::from(window.viewport_size().height);
        let rem = window.rem_size();
        let footer = if self.saved.footer { f32::from(FOOTER_HEIGHT.to_pixels(rem)) } else { 0. };
        let (mut top, mut bottom) = (f32::from(TITLE_BAR_HEIGHT.to_pixels(rem)), footer);
        if let Some(room) = self.toast_room.get() {
            top = top.max(f32::from(room.top()));
            bottom = bottom.max(height - f32::from(room.bottom()));
        }
        let place = corner.place(top, bottom);
        let (shown, more) = self.toasts.visible();
        let mut rows: Vec<AnyElement> = shown.iter().map(|toast| toast_row(toast, exit, &t, cx)).collect();
        if corner.is_top() {
            rows.reverse();
        }
        let more = (more > 0).then(|| {
            ghost_button("toasts-more", cx)
                .h(px(22.))
                .px_2()
                .text_xs()
                .bg(t.elevated)
                .border_1()
                .border_color(t.border)
                .text_color(t.text_muted)
                .child(format!("+{more} more"))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.toasts.expand();
                    cx.notify();
                }))
        });
        let stack = div()
            .id("toasts")
            .flex()
            .flex_col()
            .map(|d| if corner.is_left() { d.items_start() } else { d.items_end() })
            .gap(px(6.))
            .max_w(px(460.))
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                this.toast_hover = *hovered;
                this.toasts_paused = *hovered;
                cx.notify();
            }))
            .map(|d| if corner.is_top() { d.children(rows).children(more) } else { d.children(more).children(rows) });
        Some(
            div()
                .absolute()
                .when_some(place.top, |d, v| d.top(gpui::px(v)))
                .when_some(place.bottom, |d, v| d.bottom(gpui::px(v)))
                .when_some(place.left, |d, v| d.left(gpui::px(v)))
                .when_some(place.right, |d, v| d.right(gpui::px(v)))
                .child(stack)
                .into_any_element(),
        )
    }
}

/// One toast: one line of icon, title, muted detail, its button, and a × that
/// shows on hover. The whole line does the button's action too.
fn toast_row(toast: &Toast, exit: Duration, t: &Palette, cx: &mut Context<KynexApp>) -> AnyElement {
    let id = toast.id;
    let (which, colour) = match toast.kind {
        Kind::Info => (Icon::Dot, t.info),
        Kind::Success => (Icon::Check, t.success),
        Kind::Warning => (Icon::Dot, t.warning),
        Kind::Error => (Icon::Close, t.error),
    };
    let has_detail = !toast.detail.is_empty();
    let full = if has_detail { format!("{}\n{}", toast.title, toast.detail) } else { toast.title.clone() };
    let title = div().truncate().child(toast.title.clone());
    let title = if has_detail { title.flex_none().max_w(px(280.)) } else { title.flex_1().min_w_0() };
    let action = toast.action;
    let row = div()
        .id(("toast", id as usize))
        .group("toast")
        // Clicks on a toast stay on it, not the transcript or composer under it.
        .occlude()
        .h(px(32.))
        .max_w(px(460.))
        .flex()
        .items_center()
        .gap_2()
        .pl_3()
        .pr_1()
        .rounded_lg()
        .border_1()
        .border_color(t.border)
        .bg(t.elevated)
        .shadow_lg()
        .text_sm()
        .child(icon(which, colour).size_3p5().flex_none())
        .child(title)
        .when(has_detail, |d| {
            d.child(div().flex_1().min_w_0().truncate().text_color(t.text_muted).child(toast.detail.clone()))
        })
        .children(action.map(|a| {
            ghost_button(("toast-action", id as usize), cx).flex_none().text_color(t.accent).child(a.label()).on_click(
                cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.toast_act(id, a, window, cx);
                }),
            )
        }))
        .child(
            icon_button(("toast-close", id as usize), Icon::Close, cx)
                .flex_none()
                .invisible()
                .group_hover("toast", |s| s.visible())
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.toasts.dismiss(id);
                    cx.notify();
                })),
        )
        .tooltip(Tooltip::text(full))
        .when_some(action, |d, a| {
            d.cursor_pointer().on_click(cx.listener(move |this, _, window, cx| this.toast_act(id, a, window, cx)))
        });
    pop(("toast-in", id as usize), row, gpui::px(6.), toast.leaving(exit))
}
