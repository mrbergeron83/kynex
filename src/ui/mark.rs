//! The Kynex mark, live: a housing, the petrol knob (the operator's control) and an
//! indicator lamp showing the shown session's state. The knob is lit from the top
//! left. While Claude works the lamp pulses, the housing glows with it, rings ripple
//! out from the knob and the pointer sweeps. Geometry from docs/design/branding.md,
//! on a 100-unit canvas.

use std::time::Duration;

use gpui::{
    Animation, AnimationExt, AnyElement, App, BoxShadow, ElementId, Hsla, Pixels, Transformation, div,
    linear_color_stop, linear_gradient, point, prelude::*, px, radians,
};

use super::{Icon, Lamp, icon, pulse};
use crate::theme::theme;

/// A soft halo of `color` around an element, `blur` wide.
fn glow(color: Hsla, blur: Pixels) -> Vec<BoxShadow> {
    vec![BoxShadow { color, offset: point(px(0.), px(0.)), blur_radius: blur, spread_radius: px(0.), inset: false }]
}

pub fn mark(id: impl Into<ElementId>, size: Pixels, lamp: Lamp, cx: &App) -> AnyElement {
    let id = id.into();
    let t = theme(cx);
    let u = move |v: f32| size * (v / 100.);
    let knob = u(62.);
    let working = lamp == Lamp::Working;

    // The knob box is centred on (50, 54); the pointer svg fills it and rotates
    // about its centre. Below ~20 px the rim, pointer and ripples are too fine to read.
    let detailed = size >= px(20.);
    let pointer = icon(Icon::MarkPointer, t.elevated).size(knob).absolute().left(u(19.)).top(u(23.));
    let pointer = if !detailed {
        None
    } else if working {
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

    // A spotlight on the knob: lighter where the light hits, the accent itself below.
    let lit = Hsla { l: (t.accent.l + 0.08).min(0.95), ..t.accent };
    let knob_el = div().absolute().left(u(19.)).top(u(23.)).size(knob).rounded_full().bg(linear_gradient(
        135.,
        linear_color_stop(lit, 0.),
        linear_color_stop(t.accent, 0.7),
    ));

    // While working, a ring grows out of the knob and fades, stopping short of the lamp.
    let ripple = (detailed && working).then(|| {
        let ring = Animation::new(Duration::from_millis(1600)).repeat().with_easing(gpui::ease_out_quint());
        div()
            .absolute()
            .rounded_full()
            .border_1()
            .border_color(t.accent)
            .with_animation((id.clone(), "ripple"), ring, move |d, a| {
                let s = 62. + a * 14.;
                d.left(u(50. - s / 2.)).top(u(54. - s / 2.)).size(u(s)).opacity((1. - a) * 0.6)
            })
            .into_any_element()
    });

    let lamp_el = lamp.color(cx).map(|c| {
        let dot = div().absolute().left(u(15.)).top(u(14.)).size(u(12.)).rounded_full().bg(c);
        if working {
            dot.with_animation((id.clone(), "lamp"), pulse(), move |d, a| {
                d.opacity(a).shadow(glow(c.opacity(a), u(14.)))
            })
            .into_any_element()
        } else {
            dot.shadow(glow(c.opacity(0.6), u(14.))).into_any_element()
        }
    });

    let housing = div()
        .id(id.clone())
        .relative()
        .flex_none()
        .size(size)
        .rounded(u(20.))
        .bg(t.title_bar)
        .when(detailed, |d| d.border_1().border_color(t.border))
        .child(knob_el)
        .children(ripple)
        .children(pointer)
        .children(lamp_el);

    // The housing glows with the lamp's pulse while working.
    if working {
        let (accent, blur) = (t.accent, size.max(px(20.)) * 0.4);
        housing
            .with_animation((id, "glow"), pulse(), move |d, a| d.shadow(glow(accent.opacity(a * 0.45), blur)))
            .into_any_element()
    } else {
        housing.into_any_element()
    }
}
