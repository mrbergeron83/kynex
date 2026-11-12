//! The Kynex mark, live: a housing with a K whose stem is a text caret (the operator's,
//! in petrol) and whose arms are a prompt chevron. The caret is the lamp: it takes the
//! shown session's state colour, and while Claude works it pulses and the housing glows
//! with it. Geometry and colours from the dark mark in docs/design/branding.md, on a
//! 100-unit canvas, whatever the theme: the logo has one look.

use gpui::{AnimationExt, AnyElement, App, BoxShadow, ElementId, Hsla, Pixels, div, point, prelude::*, px};

use super::{Icon, Lamp, icon, pulse};

/// A soft halo of `color` around an element, `blur` wide.
fn glow(color: Hsla, blur: Pixels) -> Vec<BoxShadow> {
    vec![BoxShadow { color, offset: point(px(0.), px(0.)), blur_radius: blur, spread_radius: px(0.), inset: false }]
}

pub fn mark(id: impl Into<ElementId>, size: Pixels, lamp: Lamp, cx: &App) -> AnyElement {
    let id = id.into();
    let [housing_c, rim, accent, chevron_c] = crate::theme::mark_colours();
    let u = move |v: f32| size * (v / 100.);
    let working = lamp == Lamp::Working;

    // Below ~20 px the rim is too fine to read.
    let detailed = size >= px(20.);

    // The arms: an svg over the whole housing, so its 24-unit grid is the mark's 100.
    let chevron = icon(Icon::MarkChevron, chevron_c).size(size).absolute().left(px(0.)).top(px(0.));

    // The caret keeps the theme's state colours, but idle and working are the mark's petrol.
    let c = if working { accent } else { lamp.color(cx).unwrap_or(accent) };
    let caret = div().absolute().left(u(26.)).top(u(20.)).w(u(12.)).h(u(60.)).rounded_full().bg(c);
    let caret = if working {
        caret
            .with_animation((id.clone(), "caret"), pulse(), move |d, a| d.opacity(a).shadow(glow(c.opacity(a), u(14.))))
            .into_any_element()
    } else if lamp == Lamp::Idle {
        caret.into_any_element()
    } else {
        caret.shadow(glow(c.opacity(0.6), u(14.))).into_any_element()
    };

    let housing = div()
        .id(id.clone())
        .relative()
        .flex_none()
        .size(size)
        .rounded(u(20.))
        .bg(housing_c)
        .when(detailed, |d| d.border_1().border_color(rim))
        .child(chevron)
        .child(caret);

    // The housing glows with the caret's pulse while working.
    if working {
        let blur = size.max(px(20.)) * 0.4;
        housing
            .with_animation((id, "glow"), pulse(), move |d, a| d.shadow(glow(accent.opacity(a * 0.45), blur)))
            .into_any_element()
    } else {
        housing.into_any_element()
    }
}
