//! Find-in-transcript matches marked in [`Selectable`](super::selectable)
//! text: every match washed, the current one stronger. Like the selection, the
//! marks live in a global, so the blocks need no state of their own.
//!
//! Which match is current is counted per item: each block notes how many
//! matches it holds as it lays out, so a block knows how many come before it.

use std::collections::BTreeMap;
use std::ops::Range;

use gpui::{App, Hsla, SharedString};
use kynex_core::find::occurrences;

use super::selectable::Spot;

#[derive(Default)]
struct Marks {
    group: u64,
    query: SharedString,
    /// The current match: its item and which match in that item it is.
    current: Option<(usize, usize)>,
    color: Hsla,
    current_color: Hsla,
    /// How many matches each block of the group held when last laid out.
    counts: BTreeMap<Spot, usize>,
}

impl gpui::Global for Marks {}

/// Marks `query` in `group`'s blocks, `current` (item, nth in item) stronger.
pub fn set(group: u64, query: &str, current: Option<(usize, usize)>, colors: (Hsla, Hsla), cx: &mut App) {
    let m = cx.default_global::<Marks>();
    if m.group != group || m.query != query {
        m.counts.clear();
    }
    m.group = group;
    m.query = SharedString::from(query.to_string());
    m.current = current;
    (m.color, m.current_color) = colors;
}

/// Marks nothing.
pub fn clear(cx: &mut App) {
    if cx.try_global::<Marks>().is_some_and(|m| !m.query.is_empty()) {
        let m = cx.default_global::<Marks>();
        m.query = SharedString::default();
        m.counts.clear();
    }
}

/// The matches in one block to wash, with their colours; notes its count.
pub(super) fn in_block(group: u64, spot: Spot, text: &str, cx: &mut App) -> Vec<(Range<usize>, Hsla)> {
    let Some(m) = cx.try_global::<Marks>().filter(|m| m.group == group && !m.query.is_empty()) else {
        return Vec::new();
    };
    let found = occurrences(text, &m.query);
    // Matches in this item's earlier blocks, as they were last laid out.
    let before: usize = m.counts.range(Spot::new(spot.item, 0)..spot).map(|(_, n)| n).sum();
    let current = m.current.filter(|(item, _)| *item == spot.item).map(|(_, nth)| nth);
    let (color, strong) = (m.color, m.current_color);
    let marks =
        found.iter().enumerate().map(|(i, r)| (r.clone(), if current == Some(before + i) { strong } else { color }));
    let marks = marks.collect();
    let n = found.len();
    let m = cx.default_global::<Marks>();
    if m.counts.get(&spot) != Some(&n) {
        m.counts.insert(spot, n);
    }
    marks
}
