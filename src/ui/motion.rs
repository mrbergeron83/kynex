//! Short, time-based transitions, Zed style: things fade and settle into place
//! instead of popping in. Built on gpui's `with_animation`, which measures real
//! time (the same at 60 and 240 Hz), asks for frames only while running, and
//! shows the end state at once when `cx.reduce_motion()` is set (the
//! Animations setting, or the OS's "reduce motion").
//!
//! An animation plays when its element id first appears: give a view's id
//! something that changes (a session's uid) to play it again on a switch.
//!
//! Closing is the other way round: gpui drops an element the frame it's no
//! longer drawn, so a [`Leaving`] keeps what was open for a moment after it
//! closes, the view draws it once more per frame with how far it has gone,
//! and [`inert`] keeps that leftover from taking clicks meant for what's under it.

use std::time::{Duration, Instant};

use gpui::{
    Animation, AnimationElement, AnimationExt, AnyElement, App, AppContext, Bounds, ContentMask, Element, ElementId,
    GlobalElementId, IntoElement, LayoutId, Pixels, Styled, Window, px,
};
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

/// How far an exit has gone, eased: 0 still open, 1 gone.
fn out(t: f32) -> f32 {
    gpui::ease_out_quint()(t.clamp(0., 1.))
}

/// [`fade_in`] while opening; fading out by `leaving` (see [`Leaving`]) while it closes,\n/// out of the mouse's reach (see [`inert`]), as are [`pop`] and [`grow`].
pub fn fade<E: IntoElement + Styled + 'static>(
    id: impl Into<ElementId>,
    el: E,
    duration: Duration,
    leaving: Option<f32>,
) -> AnyElement {
    match leaving {
        Some(t) => inert(el.opacity(1. - out(t))).into_any_element(),
        None => fade_in(id, el, duration).into_any_element(),
    }
}

/// [`pop_in`] while opening; fading and settling `rise` back down while it closes.
pub fn pop<E: IntoElement + Styled + 'static>(
    id: impl Into<ElementId>,
    el: E,
    rise: Pixels,
    leaving: Option<f32>,
) -> AnyElement {
    match leaving {
        Some(t) => inert(el.opacity(1. - out(t)).mt(rise * out(t))).into_any_element(),
        None => pop_in(id, el, rise).into_any_element(),
    }
}

/// [`grow_width`] while opening; shrinking back to nothing while it closes.
pub fn grow<E: IntoElement + Styled + 'static>(
    id: impl Into<ElementId>,
    el: E,
    width: f32,
    leaving: Option<f32>,
) -> AnyElement {
    match leaving {
        Some(t) => inert(el.w(px(width * (1. - out(t)))).opacity(1. - out(t))).into_any_element(),
        None => grow_width(id, el, width).into_any_element(),
    }
}

/// Something that animates out after it closes: it remembers what was open,
/// and for `duration` after it's gone hands that back to be drawn leaving.
pub struct Leaving<T> {
    last: Option<T>,
    closed: Option<Instant>,
    duration: Duration,
}

impl<T: Clone> Leaving<T> {
    pub const fn new(duration: Duration) -> Self {
        Self { last: None, closed: None, duration }
    }

    /// Call once a frame with what's open now. `None` while it's open (draw
    /// it as usual) or once it's gone; while it's leaving, what was open and
    /// how far it has gone (0 → 1), asking for the frames that takes.
    /// Reduced motion lets it go at once.
    pub fn track(&mut self, open: Option<&T>, window: &mut Window, cx: &App) -> Option<(T, f32)> {
        if let Some(open) = open {
            self.last = Some(open.clone());
            self.closed = None;
            return None;
        }
        if cx.reduce_motion() {
            self.last = None;
        }
        let last = self.last.clone()?;
        let closed = *self.closed.get_or_insert_with(Instant::now);
        let t = closed.elapsed().as_secs_f32() / self.duration.as_secs_f32();
        if t >= 1. {
            self.last = None;
            self.closed = None;
            return None;
        }
        window.request_animation_frame();
        Some((last, t))
    }

    /// Like [`Self::track`], for something open while `open` is true.
    pub fn track_flag(&mut self, open: bool, window: &mut Window, cx: &App) -> Option<f32>
    where
        T: Default,
    {
        let now = open.then(T::default);
        self.track(now.as_ref(), window, cx).map(|(_, t)| t)
    }
}

/// Draws `el` as it is, but out of reach of the mouse: an element on its way
/// out shouldn't catch the click meant for what's under it. Its hitboxes are
/// registered under an empty content mask, so nothing ever hits them.
pub fn inert(el: impl IntoElement) -> Inert {
    Inert(el.into_any_element())
}

pub struct Inert(AnyElement);

impl IntoElement for Inert {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for Inert {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        // Takes the child's place in the layout rather than adding a box of its own.
        (self.0.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let nowhere = ContentMask { bounds: Bounds::default() };
        window.with_content_mask(Some(nowhere), |window| self.0.prepaint(window, cx));
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.0.paint(window, cx);
    }
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
