//! Switchable looks, Zed style: flat surfaces, 1px borders, small radii, and
//! one accent colour. The palette is a gpui global, read with `theme(cx)`.
//! The Kynex themes and the bundled fonts follow docs/design/branding.md.

use std::borrow::Cow;
use std::sync::{OnceLock, RwLock};

use gpui::{App, Global, Hsla, SharedString, rgb};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Theme {
    #[default]
    KynexDark,
    KynexLight,
    OneDark,
    /// Catppuccin Mocha, the palette most Hyprland setups use. Kept under its
    /// old name so saved state still loads.
    Hyprland,
    VsCode,
}

pub const THEMES: [(Theme, &str); 5] = [
    (Theme::KynexDark, "Kynex Dark"),
    (Theme::KynexLight, "Kynex Light"),
    (Theme::OneDark, "One Dark"),
    (Theme::Hyprland, "Catppuccin Mocha"),
    (Theme::VsCode, "VS Code Dark"),
];

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
    /// Text and icons on an accent fill (the primary button).
    pub on_accent: Hsla,
    pub hover: Hsla,
    pub active: Hsla,
    /// Inputs and code blocks, a step away from the surface they sit on.
    pub element: Hsla,
    pub selection: Hsla,
    pub success: Hsla,
    pub warning: Hsla,
    pub error: Hsla,
    pub info: Hsla,
    /// Added and removed line backgrounds in diffs.
    pub diff_add: Hsla,
    pub diff_del: Hsla,
}

fn c(hex: u32) -> Hsla {
    rgb(hex).into()
}

fn ca(hex: u32, alpha: f32) -> Hsla {
    let mut h = c(hex);
    h.a = alpha;
    h
}

impl Theme {
    pub fn palette(self) -> Palette {
        match self {
            // Instrument · Petrol: near-colourless housing, petrol for your controls,
            // IEC 60073 lamp colours for state.
            Theme::KynexDark => Palette {
                title_bar: c(0x0f0e0c),
                panel: c(0x151412),
                editor: c(0x1b1917),
                elevated: c(0x22201e),
                border: c(0x33312e),
                border_variant: c(0x282624),
                border_focused: c(0x77becf),
                text: c(0xe5e2df),
                text_muted: c(0xa7a4a0),
                text_placeholder: c(0x777471),
                accent: c(0x77becf),
                on_accent: c(0x0f0e0c),
                hover: c(0x252421),
                active: c(0x2e2c29),
                element: c(0x22201e),
                selection: c(0x18323a),
                success: c(0x6fc082),
                warning: c(0xf2af48),
                error: c(0xe8796c),
                info: c(0x77becf),
                diff_add: c(0x1a321f),
                diff_del: c(0x3f221f),
            },
            Theme::KynexLight => Palette {
                title_bar: c(0xe8e6e3),
                panel: c(0xf0edea),
                editor: c(0xf9f7f5),
                elevated: c(0xfefcfa),
                border: c(0xd4d2cf),
                border_variant: c(0xe2dfdc),
                border_focused: c(0x07657d),
                text: c(0x292622),
                text_muted: c(0x5d5a56),
                text_placeholder: c(0x888682),
                accent: c(0x07657d),
                on_accent: c(0xffffff),
                hover: c(0xe8e6e3),
                active: c(0xdedcd9),
                element: c(0xf0edea),
                selection: c(0xd7ecf4),
                success: c(0x348f4f),
                warning: c(0xd78c00),
                error: c(0xc8443a),
                info: c(0x07657d),
                diff_add: c(0xd9f3dd),
                diff_del: c(0xffe3de),
            },
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
                on_accent: c(0x282c33),
                hover: c(0x363c46),
                active: c(0x454a56),
                element: c(0x2e343e),
                selection: ca(0x74ade8, 0.25),
                success: c(0xa1c181),
                warning: c(0xdec184),
                error: c(0xd07277),
                info: c(0x74ade8),
                diff_add: ca(0xa1c181, 0.12),
                diff_del: ca(0xd07277, 0.12),
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
                on_accent: c(0x1e1e2e),
                hover: c(0x313244),
                active: c(0x45475a),
                element: c(0x181825),
                selection: ca(0x89b4fa, 0.25),
                success: c(0xa6e3a1),
                warning: c(0xf9e2af),
                error: c(0xf38ba8),
                info: c(0x89b4fa),
                diff_add: ca(0xa6e3a1, 0.12),
                diff_del: ca(0xf38ba8, 0.12),
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
                on_accent: c(0x1f1f1f),
                hover: c(0x2a2d2e),
                active: c(0x37373d),
                element: c(0x313131),
                selection: ca(0x264f78, 0.9),
                success: c(0x89d185),
                warning: c(0xe2c08d),
                error: c(0xf14c4c),
                info: c(0x3794ff),
                diff_add: ca(0x89d185, 0.12),
                diff_del: ca(0xf14c4c, 0.12),
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

/// The built-in fonts, used unless you pick others (Appearance menu): IBM Plex Sans
/// for the interface, JetBrains Mono for code, diffs and the terminal.
pub const DEFAULT_UI_FONT: &str = "IBM Plex Sans";
pub const DEFAULT_CODE_FONT: &str = "JetBrains Mono";

/// Bundled so the look doesn't depend on what's installed; all SIL OFL 1.1, their
/// licences sit next to them in assets/fonts.
const FONTS: [&[u8]; 11] = [
    include_bytes!("../assets/fonts/IBMPlexSans-Regular.ttf"),
    include_bytes!("../assets/fonts/IBMPlexSans-Italic.ttf"),
    include_bytes!("../assets/fonts/IBMPlexSans-Medium.ttf"),
    include_bytes!("../assets/fonts/IBMPlexSans-SemiBold.ttf"),
    include_bytes!("../assets/fonts/IBMPlexSans-Bold.ttf"),
    include_bytes!("../assets/fonts/HubotSansExpanded-SemiBold.ttf"),
    include_bytes!("../assets/fonts/HubotSansExpanded-ExtraBold.ttf"),
    include_bytes!("../assets/fonts/HubotSansExpanded-Black.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMono-Bold.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMono-Italic.ttf"),
];

/// Installed fonts worth offering first in the pickers, when present.
pub const SUGGESTED_UI_FONTS: [&str; 9] = [
    DEFAULT_UI_FONT,
    "Segoe UI",
    "Inter",
    "SF Pro Text",
    "Helvetica Neue",
    "Roboto",
    "Noto Sans",
    "Ubuntu",
    "Cantarell",
];
pub const SUGGESTED_CODE_FONTS: [&str; 10] = [
    DEFAULT_CODE_FONT,
    "Cascadia Code",
    "Cascadia Mono",
    "Consolas",
    "Fira Code",
    "Source Code Pro",
    "SF Mono",
    "Menlo",
    "DejaVu Sans Mono",
    "Ubuntu Mono",
];

static UI: RwLock<Option<SharedString>> = RwLock::new(None);
static CODE: RwLock<Option<SharedString>> = RwLock::new(None);
static LABEL: OnceLock<SharedString> = OnceLock::new();

/// The interface font: everything you read.
pub fn ui_font() -> SharedString {
    UI.read().ok().and_then(|f| f.clone()).unwrap_or_else(|| DEFAULT_UI_FONT.into())
}

/// The code font: code blocks, diffs, tool output, the terminal, paths and branches.
pub fn mono() -> SharedString {
    CODE.read().ok().and_then(|f| f.clone()).unwrap_or_else(|| DEFAULT_CODE_FONT.into())
}

/// Sets the interface and code fonts; an empty name means the built-in one.
pub fn set_fonts(ui: &str, code: &str) {
    let pick = |f: &str| (!f.is_empty()).then(|| SharedString::from(f.to_string()));
    if let Ok(mut u) = UI.write() {
        *u = pick(ui);
    }
    if let Ok(mut c) = CODE.write() {
        *c = pick(code);
    }
}

/// Engraved-panel lettering for the wordmark and uppercase section labels: Hubot
/// Sans Expanded. DirectWrite files the expanded cuts under "Hubot Sans" (width is
/// a stretch there, not a family), other platforms under the full name.
pub fn label_font() -> SharedString {
    LABEL.get().cloned().unwrap_or_else(ui_font)
}

/// Every font family the pickers can offer, sorted; the built-in ones included.
pub fn font_families(cx: &App) -> Vec<String> {
    let mut all = cx.text_system().all_font_names();
    all.extend([DEFAULT_UI_FONT.to_string(), DEFAULT_CODE_FONT.to_string()]);
    // Windows lists vertical-writing variants as "@Family".
    all.retain(|f| !f.starts_with('@') && !f.starts_with('.'));
    all.sort_by_key(|f| f.to_lowercase());
    all.dedup();
    all
}

pub fn init_fonts(cx: &App) {
    if let Err(e) = cx.text_system().add_fonts(FONTS.iter().map(|f| Cow::Borrowed(*f)).collect()) {
        eprintln!("kynex: couldn't load the bundled fonts: {e}");
    }
    let installed = cx.text_system().all_font_names();
    if let Some(f) = ["Hubot Sans Expanded", "Hubot Sans"].into_iter().find(|f| installed.iter().any(|n| n == f)) {
        let _ = LABEL.set(f.into());
    }
}
