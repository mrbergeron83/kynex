//! Height changes that ease instead of jumping: a tool block or diff opening
//! in the transcript, the composer growing with its text, a timeline turn
//! folding open. Without it, everything below moves in one frame.
//!
//! The wrapper remembers its content's height. When the content comes out a
//! different height, the wrapper keeps its old one for that frame (clipped),
//! then eases to the new one over `motion::NORMAL` and lets go: settled, it's
//! exactly the content's height and clips nothing. A virtualized list lays its
//! visible rows out again every frame, so a row there eases with no extra
//! work. Reduced motion jumps, as before.

use std::time::Instant;

use gpui::{
    AnyElement, App, Bounds, ContentMask, Element, ElementId, GlobalElementId, IntoElement, LayoutId, Pixels, Style,
    Window, div, prelude::*, relative,
};

use super::motion;

/// `child`, easing to each new height it takes. `id` keeps its height across
/// frames, so it must be stable for the same content.
pub fn ease_height(id: impl Into<ElementId>, child: impl IntoElement) -> EaseHeight {
    EaseHeight { id: id.into(), child: Some(child.into_any_element()) }
}

pub struct EaseHeight {
    id: ElementId,
    child: Option<AnyElement>,
}

/// From one height to another; `since` is unset once settled.
#[derive(Clone, Copy)]
struct Ease {
    from: Pixels,
    to: Pixels,
    since: Option<Instant>,
}

impl Ease {
    fn settled(at: Pixels) -> Self {
        Self { from: at, to: at, since: None }
    }

    /// The height `now`, and whether it's still on its way.
    fn at(&self, now: Instant) -> (Pixels, bool) {
        let Some(since) = self.since else { return (self.to, false) };
        let t = now.duration_since(since).as_secs_f32() / motion::NORMAL.as_secs_f32();
        if t >= 1. {
            return (self.to, false);
        }
        (self.from + (self.to - self.from) * gpui::ease_out_quint()(t), true)
    }

    /// The next state once the content measured `natural`.
    fn measured(self, natural: Pixels, now: Instant) -> Self {
        if (self.to - natural).abs() < gpui::px(0.5) {
            return self;
        }
        Self { from: self.at(now).0, to: natural, since: Some(now) }
    }
}

pub struct Layout {
    inner: AnyElement,
    inner_id: LayoutId,
    /// The height drawn this frame, when it's held rather than the content's own.
    held: Option<Pixels>,
}

impl IntoElement for EaseHeight {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for EaseHeight {
    type RequestLayoutState = Layout;
    /// Whether to clip the content to the held height.
    type PrepaintState = bool;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Layout) {
        let now = Instant::now();
        let held = match id {
            Some(id) if !cx.reduce_motion() => window.with_element_state(id, |ease: Option<Option<Ease>>, _| {
                let ease = ease.flatten();
                (ease.map(|e| e.at(now).0), ease)
            }),
            _ => None,
        };
        let child = self.child.take().unwrap_or_else(|| div().into_any_element());
        // Held: the content lays out at its own height out of flow, and the
        // wrapper takes the held one. Otherwise the content sizes the wrapper.
        let mut inner = match held {
            Some(_) => div().absolute().top_0().left_0().right_0().child(child).into_any_element(),
            None => div().child(child).into_any_element(),
        };
        let inner_id = inner.request_layout(window, cx);
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        if let Some(h) = held {
            style.size.height = h.into();
        }
        (window.request_layout(style, [inner_id], cx), Layout { inner, inner_id, held })
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        _bounds: Bounds<Pixels>,
        layout: &mut Layout,
        window: &mut Window,
        cx: &mut App,
    ) -> bool {
        layout.inner.prepaint(window, cx);
        let natural = window.layout_bounds(layout.inner_id).size.height;
        let Some(id) = id else { return false };
        let now = Instant::now();
        let reduce = cx.reduce_motion();
        let held = layout.held;
        let (moving, clip) = window.with_element_state(id, |ease: Option<Option<Ease>>, _| {
            // First seen, or not animating: just remember the height.
            let next = match ease.flatten() {
                Some(e) if !reduce => e.measured(natural, now),
                _ => Ease::settled(natural),
            };
            let moving = next.at(now).1;
            let clip = moving || held.is_some_and(|h| (h - natural).abs() >= gpui::px(0.5));
            ((moving, clip), Some(next))
        });
        if moving {
            window.request_animation_frame();
        }
        clip
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        layout: &mut Layout,
        clip: &mut bool,
        window: &mut Window,
        cx: &mut App,
    ) {
        if *clip {
            window.with_content_mask(Some(ContentMask { bounds }), |window| layout.inner.paint(window, cx));
        } else {
            layout.inner.paint(window, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::px;

    #[test]
    fn eases_to_a_new_height_then_settles() {
        let start = Instant::now();
        let e = Ease::settled(px(100.));
        assert_eq!(e.measured(px(100.2), start).since, None, "under half a pixel isn't a change");
        let e = e.measured(px(300.), start);
        let (h, moving) = e.at(start + motion::NORMAL / 4);
        assert!(moving && h > px(100.) && h < px(300.), "{h:?}");
        assert_eq!(e.at(start + motion::NORMAL), (px(300.), false));
        // Turning back mid-way starts from where it is, not from either end.
        let mid = start + motion::NORMAL / 4;
        let back = e.measured(px(100.), mid);
        assert_eq!(back.from, e.at(mid).0);
    }
}
