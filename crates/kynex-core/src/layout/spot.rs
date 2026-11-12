//! Where a pane sat in its tab, so a pane closed and opened again (the Git pane's toggle)
//! comes back to the same place at the same size, and the panes around it get theirs back.

use serde::{Deserialize, Serialize};

use super::{Axis, Direction, EPS, Node, PaneId, Rect, Tab};

/// A pane's place: the split it was one side of, and the panes on the other side.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Spot {
    pub axis: Axis,
    /// It was the second side (right or below).
    pub after: bool,
    /// Its share of the split, 0 to 1.
    pub share: f32,
    /// The panes on the other side of the split, left to right and top to bottom.
    pub sibling: Vec<PaneId>,
}

impl Spot {
    /// The side of its neighbour it was on.
    pub fn side(&self) -> Direction {
        match (self.axis, self.after) {
            (Axis::Row, true) => Direction::Right,
            (Axis::Row, false) => Direction::Left,
            (Axis::Column, true) => Direction::Down,
            (Axis::Column, false) => Direction::Up,
        }
    }

    /// The ratio a split with the pane on [`Self::side`] of a neighbour gets: the first
    /// side's share.
    pub fn ratio(&self) -> f32 {
        if self.after { 1.0 - self.share } else { self.share }
    }

    /// The same spot with its sibling panes renamed by `f`; `None` if one has no new name.
    pub fn renamed(&self, f: impl Fn(PaneId) -> Option<PaneId>) -> Option<Self> {
        let sibling = self.sibling.iter().map(|p| f(*p)).collect::<Option<Vec<_>>>()?;
        Some(Self { sibling, ..self.clone() })
    }
}

impl Node {
    /// The split holding `pane` as one of its two sides, and which side.
    fn parent_of(&self, pane: PaneId) -> Option<(&Self, bool)> {
        let Self::Split { a, b, .. } = self else { return None };
        match (&**a, &**b) {
            (Self::Pane(p), _) if *p == pane => Some((self, false)),
            (_, Self::Pane(p)) if *p == pane => Some((self, true)),
            _ => a.parent_of(pane).or_else(|| b.parent_of(pane)),
        }
    }

    /// The subtree whose panes are exactly `panes`, in that order.
    fn holding_mut(&mut self, panes: &[PaneId]) -> Option<&mut Self> {
        let mut leaves = Vec::new();
        self.leaves(&mut leaves);
        if leaves == panes {
            return Some(self);
        }
        match self {
            Self::Pane(_) => None,
            Self::Split { a, b, .. } => a.holding_mut(panes).or_else(|| b.holding_mut(panes)),
        }
    }
}

impl Tab {
    /// Where `pane` sits; `None` if it isn't here or is alone in the tab.
    pub fn spot(&self, pane: PaneId) -> Option<Spot> {
        let (split, after) = self.root.parent_of(pane)?;
        let Node::Split { axis, ratio, a, b } = split else { return None };
        let mut sibling = Vec::new();
        (if after { a } else { b }).leaves(&mut sibling);
        Some(Spot { axis: *axis, after, share: if after { 1.0 - ratio } else { *ratio }, sibling })
    }

    /// Puts the new pane `pane` back at `spot`, beside the part of the tree holding exactly
    /// the panes it had beside it, and focuses it. False if those panes aren't together any
    /// more, or there is no room for it in `bounds`.
    pub fn put_back(&mut self, pane: PaneId, spot: &Spot, bounds: Rect) -> bool {
        if self.contains(pane) || spot.sibling.is_empty() || !spot.share.is_finite() {
            return false;
        }
        let mut root = self.root.clone();
        let Some(node) = root.holding_mut(&spot.sibling) else { return false };
        let old = std::mem::replace(node, Node::Pane(pane));
        let new = Box::new(Node::Pane(pane));
        let (a, b) = if spot.after { (Box::new(old), new) } else { (new, Box::new(old)) };
        *node = Node::Split { axis: spot.axis, ratio: spot.ratio().clamp(0.0, 1.0), a, b };
        if root.min_extent(Axis::Row) > bounds.w + EPS || root.min_extent(Axis::Column) > bounds.h + EPS {
            return false;
        }
        self.root = root;
        self.focus = pane;
        self.zoomed = None;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BIG: Rect = Rect { x: 0.0, y: 0.0, w: 2000.0, h: 1000.0 };

    /// Sessions | (session over terminal) | git.
    fn tab() -> Tab {
        let mut t = Tab::new("1", 1);
        assert!(t.split_with(1, Axis::Row, 2, 0.2, BIG));
        assert!(t.split(2, Axis::Column, 3, BIG));
        assert!(t.split_with(2, Axis::Row, 4, 0.6, BIG));
        t
    }

    #[test]
    fn a_pane_closed_and_put_back_leaves_every_size_as_it_was() {
        let mut t = tab();
        let before = t.rects(BIG);
        let spot = t.spot(4).unwrap();
        assert_eq!(spot.side(), Direction::Right);
        assert_eq!(spot.sibling, [2]);
        assert!((spot.share - 0.4).abs() < 1e-6);
        assert!(t.close(4));
        assert!(t.put_back(4, &spot, BIG));
        assert_eq!(t.rects(BIG), before);
        assert_eq!(t.focus(), 4);
    }

    #[test]
    fn a_pane_beside_a_group_goes_back_beside_the_group() {
        let mut t = Tab::new("1", 1);
        assert!(t.split(1, Axis::Row, 2, BIG));
        assert!(t.split_with(1, Axis::Row, 3, 0.3, BIG));
        // (1 | 3) | 2: 2 is beside the pair.
        let before = t.rects(BIG);
        let spot = t.spot(2).unwrap();
        assert_eq!(spot.sibling, [1, 3]);
        t.close(2);
        assert!(t.put_back(2, &spot, BIG));
        assert_eq!(t.rects(BIG), before);
    }

    #[test]
    fn a_first_side_comes_back_first() {
        let mut t = Tab::new("1", 1);
        assert!(t.split_with(1, Axis::Column, 2, 0.25, BIG));
        let spot = t.spot(1).unwrap();
        assert_eq!(spot.side(), Direction::Up);
        assert!((spot.ratio() - 0.25).abs() < 1e-6);
        t.close(1);
        assert!(t.put_back(1, &spot, BIG));
        assert_eq!(t.panes(), [1, 2]);
    }

    #[test]
    fn panes_no_longer_together_refuse_the_spot() {
        let mut t = tab();
        let spot = Spot { axis: Axis::Row, after: true, share: 0.4, sibling: vec![1, 3] };
        assert!(!t.put_back(9, &spot, BIG));
        assert!(!t.contains(9));
        // Nor a pane already here, nor no room.
        let spot = t.spot(4).unwrap();
        assert!(!t.put_back(4, &spot, BIG));
        let small = Rect { w: 500.0, ..BIG };
        t.close(4);
        assert!(!t.put_back(4, &spot, small));
    }

    #[test]
    fn alone_or_absent_has_no_spot() {
        assert!(Tab::new("1", 1).spot(1).is_none());
        assert!(tab().spot(42).is_none());
    }

    #[test]
    fn renaming_maps_every_sibling_or_none() {
        let spot = Spot { axis: Axis::Row, after: true, share: 0.5, sibling: vec![1, 2] };
        assert_eq!(spot.renamed(|p| Some(p + 10)).unwrap().sibling, [11, 12]);
        assert!(spot.renamed(|p| (p == 1).then_some(5)).is_none());
    }
}
