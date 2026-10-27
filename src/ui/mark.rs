//! The Kynex mark, live: a housing, the petrol knob (the operator's control) and an
//! indicator lamp showing the shown session's state. While Claude works the lamp
//! pulses and the pointer sweeps. Geometry from docs/design/branding.md, on a
//! 100-unit canvas.

use std::time::Duration;

use gpui::{Animation, AnimationExt, AnyElement, App, ElementId, Pixels, Transformation, div, prelude::*, px, radians};

use super::{Icon, Lamp, icon, pulse};
use crate::theme::theme;

pub fn mark(id: impl Into<ElementId>, size: Pixels, lamp: Lamp, cx: &App) -> AnyElement {
    let id = id.into();
    let t = theme(cx);
    let u = |v: f32| size * (v / 100.);
    let knob = u(62.);

    // The knob box is centred on (50, 54); the pointer svg fills it and rotates
    // about its centre. Below ~20 px the rim and pointer are too fine to read.
    let detailed = size >= px(20.);
    let pointer = icon(Icon::MarkPointer, t.elevated).size(knob).absolute().left(u(19.)).top(u(23.));
    let pointer = if !detailed {
        None
    } else if lamp == Lamp::Working {
        let sweep = Animation::new(Duration::from_millis(4800)).repeat().with_easing(gpui::bounce(gpui::ease_in_out));
        Some(
            pointer
                .with_animation((id.clone(), "pointer"), sweep, |p, d| {
                    p.with_transformation(Transformation::rotate(radians((d * 80. - 40.).to_radians())))
                })
                .into_any_element(),
        )
    } else {
        Some(pointer.into_any_element())
    };

    let lamp_el = lamp.color(cx).map(|c| {
        let dot = div().absolute().left(u(15.)).top(u(14.)).size(u(12.)).rounded_full().bg(c);
        if lamp == Lamp::Working {
            dot.with_animation((id.clone(), "lamp"), pulse(), |d, a| d.opacity(a)).into_any_element()
        } else {
            dot.into_any_element()
        }
    });

    div()
        .id(id)
        .relative()
        .flex_none()
        .size(size)
        .rounded(u(20.))
        .bg(t.title_bar)
        .when(detailed, |d| d.border_1().border_color(t.border))
        .child(div().absolute().left(u(19.)).top(u(23.)).size(knob).rounded_full().bg(t.accent))
        .children(pointer)
        .children(lamp_el)
        .into_any_element()
}
