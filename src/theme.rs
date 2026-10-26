//! Switchable looks, Zed style: flat surfaces, 1px borders, small radii, and
//! one accent colour. The palette is a gpui global, read with `theme(cx)`.

use std::sync::OnceLock;

use gpui::{App, Global, Hsla, Rgba, SharedString, rgb};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Theme {
    #[default]
    OneDark,
    /// Catppuccin Mocha, the palette most Hyprland setups use. Kept under its
    /// old name so saved state still loads.
    Hyprland,
    VsCode,
}

pub const THEMES: [(Theme, &str); 3] =
    [(Theme::OneDark, "One Dark"), (Theme::Hyprland, "Catppuccin Mocha"), (Theme::VsCode, "VS Code Dark")];

#[derive(Clone)]
pub struct Palette {
    /// Title bar and status areas.
    pub title_bar: Hsla,
    /// Side panels.
    pub panel: Hsla,
    /// The main reading surface: transcript, diffs.
    pub editor: Hsla,
    /// Popovers, menus, modals.
    pub elevated: Hsla,
    pub border: Hsla,
    /// Fainter borders between rows and sections.
    pub border_variant: Hsla,
    /// The focus ring.
    pub border_focused: Hsla,
    pub text: Hsla,
    pub text_muted: Hsla,
    pub text_placeholder: Hsla,
    pub accent: Hsla,
    pub hover: Hsla,
    pub active: Hsla,
    /// Inputs and code blocks, a step away from the surface they sit on.
    pub element: Hsla,
    pub selection: Hsla,
    pub success: Hsla,
    pub warning: Hsla,
    pub error: Hsla,
    pub info: Hsla,
}

fn c(hex: u32) -> Hsla {
    Rgba::from(rgb(hex)).into()
}

fn ca(hex: u32, alpha: f32) -> Hsla {
    let mut h = c(hex);
    h.a = alpha;
    h
}

impl Theme {
    pub fn palette(self) -> Palette {
        match self {
            // Zed's One Dark.
            Theme::OneDark => Palette {
                title_bar: c(0x3b414d),
                panel: c(0x2f343e),
                editor: c(0x282c33),
                elevated: c(0x2f343e),
                border: c(0x464b57),
                border_variant: c(0x363c46),
                border_focused: c(0x47679e),
                text: c(0xdce0e5),
                text_muted: c(0xa9afbc),
                text_placeholder: c(0x878a98),
                accent: c(0x74ade8),
                hover: c(0x363c46),
                active: c(0x454a56),
                element: c(0x2e343e),
                selection: ca(0x74ade8, 0.25),
                success: c(0xa1c181),
                warning: c(0xdec184),
                error: c(0xd07277),
                info: c(0x74ade8),
            },
            Theme::Hyprland => Palette {
                title_bar: c(0x11111b),
                panel: c(0x181825),
                editor: c(0x1e1e2e),
                elevated: c(0x181825),
                border: c(0x313244),
                border_variant: c(0x26263a),
                border_focused: c(0x89b4fa),
                text: c(0xcdd6f4),
                text_muted: c(0xa6adc8),
                text_placeholder: c(0x6c7086),
                accent: c(0x89b4fa),
                hover: c(0x313244),
                active: c(0x45475a),
                element: c(0x181825),
                selection: ca(0x89b4fa, 0.25),
                success: c(0xa6e3a1),
                warning: c(0xf9e2af),
                error: c(0xf38ba8),
                info: c(0x89b4fa),
            },
            // VS Code's Dark Modern.
            Theme::VsCode => Palette {
                title_bar: c(0x181818),
                panel: c(0x181818),
                editor: c(0x1f1f1f),
                elevated: c(0x202020),
                border: c(0x2b2b2b),
                border_variant: c(0x2b2b2b),
                border_focused: c(0x0078d4),
                text: c(0xcccccc),
                text_muted: c(0x9d9d9d),
                text_placeholder: c(0x6e7681),
                accent: c(0x0078d4),
                hover: c(0x2a2d2e),
                active: c(0x37373d),
                element: c(0x313131),
                selection: ca(0x264f78, 0.9),
                success: c(0x89d185),
                warning: c(0xe2c08d),
                error: c(0xf14c4c),
                info: c(0x3794ff),
            },
        }
    }
}

pub struct ActiveTheme(pub Palette);

impl Global for ActiveTheme {}

pub fn set(theme: Theme, cx: &mut App) {
    cx.set_global(ActiveTheme(theme.palette()));
}

pub fn theme(cx: &App) -> &Palette {
    &cx.global::<ActiveTheme>().0
}

static MONO: OnceLock<SharedString> = OnceLock::new();

/// Monospace family for code, diffs and tool output: the first one installed.
pub fn mono() -> SharedString {
    MONO.get().cloned().unwrap_or_else(|| "monospace".into())
}

pub fn init_fonts(cx: &App) {
    const PREFERRED: [&str; 8] = [
        "Cascadia Mono",
        "Cascadia Code",
        "JetBrains Mono",
        "SF Mono",
        "Menlo",
        "Consolas",
        "DejaVu Sans Mono",
        "Liberation Mono",
    ];
    let installed = cx.text_system().all_font_names();
    let pick = PREFERRED.iter().find(|f| installed.iter().any(|n| n == *f)).copied().unwrap_or("monospace");
    let _ = MONO.set(pick.into());
}
