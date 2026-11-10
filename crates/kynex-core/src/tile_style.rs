//! How the pane tiles look: the gap between them and around the outside, the border width
//! and the corner radius. A theme gives the defaults ([`TileStyle`]); the Appearance
//! settings change them ([`TileTweaks`]), and `resolve` puts the two together. All in
//! logical pixels.

use serde::{Deserialize, Serialize};

/// Widest gap, border and radius the settings allow.
pub const MAX_GAP: f32 = 24.0;
pub const MAX_BORDER: f32 = 4.0;
pub const MAX_RADIUS: f32 = 20.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TileStyle {
    /// Between tiles and between a tile and the window's edge. 0 draws flush panes with
    /// 1px dividers.
    pub gap: f32,
    /// A tile's border; the focused one's is a pixel thicker.
    pub border: f32,
    pub radius: f32,
}

impl Default for TileStyle {
    fn default() -> Self {
        Self { gap: 6.0, border: 1.0, radius: 6.0 }
    }
}

/// What the settings override of the theme's [`TileStyle`]; `None` follows the theme.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TileTweaks {
    pub gap: Option<f32>,
    pub border: Option<f32>,
    pub radius: Option<f32>,
}

impl TileTweaks {
    /// The theme's style with the settings on top, each kept in range and whole.
    pub fn resolve(&self, theme: TileStyle) -> TileStyle {
        let pick = |own: Option<f32>, base: f32, max: f32| {
            let v = own.filter(|v| v.is_finite()).unwrap_or(base);
            v.clamp(0.0, max).round()
        };
        TileStyle {
            gap: pick(self.gap, theme.gap, MAX_GAP),
            border: pick(self.border, theme.border, MAX_BORDER),
            radius: pick(self.radius, theme.radius, MAX_RADIUS),
        }
    }

    /// Nothing is overridden.
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_theme_applies_until_a_setting_overrides_it() {
        let theme = TileStyle { gap: 10.0, border: 2.0, radius: 10.0 };
        assert_eq!(TileTweaks::default().resolve(theme), theme);
        let tweaks = TileTweaks { gap: Some(0.0), ..TileTweaks::default() };
        assert_eq!(tweaks.resolve(theme), TileStyle { gap: 0.0, border: 2.0, radius: 10.0 });
    }

    #[test]
    fn values_are_kept_in_range_and_whole() {
        let tweaks = TileTweaks { gap: Some(99.0), border: Some(-3.0), radius: Some(7.6) };
        let s = tweaks.resolve(TileStyle::default());
        assert_eq!((s.gap, s.border, s.radius), (MAX_GAP, 0.0, 8.0));
        let junk = TileTweaks { gap: Some(f32::NAN), ..TileTweaks::default() };
        assert_eq!(junk.resolve(TileStyle::default()).gap, TileStyle::default().gap);
    }

    #[test]
    fn saved_settings_without_tweaks_follow_the_theme() {
        let t: TileTweaks = serde_json::from_str("{}").unwrap();
        assert!(t.is_default());
        let t: TileTweaks = serde_json::from_str(r#"{"gap":4.0}"#).unwrap();
        assert_eq!(t.gap, Some(4.0));
    }
}
