//! Switchable looks. The default is Hyprland-inspired: dark gapped tiles with
//! rounded corners and a cyan→green gradient border on the focused tile.
//! The VS Code look uses Dark Modern colours, tight gaps, square-ish corners
//! and a solid blue focus border.

use std::sync::atomic::{AtomicU8, Ordering};

use eframe::egui::{self, Color32, CornerRadius, Margin, Pos2, Rect, Stroke, Vec2, epaint};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Theme {
    #[default]
    Hyprland,
    VsCode,
}

pub const THEMES: [(Theme, &str); 2] = [(Theme::Hyprland, "Hyprland"), (Theme::VsCode, "VS Code")];

pub struct Palette {
    pub crust: Color32,
    pub mantle: Color32,
    pub base: Color32,
    pub surface0: Color32,
    pub surface1: Color32,
    pub surface2: Color32,
    pub overlay: Color32,
    pub subtext: Color32,
    pub text: Color32,
    pub blue: Color32,
    pub green: Color32,
    pub red: Color32,
    pub peach: Color32,
    pub yellow: Color32,
    /// Focus border gradient; the two ends are equal for a solid border.
    pub border_a: Color32,
    pub border_b: Color32,
    /// Text drawn on top of an accent fill (active pill, hovered close button).
    pub on_accent: Color32,
    pub gap: f32,
    pub radius: u8,
    pub small_radius: u8,
    pub window_radius: u8,
    pub border: f32,
}

// Catppuccin Mocha, the palette most Hyprland setups use.
// Border is Hyprland's default `col.active_border`: rgba(33ccffee) rgba(00ff99ee) 45deg.
static HYPRLAND: Palette = Palette {
    crust: Color32::from_rgb(0x11, 0x11, 0x1b),
    mantle: Color32::from_rgb(0x18, 0x18, 0x25),
    base: Color32::from_rgb(0x1e, 0x1e, 0x2e),
    surface0: Color32::from_rgb(0x31, 0x32, 0x44),
    surface1: Color32::from_rgb(0x45, 0x47, 0x5a),
    surface2: Color32::from_rgb(0x58, 0x5b, 0x70),
    overlay: Color32::from_rgb(0x6c, 0x70, 0x86),
    subtext: Color32::from_rgb(0xa6, 0xad, 0xc8),
    text: Color32::from_rgb(0xcd, 0xd6, 0xf4),
    blue: Color32::from_rgb(0x89, 0xb4, 0xfa),
    green: Color32::from_rgb(0xa6, 0xe3, 0xa1),
    red: Color32::from_rgb(0xf3, 0x8b, 0xa8),
    peach: Color32::from_rgb(0xfa, 0xb3, 0x87),
    yellow: Color32::from_rgb(0xf9, 0xe2, 0xaf),
    border_a: Color32::from_rgb(0x33, 0xcc, 0xff),
    border_b: Color32::from_rgb(0x00, 0xff, 0x99),
    on_accent: Color32::from_rgb(0x11, 0x11, 0x1b),
    gap: 8.0,
    radius: 10,
    small_radius: 6,
    window_radius: 12,
    border: 2.0,
};

// VS Code's Dark Modern theme.
static VSCODE: Palette = Palette {
    crust: Color32::from_rgb(0x18, 0x18, 0x18),
    mantle: Color32::from_rgb(0x2a, 0x2a, 0x2a),
    base: Color32::from_rgb(0x1f, 0x1f, 0x1f),
    surface0: Color32::from_rgb(0x31, 0x31, 0x31),
    surface1: Color32::from_rgb(0x3c, 0x3c, 0x3c),
    surface2: Color32::from_rgb(0x45, 0x45, 0x45),
    overlay: Color32::from_rgb(0x6e, 0x76, 0x81),
    subtext: Color32::from_rgb(0x9d, 0x9d, 0x9d),
    text: Color32::from_rgb(0xcc, 0xcc, 0xcc),
    blue: Color32::from_rgb(0x37, 0x94, 0xff),
    green: Color32::from_rgb(0x89, 0xd1, 0x85),
    red: Color32::from_rgb(0xf1, 0x4c, 0x4c),
    peach: Color32::from_rgb(0xce, 0x91, 0x78),
    yellow: Color32::from_rgb(0xe2, 0xc0, 0x8d),
    border_a: Color32::from_rgb(0x00, 0x78, 0xd4),
    border_b: Color32::from_rgb(0x00, 0x78, 0xd4),
    on_accent: Color32::WHITE,
    gap: 4.0,
    radius: 2,
    small_radius: 2,
    window_radius: 8,
    border: 1.0,
};

static CURRENT: AtomicU8 = AtomicU8::new(Theme::Hyprland as u8);

/// The active palette.
pub fn p() -> &'static Palette {
    match CURRENT.load(Ordering::Relaxed) {
        x if x == Theme::VsCode as u8 => &VSCODE,
        _ => &HYPRLAND,
    }
}

/// Switch to `theme` and restyle egui's widgets to match.
pub fn apply(ctx: &egui::Context, theme: Theme) {
    CURRENT.store(theme as u8, Ordering::Relaxed);
    let p = p();

    ctx.all_styles_mut(|style| {
        let v = &mut style.visuals;
        *v = egui::Visuals::dark();
        v.panel_fill = p.crust;
        v.window_fill = p.base;
        v.extreme_bg_color = p.mantle;
        v.faint_bg_color = p.mantle;
        v.code_bg_color = p.mantle;
        v.override_text_color = Some(p.text);
        v.hyperlink_color = p.blue;
        v.warn_fg_color = p.yellow;
        v.error_fg_color = p.red;
        v.selection.bg_fill = p.border_a.gamma_multiply(0.35);
        v.selection.stroke = Stroke::new(1.0, p.border_a);
        v.window_corner_radius = CornerRadius::same(p.radius);
        v.menu_corner_radius = CornerRadius::same(p.radius);
        v.window_stroke = Stroke::new(p.border, p.surface1);
        v.window_shadow = epaint::Shadow { offset: [0, 6], blur: 20, spread: 0, color: Color32::from_black_alpha(140) };
        v.popup_shadow = v.window_shadow;

        let round = CornerRadius::same(p.small_radius);
        for (w, fill, fg) in [
            (&mut v.widgets.noninteractive, p.base, p.subtext),
            (&mut v.widgets.inactive, p.surface0, p.text),
            (&mut v.widgets.hovered, p.surface1, p.text),
            (&mut v.widgets.active, p.surface2, p.text),
            (&mut v.widgets.open, p.surface1, p.text),
        ] {
            w.bg_fill = fill;
            w.weak_bg_fill = fill;
            w.bg_stroke = Stroke::NONE;
            w.fg_stroke = Stroke::new(1.0, fg);
            w.corner_radius = round;
        }
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.surface0);
        v.widgets.hovered.bg_stroke = Stroke::new(1.0, p.surface2);

        style.spacing.item_spacing = Vec2::new(8.0, 6.0);
        style.spacing.button_padding = Vec2::new(10.0, 4.0);
    });
}

/// Use a coding font everywhere when one is installed, for the riced look.
/// Falls back to egui's bundled fonts on systems without them.
pub fn install_fonts(ctx: &egui::Context) {
    let candidates = [
        "C:\\Windows\\Fonts\\CascadiaCode.ttf",
        "C:\\Windows\\Fonts\\CascadiaMono.ttf",
        "/usr/share/fonts/TTF/JetBrainsMono-Regular.ttf",
        "/usr/share/fonts/truetype/jetbrains-mono/JetBrainsMono-Regular.ttf",
        "/System/Library/Fonts/SFNSMono.ttf",
        "/System/Library/Fonts/Menlo.ttc",
    ];
    let Some(bytes) = candidates.iter().find_map(|p| std::fs::read(p).ok()) else { return };

    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert("kynex-mono".into(), egui::FontData::from_owned(bytes).into());
    for family in [egui::FontFamily::Monospace, egui::FontFamily::Proportional] {
        // Prepend so egui's bundled fonts still cover emoji and missing glyphs.
        fonts.families.entry(family).or_default().insert(0, "kynex-mono".into());
    }
    ctx.set_fonts(fonts);
}

/// A Hyprland-style window: rounded, padded, with a gradient border when focused.
/// `fill` makes the tile take all remaining space.
pub fn tile<R>(
    ui: &mut egui::Ui,
    id: &str,
    title: &str,
    focused: bool,
    fill: bool,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let p = p();
    let id = ui.scope_id().with(id);
    let inner = egui::Frame::new()
        .fill(p.base)
        .corner_radius(CornerRadius::same(p.radius))
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            if fill {
                ui.set_min_size(ui.available_size());
            } else {
                ui.set_min_width(ui.available_width());
            }
            if !title.is_empty() {
                ui.label(egui::RichText::new(title).small().color(p.overlay));
                ui.add_space(2.0);
            }
            add(ui)
        });

    let t = ui.ctx().animate_bool_with_time(id, focused, 0.15);
    let rect = inner.response.rect;
    let painter = ui.painter();
    painter.rect_stroke(rect, CornerRadius::same(p.radius), Stroke::new(p.border, p.surface0), egui::StrokeKind::Inside);
    if t > 0.0 {
        gradient_border(painter, rect.shrink(p.border / 2.0), p.radius as f32, p.border, t);
    }
    inner.inner
}

/// Paints a rounded-rect outline whose colour runs from `border_a` (top-left)
/// to `border_b` (bottom-right), like Hyprland's 45° border gradient.
pub fn gradient_border(painter: &egui::Painter, rect: Rect, radius: f32, width: f32, opacity: f32) {
    let path = rounded_outline(rect, radius);
    let mut mesh = epaint::Mesh::default();
    let half = width / 2.0;
    for (p, n) in &path {
        let c = gradient_at(rect, *p).gamma_multiply(opacity);
        mesh.colored_vertex(*p + *n * half, c);
        mesh.colored_vertex(*p - *n * half, c);
    }
    let count = path.len() as u32;
    for i in 0..count {
        let j = (i + 1) % count;
        let (a, b, c, d) = (2 * i, 2 * i + 1, 2 * j, 2 * j + 1);
        mesh.add_triangle(a, b, c);
        mesh.add_triangle(b, d, c);
    }
    painter.add(egui::Shape::mesh(mesh));
}

/// A rounded rect filled with the border gradient.
pub fn gradient_fill(painter: &egui::Painter, rect: Rect, radius: f32) {
    let path = rounded_outline(rect, radius);
    let mut mesh = epaint::Mesh::default();
    mesh.colored_vertex(rect.center(), gradient_at(rect, rect.center()));
    for (p, _) in &path {
        mesh.colored_vertex(*p, gradient_at(rect, *p));
    }
    let count = path.len() as u32;
    for i in 0..count {
        mesh.add_triangle(0, 1 + i, 1 + (i + 1) % count);
    }
    painter.add(egui::Shape::mesh(mesh));
}

/// Points around a rounded rect, clockwise from the top-right corner, each with its outward normal.
fn rounded_outline(rect: Rect, radius: f32) -> Vec<(Pos2, Vec2)> {
    const STEPS: usize = 10;
    let r = radius.min(rect.width() / 2.0).min(rect.height() / 2.0);
    let corners = [
        (Pos2::new(rect.right() - r, rect.top() + r), -90.0_f32),
        (Pos2::new(rect.right() - r, rect.bottom() - r), 0.0),
        (Pos2::new(rect.left() + r, rect.bottom() - r), 90.0),
        (Pos2::new(rect.left() + r, rect.top() + r), 180.0),
    ];
    let mut path = Vec::with_capacity(4 * (STEPS + 1));
    for (center, start) in corners {
        for s in 0..=STEPS {
            let a = (start + 90.0 * s as f32 / STEPS as f32).to_radians();
            let n = Vec2::new(a.cos(), a.sin());
            path.push((center + n * r, n));
        }
    }
    path
}

/// Colour of the 45° gradient at `pos`: `border_a` at the top-left, `border_b` at the bottom-right.
fn gradient_at(rect: Rect, pos: Pos2) -> Color32 {
    let t = ((pos.x - rect.left()) + (pos.y - rect.top())) / (rect.width() + rect.height()).max(1.0);
    lerp_color(p().border_a, p().border_b, t)
}

pub fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let mix = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()))
}

/// A Waybar-style workspace pill.
pub fn workspace_pill(ui: &mut egui::Ui, number: usize, label: &str, active: bool, badge: Option<Color32>) -> egui::Response {
    let p = p();
    let num = egui::RichText::new(number.to_string()).color(if active { p.on_accent } else { p.overlay }).strong();
    let name = egui::RichText::new(label).color(if active { p.on_accent } else { p.text });
    let mut job = egui::text::LayoutJob::default();
    num.append_to(&mut job, ui.style(), egui::FontSelection::Default, egui::Align::Center);
    egui::RichText::new("  ").append_to(&mut job, ui.style(), egui::FontSelection::Default, egui::Align::Center);
    name.append_to(&mut job, ui.style(), egui::FontSelection::Default, egui::Align::Center);

    let galley = ui.painter().layout_job(job);
    let pad = Vec2::new(12.0, 5.0);
    let dot = if badge.is_some() { 12.0 } else { 0.0 };
    let size = galley.size() + pad * 2.0 + Vec2::new(dot, 0.0);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());

    let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.1);
    let painter = ui.painter();
    let radius = p.small_radius + 2;
    if active {
        // Filled with the border gradient, like an active Hyprland workspace.
        gradient_fill(painter, rect, radius as f32);
    } else {
        painter.rect_filled(rect, CornerRadius::same(radius), lerp_color(p.surface0, p.surface1, hover));
    }
    painter.galley(rect.min + pad, galley, p.text);
    if let Some(c) = badge {
        painter.circle_filled(Pos2::new(rect.right() - pad.x - 3.0, rect.center().y), 3.5, c);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}
