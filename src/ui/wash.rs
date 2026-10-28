//! Hover and selection backgrounds that fade, as in Zed. gpui's `.hover()`
//! flips a style in one frame and has no transition, so the shared widgets
//! draw their wash with this instead: an element laid over the widget's
//! padding box, under its children, that tracks the pointer itself and blends
//! between colours over `motion::QUICK`.
//!
//! It asks for frames only while a fade runs; a settled wash, hovered or not,
//! costs nothing. Reduced motion flips at once, as `.hover()` did.

use std::time::Instant;

use gpui::{
    App, Bounds, DispatchPhase, Element, ElementId, GlobalElementId, Hitbox, HitboxBehavior, Hsla, IntoElement,
    LayoutId, MouseMoveEvent, Pixels, Position, Rgba, Style, Window, fill, px, rems,
};

use super::motion;

/// The wash for a widget: `hover` under the pointer, else `on` (a selected
/// row, an open menu's button), else nothing. Put it first among the
/// widget's children, and make the widget `.relative()`.
pub fn wash(hover: Hsla, on: Option<Hsla>) -> Wash {
    Wash { hover, on }
}

/// A list row's wash: `active` while selected, which hovering doesn't change,
/// else `hover` under the pointer.
pub fn row_wash(selected: bool, hover: Hsla, active: Hsla) -> Wash {
    if selected { wash(active, Some(active)) } else { wash(hover, None) }
}

pub struct Wash {
    hover: Hsla,
    on: Option<Hsla>,
}

/// A fade from one colour to another, kept across frames.
struct Fade {
    from: Hsla,
    to: Hsla,
    since: Instant,
}

impl Fade {
    /// The colour `now`, and whether it's still on its way.
    fn at(&self, now: Instant) -> (Hsla, bool) {
        let t = now.duration_since(self.since).as_secs_f32() / motion::QUICK.as_secs_f32();
        if t >= 1. {
            return (self.to, false);
        }
        (mix(self.from, self.to, gpui::ease_out_quint()(t)), true)
    }
}

/// `a` to `b` by `t`, through RGB so hues don't swing on the way.
fn mix(a: Hsla, b: Hsla, t: f32) -> Hsla {
    let (a, b) = (Rgba::from(a), Rgba::from(b));
    let l = |x: f32, y: f32| x + (y - x) * t;
    Rgba { r: l(a.r, b.r), g: l(a.g, b.g), b: l(a.b, b.b), a: l(a.a, b.a) }.into()
}

impl IntoElement for Wash {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for Wash {
    type RequestLayoutState = ();
    type PrepaintState = Hitbox;

    fn id(&self) -> Option<ElementId> {
        Some("wash".into())
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
        let zero = px(0.).into();
        let inset = gpui::Edges { top: zero, right: zero, bottom: zero, left: zero };
        let style = Style { position: Position::Absolute, inset, ..Default::default() };
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        _cx: &mut App,
    ) -> Hitbox {
        // Doesn't block: the widget's own children and handlers still get the mouse.
        window.insert_hitbox(bounds, HitboxBehavior::Normal)
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        hitbox: &mut Hitbox,
        window: &mut Window,
        cx: &mut App,
    ) {
        let hovered = hitbox.is_hovered(window);
        let target = if hovered { Some(self.hover) } else { self.on };
        let reduce = cx.reduce_motion();
        let now = Instant::now();
        let (colour, moving) = window.with_element_state(id.expect("wash has an id"), |fade: Option<Fade>, _| {
            // Nothing becomes the last colour gone clear, so it fades rather than greys.
            let mut fade = fade.unwrap_or_else(|| {
                let first = target.unwrap_or_default();
                Fade { from: first, to: first, since: now - motion::QUICK }
            });
            let target = target.unwrap_or(fade.to.opacity(0.));
            if target != fade.to {
                fade = Fade { from: fade.at(now).0, to: target, since: now };
            }
            let shown = if reduce { (fade.to, false) } else { fade.at(now) };
            (shown, fade)
        });
        if moving {
            window.request_animation_frame();
        }
        if colour.a > 0. {
            let radius = rems(0.375).to_pixels(window.rem_size());
            window.paint_quad(fill(bounds, colour).corner_radii(radius));
        }
        // The pointer coming or going repaints, so the fade starts.
        let view = window.current_view();
        let hitbox = hitbox.clone();
        window.on_mouse_event(move |_: &MouseMoveEvent, phase, window, cx| {
            if phase == DispatchPhase::Capture && hitbox.is_hovered(window) != hovered {
                cx.notify(view);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fades_through_rgb_and_lands() {
        let (a, b) = (gpui::hsla(0., 0., 0., 0.), gpui::hsla(0., 0., 1., 1.));
        let mid = Rgba::from(mix(a, b, 0.5));
        assert!((mid.r - 0.5).abs() < 1e-3 && (mid.a - 0.5).abs() < 1e-3);
        let start = Instant::now();
        let fade = Fade { from: a, to: b, since: start };
        assert!(fade.at(start).1);
        assert_eq!(fade.at(start + motion::QUICK), (b, false));
    }
}
