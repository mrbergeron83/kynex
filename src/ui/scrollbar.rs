//! Overlay scrollbars: a thin thumb drawn over the content (it never takes
//! width from it) that shows while scrolling or with the pointer over its
//! track, and fades out after. Dragging the thumb scrolls; clicking the track
//! pages. Draw [`scrollbar`] as the last child of a `relative()` box that
//! holds the scroll area; it fills the box and only takes the pointer over
//! its track.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    App, Axis, Bounds, CursorStyle, DispatchPhase, Div, Element, ElementId, GlobalElementId, Hitbox, HitboxBehavior,
    InspectorElementId, IntoElement, LayoutId, ListState, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ParentElement, Pixels, Point, ScrollHandle, Style, Styled, UniformListScrollHandle, Window, div, fill, point, px,
    relative,
};

use crate::theme::theme;

/// The thumb's width, its inset from the edge, and the strip that takes the pointer.
const THUMB: f32 = 4.;
const INSET: f32 = 2.;
const TRACK: f32 = 8.;
/// The shortest a thumb gets, so it stays easy to grab.
const MIN_THUMB: f32 = 24.;
/// How long it stays after the last scroll, and how long it takes to fade.
const HOLD: Duration = Duration::from_millis(800);
const FADE: Duration = Duration::from_millis(150);

/// Something that scrolls: how far it is along an axis, and how to move it.
pub trait ScrollSource: 'static {
    /// The distance scrolled (0 at the start), the most it can be, and the viewport's length.
    fn metrics(&self, axis: Axis) -> (f32, f32, f32);
    fn scroll_to(&self, axis: Axis, offset: f32);
    /// A thumb drag starts (`true`) or ends.
    fn dragging(&self, _: bool) {}
}

fn along(p: Point<Pixels>, axis: Axis) -> f32 {
    f32::from(match axis {
        Axis::Vertical => p.y,
        Axis::Horizontal => p.x,
    })
}

impl ScrollSource for ScrollHandle {
    fn metrics(&self, axis: Axis) -> (f32, f32, f32) {
        let size = self.bounds().size;
        let viewport = f32::from(if axis == Axis::Vertical { size.height } else { size.width });
        (-along(self.offset(), axis), along(self.max_offset(), axis), viewport)
    }

    fn scroll_to(&self, axis: Axis, offset: f32) {
        let mut o = self.offset();
        match axis {
            Axis::Vertical => o.y = px(-offset),
            Axis::Horizontal => o.x = px(-offset),
        }
        self.set_offset(o);
    }
}

impl ScrollSource for UniformListScrollHandle {
    fn metrics(&self, axis: Axis) -> (f32, f32, f32) {
        self.0.borrow().base_handle.metrics(axis)
    }

    fn scroll_to(&self, axis: Axis, offset: f32) {
        let handle = self.0.borrow().base_handle.clone();
        handle.scroll_to(axis, offset);
    }
}

/// A `gpui::list`; it only scrolls up and down.
impl ScrollSource for ListState {
    fn metrics(&self, axis: Axis) -> (f32, f32, f32) {
        if axis == Axis::Horizontal {
            return (0., 0., 0.);
        }
        let viewport = f32::from(self.viewport_bounds().size.height);
        (-f32::from(self.scroll_px_offset_for_scrollbar().y), f32::from(self.max_offset_for_scrollbar().y), viewport)
    }

    fn scroll_to(&self, _: Axis, offset: f32) {
        self.set_offset_from_scrollbar(point(px(0.), px(-offset)));
    }

    fn dragging(&self, on: bool) {
        if on {
            self.scrollbar_drag_started();
        } else {
            self.scrollbar_drag_ended();
        }
    }
}

thread_local! {
    static HANDLES: RefCell<HashMap<ElementId, ScrollHandle>> = RefCell::default();
}

/// A scroll handle kept across frames under `key`, for scroll areas drawn
/// without a struct to hold one (menus, modals, rows).
pub fn handle(key: impl Into<ElementId>) -> ScrollHandle {
    let key = key.into();
    HANDLES.with_borrow_mut(|all| {
        if all.len() >= 1024 && !all.contains_key(&key) {
            all.clear();
        }
        all.entry(key).or_default().clone()
    })
}

/// `area`, a scroll area tracking `handle`, with a vertical scrollbar over it.
/// The wrapper takes `area`'s place: size it with `wrap`.
pub fn vertical_over(
    id: impl Into<ElementId>,
    handle: ScrollHandle,
    area: impl IntoElement,
    wrap: impl FnOnce(Div) -> Div,
) -> Div {
    wrap(div().relative().flex().flex_col()).child(area).child(vertical(id, handle))
}

/// A scrollbar for `source` along `axis` (vertical on the right, horizontal at the bottom).
pub fn scrollbar(id: impl Into<ElementId>, source: impl ScrollSource, axis: Axis) -> Scrollbar {
    Scrollbar { id: id.into(), source: Rc::new(source), axis }
}

/// A vertical [`scrollbar`].
pub fn vertical(id: impl Into<ElementId>, source: impl ScrollSource) -> Scrollbar {
    scrollbar(id, source, Axis::Vertical)
}

pub struct Scrollbar {
    id: ElementId,
    source: Rc<dyn ScrollSource>,
    axis: Axis,
}

#[derive(Default)]
struct State {
    /// The offset last drawn, to notice scrolling.
    offset: f32,
    moved: Option<Instant>,
    hovered: bool,
    /// While the thumb is held: where along it it was grabbed.
    grab: Option<f32>,
}

/// Where the track and thumb sit in `bounds`.
struct Geometry {
    track: Bounds<Pixels>,
    /// Along the axis: the track's start and length, the thumb's start and length.
    start: f32,
    length: f32,
    thumb_at: f32,
    thumb: f32,
}

impl Geometry {
    fn new(bounds: Bounds<Pixels>, axis: Axis, (offset, max, viewport): (f32, f32, f32)) -> Self {
        let track = match axis {
            Axis::Vertical => {
                Bounds::new(point(bounds.right() - px(TRACK), bounds.top()), gpui::size(px(TRACK), bounds.size.height))
            }
            Axis::Horizontal => {
                Bounds::new(point(bounds.left(), bounds.bottom() - px(TRACK)), gpui::size(bounds.size.width, px(TRACK)))
            }
        };
        let start = along(track.origin, axis);
        let length = along(point(track.size.width, track.size.height), axis);
        let content = viewport + max;
        let thumb = if content > 0. { (viewport / content * length).max(MIN_THUMB).min(length) } else { length };
        let thumb_at = start + if max > 0. { (offset / max).clamp(0., 1.) * (length - thumb) } else { 0. };
        Self { track, start, length, thumb_at, thumb }
    }

    /// The offset that puts the thumb's start at `at`.
    fn offset_for(&self, at: f32, max: f32) -> f32 {
        let room = self.length - self.thumb;
        if room <= 0. { 0. } else { ((at - self.start) / room).clamp(0., 1.) * max }
    }
}

impl IntoElement for Scrollbar {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for Scrollbar {
    type RequestLayoutState = ();
    type PrepaintState = Option<Hitbox>;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style { position: gpui::Position::Absolute, ..Default::default() };
        style.inset.top = px(0.).into();
        style.inset.left = px(0.).into();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, None, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        _: &mut App,
    ) -> Option<Hitbox> {
        let (_, max, _) = self.source.metrics(self.axis);
        (max > 0.5).then(|| {
            let g = Geometry::new(bounds, self.axis, self.source.metrics(self.axis));
            window.insert_hitbox(g.track, HitboxBehavior::BlockMouse)
        })
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        hitbox: &mut Option<Hitbox>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(id) = id else { return };
        let Some(hitbox) = hitbox.clone() else { return };
        let state = window.with_element_state(id, |s: Option<Rc<RefCell<State>>>, _| {
            let s = s.unwrap_or_default();
            (s.clone(), s)
        });
        let (axis, source) = (self.axis, self.source.clone());
        let metrics = source.metrics(axis);
        let g = Geometry::new(bounds, axis, metrics);
        let now = Instant::now();

        // How visible it is: fully while held, hovered or just scrolled, then fading.
        let alpha = {
            let mut s = state.borrow_mut();
            if (metrics.0 - s.offset).abs() > 0.5 {
                s.offset = metrics.0;
                s.moved = Some(now);
            }
            s.hovered = hitbox.is_hovered(window);
            let since = s.moved.map(|t| now.duration_since(t));
            if s.grab.is_some() || s.hovered {
                1.
            } else {
                match since {
                    Some(d) if d < HOLD => {
                        window.request_animation_frame();
                        0.5
                    }
                    Some(d) if d < HOLD + FADE && !cx.reduce_motion() => {
                        window.request_animation_frame();
                        0.5 * (1. - (d - HOLD).as_secs_f32() / FADE.as_secs_f32())
                    }
                    _ => 0.,
                }
            }
        };
        if alpha > 0. {
            let side = (TRACK - THUMB) - INSET;
            let thumb = match axis {
                Axis::Vertical => {
                    Bounds::new(point(g.track.left() + px(side), px(g.thumb_at)), gpui::size(px(THUMB), px(g.thumb)))
                }
                Axis::Horizontal => {
                    Bounds::new(point(px(g.thumb_at), g.track.top() + px(side)), gpui::size(px(g.thumb), px(THUMB)))
                }
            };
            let color = theme(cx).text_placeholder.opacity(alpha);
            window.paint_quad(fill(thumb, color).corner_radii(px(THUMB / 2.)));
        }
        window.set_cursor_style(CursorStyle::Arrow, &hitbox);

        window.on_mouse_event({
            let (state, source, hitbox) = (state.clone(), source.clone(), hitbox.clone());
            move |e: &MouseDownEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble || e.button != MouseButton::Left || !hitbox.is_hovered(window) {
                    return;
                }
                cx.stop_propagation();
                let (offset, max, viewport) = source.metrics(axis);
                let g = Geometry::new(bounds, axis, (offset, max, viewport));
                let at = along(e.position, axis);
                if at >= g.thumb_at && at <= g.thumb_at + g.thumb {
                    state.borrow_mut().grab = Some(at - g.thumb_at);
                    source.dragging(true);
                } else {
                    // A page toward the click.
                    let page = if at < g.thumb_at { -viewport } else { viewport };
                    source.scroll_to(axis, (offset + page).clamp(0., max));
                }
                window.refresh();
            }
        });
        window.on_mouse_event({
            let (state, source, hitbox) = (state.clone(), source.clone(), hitbox.clone());
            move |e: &MouseMoveEvent, phase, window, _| {
                if phase != DispatchPhase::Bubble {
                    return;
                }
                let grab = state.borrow().grab;
                match grab {
                    Some(grab) if e.dragging() => {
                        let (offset, max, viewport) = source.metrics(axis);
                        let g = Geometry::new(bounds, axis, (offset, max, viewport));
                        source.scroll_to(axis, g.offset_for(along(e.position, axis) - grab, max));
                        window.refresh();
                    }
                    _ if state.borrow().hovered != hitbox.is_hovered(window) => window.refresh(),
                    _ => {}
                }
            }
        });
        window.on_mouse_event(move |e: &MouseUpEvent, phase, window, _| {
            if phase == DispatchPhase::Bubble
                && e.button == MouseButton::Left
                && state.borrow_mut().grab.take().is_some()
            {
                source.dragging(false);
                window.refresh();
            }
        });
    }
}
