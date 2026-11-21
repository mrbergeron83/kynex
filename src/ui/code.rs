//! Highlighted code: `kynex_core::highlight` tokens as gpui text styles.

use std::ops::Range;

use gpui::HighlightStyle;
use kynex_core::highlight::Token;

use crate::theme::Syntax;

/// Tokens as styles for `StyledText::with_highlights`.
pub fn styles(tokens: &[(Range<usize>, Token)], syntax: &Syntax) -> Vec<(Range<usize>, HighlightStyle)> {
    tokens.iter().map(|(r, t)| (r.clone(), syntax.style(*t))).collect()
}
