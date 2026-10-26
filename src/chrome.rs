//! The window frame, Zed style: no OS title bar, our own bar across the top.
//!
//! - Windows: the bar and buttons tell the OS what they are
//!   (`WindowControlArea`), so dragging, Aero Snap and the snap-layout flyout
//!   on the maximize button all work natively.
//! - macOS: the native traffic lights stay; the bar leaves room for them.
//! - Linux: we draw the buttons and drive the window ourselves.

use gpui::{App, MouseButton, Pixels, Stateful, Window, WindowControlArea, div, prelude::*, px};

use crate::theme::theme;
use crate::ui::{Icon, icon};

pub const TITLE_BAR_HEIGHT: Pixels = px(36.);

/// Room for the macOS traffic lights at the bar's left end.
pub fn leading_inset() -> Pixels {
    if cfg!(target_os = "macos") { px(76.) } else { px(0.) }
}

/// Makes an element part of the draggable title bar: drag to move,
/// double-click to maximize.
pub fn drag_area(el: Stateful<gpui::Div>) -> Stateful<gpui::Div> {
    if cfg!(target_os = "windows") {
        el.window_control_area(WindowControlArea::Drag)
    } else {
        el.on_mouse_down(MouseButton::Left, |ev, window, _| {
            if ev.click_count >= 2 {
                window.titlebar_double_click();
            } else {
                window.start_window_move();
            }
        })
    }
}

/// Minimize, maximize/restore and close, at the bar's right end. None on
/// macOS, which keeps its own.
pub fn window_controls(window: &Window, cx: &App) -> impl IntoElement {
    let t = theme(cx);
    let maximized = window.is_maximized();
    let button = |id: &'static str, which: Icon, area: WindowControlArea, close: bool| {
        let hover = if close { gpui::rgb(0xc42b1c).into() } else { t.hover };
        let fg = t.text_muted;
        div()
            .id(id)
            .flex()
            .items_center()
            .justify_center()
            .w(px(46.))
            .h_full()
            .text_color(fg)
            .hover(move |s| s.bg(hover).text_color(gpui::white()))
            .child(icon(which, fg).size(px(14.)))
            .when(cfg!(target_os = "windows"), |d| d.window_control_area(area))
            .when(!cfg!(target_os = "windows"), |d| {
                d.on_click(move |_, window, _| match area {
                    WindowControlArea::Min => window.minimize_window(),
                    WindowControlArea::Max => window.zoom_window(),
                    _ => window.remove_window(),
                })
            })
    };
    div().flex().h_full().flex_none().when(!cfg!(target_os = "macos"), |d| {
        d.child(button("win-min", Icon::Minimize, WindowControlArea::Min, false))
            .child(button(
                "win-max",
                if maximized { Icon::Restore } else { Icon::Maximize },
                WindowControlArea::Max,
                false,
            ))
            .child(button("win-close", Icon::Close, WindowControlArea::Close, true))
    })
}
