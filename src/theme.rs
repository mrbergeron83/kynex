//! Switchable looks, Zed style: flat surfaces, 1px borders, small radii, and
//! one accent colour. The palette is a gpui global, read with `theme(cx)`.
//! The Kynex themes and the bundled fonts follow docs/design/branding.md.

use std::borrow::Cow;
use std::sync::{OnceLock, RwLock};

use gpui::{App, FontStyle, Global, HighlightStyle, Hsla, SharedString, WindowAppearance, rgb};
use kynex_core::highlight::Token;
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
    /// Kynex Light or Kynex Dark, following the OS's light / dark setting live.
    System,
}

pub const THEMES: [(Theme, &str); 6] = [
    (Theme::System, "System (Kynex Light / Dark)"),
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
    /// Selected text in the transcript: clearer than a selected row.
    pub text_selection: Hsla,
    pub success: Hsla,
    pub warning: Hsla,
    pub error: Hsla,
    pub info: Hsla,
    /// Added and removed line backgrounds in diffs.
    pub diff_add: Hsla,
    pub diff_del: Hsla,
    /// Syntax highlighting (`kynex_core::highlight::Token`); comments draw italic.
    pub syntax: Syntax,
}

/// Code colours: low chroma, so code doesn't out-shout the prose.
#[derive(Clone, Copy)]
pub struct Syntax {
    pub keyword: Hsla,
    pub string: Hsla,
    pub r#type: Hsla,
    pub number: Hsla,
    pub comment: Hsla,
    pub function: Hsla,
    pub punctuation: Hsla,
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
    /// The theme to draw with: `System` picks Kynex Light or Dark by the OS's appearance.
    pub fn resolve(self, appearance: WindowAppearance) -> Theme {
        match (self, appearance) {
            (Theme::System, WindowAppearance::Dark | WindowAppearance::VibrantDark) => Theme::KynexDark,
            (Theme::System, _) => Theme::KynexLight,
            (th, _) => th,
        }
    }

    pub fn palette(self) -> Palette {
        match self {
            Theme::System => Theme::KynexDark.palette(),
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
                text_selection: c(0x24505c),
                success: c(0x6fc082),
                warning: c(0xf2af48),
                error: c(0xe8796c),
                info: c(0x77becf),
                diff_add: c(0x1a321f),
                diff_del: c(0x3f221f),
                // Warm keywords on Claude's side, petrol types on the operator's.
                syntax: Syntax {
                    keyword: c(0xcf9a74),
                    string: c(0x9fc48a),
                    r#type: c(0x9ec6d4),
                    number: c(0xd9b36a),
                    comment: c(0x777471),
                    function: c(0xe5e2df),
                    punctuation: c(0xa7a4a0),
                },
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
                text_selection: c(0xb9dbe8),
                success: c(0x348f4f),
                warning: c(0xd78c00),
                error: c(0xc8443a),
                info: c(0x07657d),
                diff_add: c(0xd9f3dd),
                diff_del: c(0xffe3de),
                syntax: Syntax {
                    keyword: c(0xa4542a),
                    string: c(0x3f7f2f),
                    r#type: c(0x07657d),
                    number: c(0x8a6a12),
                    comment: c(0x888682),
                    function: c(0x292622),
                    punctuation: c(0x5d5a56),
                },
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
                text_selection: ca(0x74ade8, 0.3),
                success: c(0xa1c181),
                warning: c(0xdec184),
                error: c(0xd07277),
                info: c(0x74ade8),
                diff_add: ca(0xa1c181, 0.12),
                diff_del: ca(0xd07277, 0.12),
                syntax: Syntax {
                    keyword: c(0xb477cf),
                    string: c(0xa1c181),
                    r#type: c(0x6eb4bf),
                    number: c(0xbf956a),
                    comment: c(0x5d636f),
                    function: c(0x73ade9),
                    punctuation: c(0xacb2be),
                },
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
                text_selection: ca(0x89b4fa, 0.3),
                success: c(0xa6e3a1),
                warning: c(0xf9e2af),
                error: c(0xf38ba8),
                info: c(0x89b4fa),
                diff_add: ca(0xa6e3a1, 0.12),
                diff_del: ca(0xf38ba8, 0.12),
                // Mauve, green, yellow, peach, overlay.
                syntax: Syntax {
                    keyword: c(0xcba6f7),
                    string: c(0xa6e3a1),
                    r#type: c(0xf9e2af),
                    number: c(0xfab387),
                    comment: c(0x7f849c),
                    function: c(0x89b4fa),
                    punctuation: c(0x9399b2),
                },
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
                text_selection: c(0x264f78),
                success: c(0x89d185),
                warning: c(0xe2c08d),
                error: c(0xf14c4c),
                info: c(0x3794ff),
                diff_add: ca(0x89d185, 0.12),
                diff_del: ca(0xf14c4c, 0.12),
                syntax: Syntax {
                    keyword: c(0x569cd6),
                    string: c(0xce9178),
                    r#type: c(0x4ec9b0),
                    number: c(0xb5cea8),
                    comment: c(0x6a9955),
                    function: c(0xdcdcaa),
                    punctuation: c(0xd4d4d4),
                },
            },
        }
    }
}

impl Syntax {
    /// How a highlighted token draws: its colour, and italic for comments.
    pub fn style(&self, token: Token) -> HighlightStyle {
        let color = match token {
            Token::Keyword => self.keyword,
            Token::String => self.string,
            Token::Type => self.r#type,
            Token::Number => self.number,
            Token::Comment => self.comment,
            Token::Function => self.function,
            Token::Punctuation => self.punctuation,
        };
        let font_style = (token == Token::Comment).then_some(FontStyle::Italic);
        HighlightStyle { color: Some(color), font_style, ..Default::default() }
    }
}

impl Palette {
    /// A light theme: dark text on a light surface.
    pub fn is_light(&self) -> bool {
        self.editor.l > 0.5
    }

    /// The window dimmed behind a modal, menu-like overlay or the palette.
    pub fn scrim(&self) -> Hsla {
        ca(0x000000, 0.25)
    }

    /// The window darkened behind a picture shown full size.
    pub fn scrim_media(&self) -> Hsla {
        ca(0x000000, 0.8)
    }

    /// The window's close button on hover, and its glyph: Windows' own red.
    pub fn close_hover(&self) -> (Hsla, Hsla) {
        (c(0xc42b1c), c(0xffffff))
    }

    /// The terminal's 16 ANSI colours (normal, then bright), tuned for this
    /// palette's background: One Dark's on dark themes, One Light's on light ones.
    pub fn ansi(&self) -> [u32; 16] {
        if self.is_light() {
            [
                0x383a42, 0xca1243, 0x50a14f, 0xc18401, 0x4078f2, 0xa626a4, 0x0184bc, 0xa0a1a7, 0x696c77, 0xe45649,
                0x4bbf48, 0xd29b00, 0x5a8cff, 0xc04fbe, 0x22a1d6, 0x202227,
            ]
        } else {
            [
                0x3b4048, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xabb2bf, 0x5c6370, 0xff7b86,
                0xb5e890, 0xffd68a, 0x82c4ff, 0xde9bf0, 0x7fd8e3, 0xffffff,
            ]
        }
    }

    /// The terminal's background: the editor's, a step darker on dark themes.
    pub fn terminal_background(&self) -> Hsla {
        if self.is_light() { self.editor } else { mix(self.editor, c(0x000000), 0.25) }
    }

    /// A project colour (`kynex_core::project::COLOURS`) as a shade that reads on
    /// this palette: lighter on dark themes, deeper on light ones.
    pub fn project_colour(&self, name: &str) -> Hsla {
        let hue = match name {
            "red" => 0.,
            "orange" => 28.,
            "yellow" => 48.,
            "green" => 135.,
            "teal" => 178.,
            "blue" => 212.,
            "violet" => 268.,
            _ => 325.,
        };
        let (s, l) = if self.is_light() { (0.62, 0.40) } else { (0.58, 0.64) };
        gpui::hsla(hue / 360., s, l, 1.)
    }
}

/// `over` laid on `base` at `amount` (0 to 1), as one opaque colour.
/// The app mark's colours (`mark-dark.svg`), the same in every theme: housing,
/// rim, knob and pointer.
pub fn mark_colours() -> [Hsla; 4] {
    [0x0f0e0c, 0x33312e, 0x77becf, 0x22201e].map(c)
}

pub fn mix(base: Hsla, over: Hsla, amount: f32) -> Hsla {
    let (a, b) = (base.to_rgb(), over.to_rgb());
    let lerp = |x: f32, y: f32| x + (y - x) * amount;
    gpui::Rgba { r: lerp(a.r, b.r), g: lerp(a.g, b.g), b: lerp(a.b, b.b), a: 1. }.into()
}

pub struct ActiveTheme(pub Palette);

impl Global for ActiveTheme {}

/// Draws with `theme`; `System` as the OS's appearance is now (call again when it changes).
pub fn set(theme: Theme, cx: &mut App) {
    let theme = theme.resolve(cx.window_appearance());
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
