//! Short, time-based transitions, Zed style: things fade and settle into place
//! instead of popping in. Built on gpui's `with_animation`, which measures real
//! time (the same at 60 and 240 Hz), asks for frames only while running, and
//! shows the end state at once when `cx.reduce_motion()` is set (the
//! Animations setting, or the OS's "reduce motion").
//!
//! An animation plays when its element id first appears: give a view's id
//! something that changes (a session's uid) to play it again on a switch.

use std::time::Duration;

use gpui::{Animation, AnimationElement, AnimationExt, App, AppContext, ElementId, IntoElement, Pixels, Styled, px};
use kynex_core::motion::{Motion, os_reduces_motion};

/// Hovers, tooltips, small popups.
pub const QUICK: Duration = Duration::from_millis(110);
/// Menus, modals, panels, view switches.
pub const NORMAL: Duration = Duration::from_millis(170);

fn ease_out() -> Animation {
    Animation::new(NORMAL).with_easing(gpui::ease_out_quint())
}

/// Fades in over `duration`.
pub fn fade_in<E: IntoElement + Styled + 'static>(
    id: impl Into<ElementId>,
    el: E,
    duration: Duration,
) -> AnimationElement<E> {
    el.with_animation(id, Animation::new(duration).with_easing(gpui::ease_out_quint()), |el, t| el.opacity(t))
}

/// Fades in while settling `rise` up into place: menus, modals, toasts.
pub fn pop_in<E: IntoElement + Styled + 'static>(id: impl Into<ElementId>, el: E, rise: Pixels) -> AnimationElement<E> {
    el.with_animation(id, ease_out(), move |el, t| el.opacity(t).mt(rise * (1. - t)))
}

/// Grows from nothing to `width`: a side panel opening.
pub fn grow_width<E: IntoElement + Styled + 'static>(
    id: impl Into<ElementId>,
    el: E,
    width: f32,
) -> AnimationElement<E> {
    el.with_animation(id, ease_out(), move |el, t| el.w(px(width * t)).opacity(t))
}

/// Applies the Animations setting: at once from what's known, then again once
/// the OS setting has been read in the background.
pub fn apply(motion: Motion, cx: &mut App) {
    cx.set_reduce_motion(motion == Motion::Off);
    if motion == Motion::System {
        cx.spawn(async move |cx| {
            let os = cx.background_spawn(async { os_reduces_motion() }).await;
            cx.update(|cx| cx.set_reduce_motion(motion.reduce(os)));
        })
        .detach();
    }
}
