//! How wide the transcript's column is: the Transcript width setting, and
//! the widths of prose and of tool rows it gives in the room there is.
//!
//! Prose keeps a readable measure, centred. Tool rows (diffs, output) start
//! where the prose starts and may grow into the margin on the right, so
//! everything lines up on the left and wide code needn't scroll.

use serde::{Deserialize, Serialize};

/// The Transcript width setting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ColumnWidth {
    /// Prose at about 100 characters a line; tool rows a little wider.
    #[default]
    Readable,
    /// More of both, for big screens.
    Wide,
    /// The whole width of the window.
    Full,
}

/// Widths, in pixels, for one transcript row.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Columns {
    /// Messages and the composer, centred.
    pub prose: f32,
    /// Tool rows, from the prose's left edge.
    pub tools: f32,
}

impl ColumnWidth {
    pub const ALL: [ColumnWidth; 3] = [ColumnWidth::Readable, ColumnWidth::Wide, ColumnWidth::Full];

    pub fn label(self) -> &'static str {
        match self {
            ColumnWidth::Readable => "Readable",
            ColumnWidth::Wide => "Wide",
            ColumnWidth::Full => "Full width",
        }
    }

    pub fn help(self) -> &'static str {
        match self {
            ColumnWidth::Readable => "Text 820 px; tool output up to 1040 px",
            ColumnWidth::Wide => "Text 1040 px; tool output up to 1400 px",
            ColumnWidth::Full => "Text and tool output fill the window",
        }
    }

    /// The most the prose may take; `None` is all there is.
    pub fn max_prose(self) -> Option<f32> {
        self.targets().map(|(prose, _)| prose)
    }

    fn targets(self) -> Option<(f32, f32)> {
        match self {
            ColumnWidth::Readable => Some((820., 1040.)),
            ColumnWidth::Wide => Some((1040., 1400.)),
            ColumnWidth::Full => None,
        }
    }

    /// The widths in `available` pixels. Prose is centred, so tool rows get
    /// at most the margin on its right on top of its width.
    pub fn columns(self, available: f32) -> Columns {
        let available = available.max(0.);
        let Some((prose, tools)) = self.targets() else {
            return Columns { prose: available, tools: available };
        };
        let prose = prose.min(available);
        let right_margin = (available - prose) / 2.;
        Columns { prose, tools: tools.min(prose + right_margin).max(prose) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn narrow_windows_get_one_column() {
        let c = ColumnWidth::Readable.columns(700.);
        assert_eq!(c, Columns { prose: 700., tools: 700. });
    }

    #[test]
    fn tools_grow_into_the_right_margin_only() {
        // 1000 px: prose 820, 90 px margin each side.
        assert_eq!(ColumnWidth::Readable.columns(1000.), Columns { prose: 820., tools: 910. });
        // Plenty of room: capped.
        assert_eq!(ColumnWidth::Readable.columns(2400.), Columns { prose: 820., tools: 1040. });
        assert_eq!(ColumnWidth::Wide.columns(3600.), Columns { prose: 1040., tools: 1400. });
    }

    #[test]
    fn full_fills_and_nothing_goes_negative() {
        assert_eq!(ColumnWidth::Full.columns(3000.), Columns { prose: 3000., tools: 3000. });
        assert_eq!(ColumnWidth::Readable.columns(-5.), Columns { prose: 0., tools: 0. });
        assert_eq!(ColumnWidth::Full.max_prose(), None);
    }

    #[test]
    fn round_trips_through_json() {
        for w in ColumnWidth::ALL {
            let s = serde_json::to_string(&w).unwrap();
            assert_eq!(serde_json::from_str::<ColumnWidth>(&s).unwrap(), w);
        }
    }
}
