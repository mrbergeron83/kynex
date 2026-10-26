//! Custom window chrome: Kynex draws its own frame instead of the OS title bar,
//! so the whole window matches the Hyprland look.

use eframe::egui::{self, CornerRadius, CursorIcon, Pos2, Rect, Sense, Stroke, Vec2, ViewportCommand};

use crate::theme;

const RESIZE_EDGE: f32 = 6.0;

fn is_maximized(ctx: &egui::Context) -> bool {
    ctx.input(|i| i.viewport().maximized.unwrap_or(false))
}

/// Paints the window background: rounded and slightly see-through when floating,
/// square when maximized.
pub fn background(ui: &egui::Ui) {
    let rect = ui.max_rect();
    let p = theme::p();
    let radius = if is_maximized(ui.ctx()) { 0 } else { p.window_radius };
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(radius), p.crust.gamma_multiply(0.94));
    painter.rect_stroke(rect, CornerRadius::same(radius), Stroke::new(1.0, p.surface0), egui::StrokeKind::Inside);
}

/// Fills the remaining space in a row with a handle that moves the window,
/// and maximizes/restores it on double-click.
pub fn drag_region(ui: &mut egui::Ui) {
    let (_, resp) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
    if resp.double_clicked() {
        let max = is_maximized(ui.ctx());
        ui.ctx().send_viewport_cmd(ViewportCommand::Maximized(!max));
    } else if resp.drag_started_by(egui::PointerButton::Primary) {
        ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
    }
}

#[derive(Clone, Copy)]
enum Control {
    Minimize,
    Maximize,
    Close,
}

/// Minimize / maximize / close, drawn as thin line icons. Call inside a right-to-left layout.
pub fn window_controls(ui: &mut egui::Ui) {
    for c in [Control::Close, Control::Maximize, Control::Minimize] {
        if control_button(ui, c).clicked() {
            let ctx = ui.ctx();
            match c {
                Control::Close => ctx.send_viewport_cmd(ViewportCommand::Close),
                Control::Maximize => ctx.send_viewport_cmd(ViewportCommand::Maximized(!is_maximized(ctx))),
                Control::Minimize => ctx.send_viewport_cmd(ViewportCommand::Minimized(true)),
            }
        }
    }
}

fn control_button(ui: &mut egui::Ui, c: Control) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(30.0, 24.0), Sense::click());
    let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.08);
    let painter = ui.painter();
    let p = theme::p();
    let hover_fill = if matches!(c, Control::Close) { p.red } else { p.surface1 };
    if hover > 0.0 {
        painter.rect_filled(rect, CornerRadius::same(p.small_radius), hover_fill.gamma_multiply(hover));
    }
    let fg = if matches!(c, Control::Close) && hover > 0.5 { p.on_accent } else { p.subtext };
    let stroke = Stroke::new(1.3, fg);
    let center = rect.center();
    let s = 4.5;
    match c {
        Control::Minimize => {
            painter.line_segment([center + Vec2::new(-s, 0.0), center + Vec2::new(s, 0.0)], stroke);
        }
        Control::Maximize => {
            if is_maximized(ui.ctx()) {
                let back = Rect::from_center_size(center + Vec2::new(1.5, -1.5), Vec2::splat(2.0 * s - 2.0));
                let front = Rect::from_center_size(center + Vec2::new(-1.5, 1.5), Vec2::splat(2.0 * s - 2.0));
                painter.rect_stroke(back, CornerRadius::same(2), stroke, egui::StrokeKind::Middle);
                painter.rect_filled(front, CornerRadius::same(2), p.base);
                painter.rect_stroke(front, CornerRadius::same(2), stroke, egui::StrokeKind::Middle);
            } else {
                let r = Rect::from_center_size(center, Vec2::splat(2.0 * s));
                painter.rect_stroke(r, CornerRadius::same(2), stroke, egui::StrokeKind::Middle);
            }
        }
        Control::Close => {
            painter.line_segment([center + Vec2::new(-s, -s), center + Vec2::new(s, s)], stroke);
            painter.line_segment([center + Vec2::new(-s, s), center + Vec2::new(s, -s)], stroke);
        }
    }
    resp.on_hover_cursor(CursorIcon::Default)
}

/// Invisible handles along the window edges for resizing an undecorated window.
/// Call last, so they win hit-testing over the content underneath.
pub fn resize_edges(ui: &mut egui::Ui) {
    if is_maximized(ui.ctx()) {
        return;
    }
    use egui::ResizeDirection as D;
    let r = ui.max_rect();
    let e = RESIZE_EDGE;
    let c = e * 2.0;
    let zones = [
        (D::NorthWest, Rect::from_min_size(r.left_top(), Vec2::splat(c)), CursorIcon::ResizeNwSe),
        (D::NorthEast, Rect::from_min_size(Pos2::new(r.right() - c, r.top()), Vec2::splat(c)), CursorIcon::ResizeNeSw),
        (D::SouthWest, Rect::from_min_size(Pos2::new(r.left(), r.bottom() - c), Vec2::splat(c)), CursorIcon::ResizeNeSw),
        (D::SouthEast, Rect::from_min_size(r.right_bottom() - Vec2::splat(c), Vec2::splat(c)), CursorIcon::ResizeNwSe),
        (D::North, Rect::from_min_max(Pos2::new(r.left() + c, r.top()), Pos2::new(r.right() - c, r.top() + e)), CursorIcon::ResizeVertical),
        (D::South, Rect::from_min_max(Pos2::new(r.left() + c, r.bottom() - e), Pos2::new(r.right() - c, r.bottom())), CursorIcon::ResizeVertical),
        (D::West, Rect::from_min_max(Pos2::new(r.left(), r.top() + c), Pos2::new(r.left() + e, r.bottom() - c)), CursorIcon::ResizeHorizontal),
        (D::East, Rect::from_min_max(Pos2::new(r.right() - e, r.top() + c), Pos2::new(r.right(), r.bottom() - c)), CursorIcon::ResizeHorizontal),
    ];
    for (i, (dir, rect, cursor)) in zones.into_iter().enumerate() {
        let resp = ui.interact(rect, ui.scope_id().with(("resize-edge", i)), Sense::drag());
        if resp.hovered() || resp.dragged() {
            ui.ctx().set_cursor_icon(cursor);
        }
        if resp.drag_started_by(egui::PointerButton::Primary) {
            ui.ctx().send_viewport_cmd(ViewportCommand::BeginResize(dir));
        }
    }
}

/// Fully transparent clear colour, so the rounded window corners show the desktop.
pub const CLEAR: [f32; 4] = [0.0, 0.0, 0.0, 0.0];
