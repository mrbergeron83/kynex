//! Smooth mouse-wheel scrolling for the whole window. gpui scrolls a notch at
//! a time, about 60px; here every wheel turn is caught before any element sees
//! it and played back over the next frames as the small pixel scrolls a
//! touchpad sends ([`Wheel`]). So every scroll area glides, lists and panels
//! and the terminal alike, without code of its own.
//!
//! Touchpads and touchscreens already scroll in pixels and pass straight
//! through; so does Ctrl+wheel. With motion reduced a notch still goes the
//! same distance, just at once.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

use gpui::{
    App, DispatchPhase, IntoElement, Modifiers, MouseDownEvent, PlatformInput, ScrollDelta, ScrollEvent, ScrollSource,
    Styled, TouchPhase, Window, canvas, point, px,
};
use kynex_core::motion::Wheel;

#[derive(Default)]
struct State {
    wheel: Wheel,
    last_frame: Option<Instant>,
    /// A frame callback is scheduled.
    ticking: bool,
    modifiers: Modifiers,
}

/// The window's wheel state. Draw [`SmoothWheel::listener`] once, anywhere
/// in the window.
#[derive(Clone, Default)]
pub struct SmoothWheel(Rc<RefCell<State>>);

impl SmoothWheel {
    /// An empty element that takes the wheel turns over the whole window.
    pub fn listener(&self) -> impl IntoElement + use<> {
        let state = self.0.clone();
        canvas(
            |_, _, _| {},
            move |_, _, window, _| {
                let s = state.clone();
                window.on_mouse_event(move |ev: &ScrollEvent, phase, window, cx| {
                    if phase == DispatchPhase::Capture {
                        take_turn(&s, ev, window, cx);
                    }
                });
                // Clicking (or grabbing a scrollbar) stops a glide where it is.
                let s = state.clone();
                window.on_mouse_event(move |_: &MouseDownEvent, phase, _, _| {
                    if phase == DispatchPhase::Capture {
                        s.borrow_mut().wheel.stop();
                    }
                });
            },
        )
        .absolute()
        .size_0()
    }
}

/// A wheel turn: kept from the elements under the pointer and queued up.
fn take_turn(state: &Rc<RefCell<State>>, ev: &ScrollEvent, window: &mut Window, cx: &mut App) {
    let ScrollDelta::Lines(lines) = ev.delta else { return };
    if ev.source != ScrollSource::Pointer || ev.modifiers.control || ev.modifiers.platform {
        return;
    }
    cx.stop_propagation();
    let mut s = state.borrow_mut();
    s.modifiers = ev.modifiers;
    s.wheel.push((lines.x, lines.y), !cx.reduce_motion());
    if !s.ticking && s.wheel.is_moving() {
        s.ticking = true;
        s.last_frame = None;
        let state = state.clone();
        window.on_next_frame(move |window, cx| tick(state, window, cx));
    }
}

/// One frame of scrolling: sent as a pixel scroll where the pointer is, just
/// before the frame draws, so it shows on that frame.
fn tick(state: Rc<RefCell<State>>, window: &mut Window, cx: &mut App) {
    let now = Instant::now();
    let (step, modifiers, moving) = {
        let mut s = state.borrow_mut();
        // The first frame after a turn: assume one 120 Hz frame went by.
        let dt = s.last_frame.map_or(1. / 120., |t| now.duration_since(t).as_secs_f32());
        s.last_frame = Some(now);
        let step = s.wheel.step(dt);
        s.ticking = s.wheel.is_moving();
        (step, s.modifiers, s.ticking)
    };
    if step != (0., 0.) {
        let ev = ScrollEvent {
            position: window.mouse_position(),
            delta: ScrollDelta::Pixels(point(px(step.0), px(step.1))),
            modifiers,
            touch_phase: TouchPhase::Moved,
            source: ScrollSource::Pointer,
        };
        window.dispatch_event(PlatformInput::Scroll(ev), cx);
    }
    if moving {
        window.on_next_frame(move |window, cx| tick(state, window, cx));
    }
}
