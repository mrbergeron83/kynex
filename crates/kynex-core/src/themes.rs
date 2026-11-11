//! Themes from JSON files in `<data dir>/themes/`. A file names a built-in theme to start
//! from and overrides what it wants:
//!
//! ```json
//! {
//!   "name": "Midnight",
//!   "base": "kynex_dark",
//!   "colours": { "panel": "#101820", "accent": "#ff8800" },
//!   "syntax": { "keyword": "#ff8800" },
//!   "tile": { "gap": 8, "border": 2, "radius": 12, "active": "#ff8800", "inactive": "#333333" }
//! }
//! ```
//!
//! Colours are `#rrggbb` or `#rrggbbaa`. This module reads and checks the files; the app
//! turns a [`Custom`] into a palette. An invalid file is an error in words and is left out.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::tile_style::{MAX_BORDER, MAX_GAP, MAX_RADIUS};

/// The palette colours a file may set (the app's `Palette` fields of those names).
pub const PALETTE_KEYS: [&str; 23] = [
    "title_bar",
    "panel",
    "editor",
    "elevated",
    "border",
    "border_variant",
    "border_focused",
    "text",
    "text_muted",
    "text_placeholder",
    "accent",
    "on_accent",
    "hover",
    "active",
    "element",
    "selection",
    "text_selection",
    "success",
    "warning",
    "error",
    "info",
    "diff_add",
    "diff_del",
];

/// The syntax colours a file may set.
pub const SYNTAX_KEYS: [&str; 7] = ["keyword", "string", "type", "number", "comment", "function", "punctuation"];

/// The built-in themes a file can start from, by the name it gives (the first is the default).
pub const BASES: [&str; 5] = ["kynex_dark", "kynex_light", "one_dark", "catppuccin_mocha", "vscode_dark"];

/// A colour: `rgb` as 0xRRGGBB and its opacity from 0 to 1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Colour {
    pub rgb: u32,
    pub alpha: f32,
}

/// The tile look a theme sets; `None` leaves what the base has.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tile {
    pub gap: Option<f32>,
    pub border: Option<f32>,
    pub radius: Option<f32>,
    /// The border of the focused tile and of the others.
    pub active: Option<Colour>,
    pub inactive: Option<Colour>,
}

/// A theme read from a file.
#[derive(Clone, Debug, PartialEq)]
pub struct Custom {
    /// The file's name without `.json`: what Settings remembers it by.
    pub id: String,
    pub name: String,
    /// One of [`BASES`].
    pub base: String,
    pub colours: Vec<(String, Colour)>,
    pub syntax: Vec<(String, Colour)>,
    pub tile: Tile,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    name: Option<String>,
    base: Option<String>,
    #[serde(default)]
    colours: BTreeMap<String, String>,
    #[serde(default)]
    syntax: BTreeMap<String, String>,
    #[serde(default)]
    tile: TileFile,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct TileFile {
    gap: Option<f32>,
    border: Option<f32>,
    radius: Option<f32>,
    active: Option<String>,
    inactive: Option<String>,
}

/// A colour written `#rrggbb` or `#rrggbbaa` (the `#` is optional).
pub fn parse_colour(text: &str) -> Result<Colour, String> {
    let hex = text.trim().trim_start_matches('#');
    if !hex.is_ascii() || !(hex.len() == 6 || hex.len() == 8) {
        return Err(format!("`{text}` isn't a colour like #rrggbb or #rrggbbaa"));
    }
    let n = u32::from_str_radix(hex, 16).map_err(|_| format!("`{text}` isn't a colour like #rrggbb or #rrggbbaa"))?;
    Ok(if hex.len() == 6 {
        Colour { rgb: n, alpha: 1.0 }
    } else {
        Colour { rgb: n >> 8, alpha: (n & 0xff) as f32 / 255.0 }
    })
}

fn number(name: &str, value: Option<f32>, max: f32) -> Result<Option<f32>, String> {
    match value {
        Some(v) if !v.is_finite() || v < 0.0 => Err(format!("tile {name} must be a number from 0 to {max}")),
        Some(v) => Ok(Some(v.min(max))),
        None => Ok(None),
    }
}

fn colours(section: &str, keys: &[&str], map: BTreeMap<String, String>) -> Result<Vec<(String, Colour)>, String> {
    map.into_iter()
        .map(|(k, v)| {
            if !keys.contains(&k.as_str()) {
                return Err(format!("`{k}` isn't a {section} colour (these are: {})", keys.join(", ")));
            }
            Ok((k.clone(), parse_colour(&v).map_err(|e| format!("{section} {k}: {e}"))?))
        })
        .collect()
}

/// A theme from the text of the file `id.json`. `Err` says what is wrong, in words.
pub fn parse(id: &str, text: &str) -> Result<Custom, String> {
    let f: File = serde_json::from_str(text).map_err(|e| format!("not a theme: {e}"))?;
    let base = f.base.unwrap_or_else(|| BASES[0].to_string());
    if !BASES.contains(&base.as_str()) {
        return Err(format!("`{base}` isn't a base theme (these are: {})", BASES.join(", ")));
    }
    let colour = |what: &str, c: Option<String>| {
        c.map(|c| parse_colour(&c).map_err(|e| format!("tile {what}: {e}"))).transpose()
    };
    let tile = Tile {
        gap: number("gap", f.tile.gap, MAX_GAP)?,
        border: number("border", f.tile.border, MAX_BORDER)?,
        radius: number("radius", f.tile.radius, MAX_RADIUS)?,
        active: colour("active", f.tile.active)?,
        inactive: colour("inactive", f.tile.inactive)?,
    };
    Ok(Custom {
        id: id.to_string(),
        name: f.name.filter(|n| !n.trim().is_empty()).unwrap_or_else(|| id.to_string()),
        base,
        colours: colours("palette", &PALETTE_KEYS, f.colours)?,
        syntax: colours("syntax", &SYNTAX_KEYS, f.syntax)?,
        tile,
    })
}

/// Where the theme files live.
pub fn dir() -> PathBuf {
    crate::paths::data_dir().join("themes")
}

/// What reading the folder found.
#[derive(Debug, Default, PartialEq)]
pub struct Loaded {
    /// The valid themes, by file name.
    pub themes: Vec<Custom>,
    /// One message per file that couldn't be used, naming the file.
    pub errors: Vec<String>,
}

/// Reads every `*.json` in `dir`, sorted by name. Blocks on the disk. A folder that isn't there
/// has no themes.
pub fn load_all(dir: &Path) -> Loaded {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("json")) && p.is_file())
        .collect();
    files.sort();
    let mut out = Loaded::default();
    for path in files {
        let id = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let file = path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        match std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|t| parse(&id, &t)) {
            Ok(t) => out.themes.push(t),
            Err(e) => out.errors.push(format!("{file}: {e}")),
        }
    }
    out
}

/// Watches the themes folder (made if it isn't there); `on_change` rings, on a thread of its
/// own, after a file was added, changed or removed.
pub fn watch(on_change: impl Fn() + Send + 'static) -> Result<crate::watch::Watch, String> {
    let dir = dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("Couldn't make {}: {e}", dir.display()))?;
    crate::watch::watch(&dir, on_change)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_are_hex_with_optional_alpha() {
        assert_eq!(parse_colour("#ff8800"), Ok(Colour { rgb: 0xff8800, alpha: 1.0 }));
        assert_eq!(parse_colour("ff880080").map(|c| (c.rgb, (c.alpha * 255.0).round() as u32)), Ok((0xff8800, 128)));
        for bad in ["", "#fff", "#gggggg", "#ff88001", "red", "#éééé"] {
            assert!(parse_colour(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_theme_overrides_a_base() {
        let t = parse(
            "mid",
            r##"{"name":"Midnight","base":"one_dark","colours":{"panel":"#101820"},
                "syntax":{"keyword":"#ff8800"},"tile":{"gap":8,"radius":99,"active":"#ff8800"}}"##,
        )
        .unwrap();
        assert_eq!((t.id.as_str(), t.name.as_str(), t.base.as_str()), ("mid", "Midnight", "one_dark"));
        assert_eq!(t.colours, [("panel".to_string(), Colour { rgb: 0x101820, alpha: 1.0 })]);
        assert_eq!(t.syntax.len(), 1);
        assert_eq!(t.tile.gap, Some(8.0));
        assert_eq!(t.tile.radius, Some(MAX_RADIUS), "clamped");
        assert_eq!(t.tile.border, None);
        assert!(t.tile.active.is_some() && t.tile.inactive.is_none());
    }

    #[test]
    fn a_bare_file_is_kynex_dark_named_after_the_file() {
        let t = parse("plain", "{}").unwrap();
        assert_eq!((t.name.as_str(), t.base.as_str()), ("plain", "kynex_dark"));
    }

    #[test]
    fn mistakes_are_said_in_words() {
        let err = |text: &str| parse("x", text).unwrap_err();
        assert!(err("nope").contains("not a theme"));
        assert!(err(r#"{"base":"solarized"}"#).contains("isn't a base theme"));
        assert!(err(r##"{"colours":{"pane":"#000000"}}"##).contains("isn't a palette colour"));
        assert!(err(r#"{"colours":{"panel":"blue"}}"#).contains("palette panel"));
        assert!(err(r##"{"syntax":{"kw":"#000000"}}"##).contains("isn't a syntax colour"));
        assert!(err(r#"{"tile":{"gap":-1}}"#).contains("tile gap"));
        assert!(err(r#"{"tiles":{}}"#).contains("not a theme"));
    }

    #[test]
    fn a_folder_gives_its_good_themes_and_names_the_bad_files() {
        let dir = std::env::temp_dir().join(format!("kynex-themes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("b.json"), r#"{"name":"B"}"#).unwrap();
        std::fs::write(dir.join("a.json"), "{}").unwrap();
        std::fs::write(dir.join("bad.json"), r#"{"base":"x"}"#).unwrap();
        std::fs::write(dir.join("notes.txt"), "ignored").unwrap();
        let loaded = load_all(&dir);
        assert_eq!(loaded.themes.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), ["a", "b"]);
        assert_eq!(loaded.errors.len(), 1);
        assert!(loaded.errors[0].starts_with("bad.json: "));
        assert_eq!(load_all(&dir.join("missing")), Loaded::default());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
