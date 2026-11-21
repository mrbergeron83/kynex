//! The window's layout model: each workspace holds one tree of **panes**. Leaves are pane ids
//! (the caller says what each pane shows), splits hold an [`Axis`] and a ratio. Splits are
//! manual: split right or down from a pane, nothing is laid out for you.
//!
//! [`Layout::rects`] turns the tree and the pixels it gets into each pane's rectangle and each
//! divider's: they tile the bounds exactly, and no pane drops under [`MIN_W`] x [`MIN_H`]
//! (a split that wouldn't fit is refused, a ratio is clamped). Ratios are fractions, so
//! resizing one split leaves the proportions inside its sides alone.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

mod spot;
pub use spot::Spot;

/// A pane's id. The caller owns what it means.
pub type PaneId = u64;

/// The least width and height a pane keeps, in pixels.
pub const MIN_W: f32 = 200.0;
pub const MIN_H: f32 = 100.0;
/// The line between two panes, in pixels.
pub const DIVIDER: f32 = 1.0;

const EPS: f32 = 1e-3;

/// How a split lines its two sides up.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Axis {
    /// Side by side: what "split right" makes. The divider is vertical.
    Row,
    /// Stacked: what "split down" makes. The divider is horizontal.
    Column,
}

/// A way to look or move: for focus, swap and resize.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

impl Direction {
    fn axis(self) -> Axis {
        match self {
            Self::Left | Self::Right => Axis::Row,
            Self::Up | Self::Down => Axis::Column,
        }
    }

    /// Towards the right or bottom.
    fn forward(self) -> bool {
        matches!(self, Self::Right | Self::Down)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    fn len(&self, axis: Axis) -> f32 {
        match axis {
            Axis::Row => self.w,
            Axis::Column => self.h,
        }
    }

    fn right(&self) -> f32 {
        self.x + self.w
    }

    fn bottom(&self) -> f32 {
        self.y + self.h
    }
}

/// Which side of a split: `false` is the first (left / top), `true` the second.
pub type Path = Vec<bool>;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Node {
    Pane(PaneId),
    Split { axis: Axis, ratio: f32, a: Box<Node>, b: Box<Node> },
}

impl Node {
    fn leaves(&self, out: &mut Vec<PaneId>) {
        match self {
            Self::Pane(id) => out.push(*id),
            Self::Split { a, b, .. } => {
                a.leaves(out);
                b.leaves(out);
            }
        }
    }

    fn first_leaf(&self) -> PaneId {
        match self {
            Self::Pane(id) => *id,
            Self::Split { a, .. } => a.first_leaf(),
        }
    }

    fn contains(&self, pane: PaneId) -> bool {
        match self {
            Self::Pane(id) => *id == pane,
            Self::Split { a, b, .. } => a.contains(pane) || b.contains(pane),
        }
    }

    /// The least length this subtree needs along `axis`.
    fn min_extent(&self, axis: Axis) -> f32 {
        match self {
            Self::Pane(_) => match axis {
                Axis::Row => MIN_W,
                Axis::Column => MIN_H,
            },
            Self::Split { axis: own, a, b, .. } if *own == axis => a.min_extent(axis) + DIVIDER + b.min_extent(axis),
            Self::Split { a, b, .. } => a.min_extent(axis).max(b.min_extent(axis)),
        }
    }

    fn at(&self, path: &[bool]) -> Option<&Self> {
        match (path.split_first(), self) {
            (None, _) => Some(self),
            (Some((side, rest)), Self::Split { a, b, .. }) => (if *side { b } else { a }).at(rest),
            _ => None,
        }
    }

    fn at_mut(&mut self, path: &[bool]) -> Option<&mut Self> {
        match (path.split_first(), self) {
            (None, node) => Some(node),
            (Some((side, rest)), Self::Split { a, b, .. }) => (if *side { b } else { a }).at_mut(rest),
            _ => None,
        }
    }

    fn split_leaf(&mut self, target: PaneId, axis: Axis, new: PaneId, ratio: f32) -> bool {
        match self {
            Self::Pane(id) if *id == target => {
                *self = Self::Split { axis, ratio, a: Box::new(Self::Pane(target)), b: Box::new(Self::Pane(new)) };
                true
            }
            Self::Pane(_) => false,
            Self::Split { a, b, .. } => {
                a.split_leaf(target, axis, new, ratio) || b.split_leaf(target, axis, new, ratio)
            }
        }
    }

    /// Splits leaf `target` in half with `new` on its `side`.
    fn insert_beside(&mut self, target: PaneId, side: Direction, new: PaneId) -> bool {
        match self {
            Self::Pane(id) if *id == target => {
                let (a, b) = if side.forward() { (target, new) } else { (new, target) };
                let (a, b) = (Box::new(Self::Pane(a)), Box::new(Self::Pane(b)));
                *self = Self::Split { axis: side.axis(), ratio: 0.5, a, b };
                true
            }
            Self::Pane(_) => false,
            Self::Split { a, b, .. } => a.insert_beside(target, side, new) || b.insert_beside(target, side, new),
        }
    }

    /// Takes `pane` out; its sibling takes the space. The pane to focus next, if it was there.
    fn remove(&mut self, pane: PaneId) -> Option<PaneId> {
        let Self::Split { a, b, .. } = self else { return None };
        let a_gone = matches!(**a, Self::Pane(id) if id == pane);
        let b_gone = matches!(**b, Self::Pane(id) if id == pane);
        if a_gone || b_gone {
            let sibling = std::mem::replace(if a_gone { &mut **b } else { &mut **a }, Self::Pane(0));
            let next = sibling.first_leaf();
            *self = sibling;
            return Some(next);
        }
        a.remove(pane).or_else(|| b.remove(pane))
    }

    fn rename(&mut self, from: PaneId, to: PaneId) {
        match self {
            Self::Pane(id) if *id == from => *id = to,
            Self::Pane(_) => {}
            Self::Split { a, b, .. } => {
                a.rename(from, to);
                b.rename(from, to);
            }
        }
    }

    fn renumber(&mut self, f: &mut impl FnMut(PaneId) -> PaneId) {
        match self {
            Self::Pane(id) => *id = f(*id),
            Self::Split { a, b, .. } => {
                a.renumber(f);
                b.renumber(f);
            }
        }
    }

    fn valid(&self) -> bool {
        match self {
            Self::Pane(_) => true,
            Self::Split { ratio, a, b, .. } => ratio.is_finite() && a.valid() && b.valid(),
        }
    }
}

/// One divider between two sides of a split.
#[derive(Clone, Debug, PartialEq)]
pub struct Divider {
    /// The line itself, [`DIVIDER`] thick.
    pub rect: Rect,
    pub axis: Axis,
    /// Which split it belongs to: pass it to [`Layout::set_ratio`].
    pub path: Path,
    /// Everything the split covers, both sides and the line.
    pub area: Rect,
}

/// Where every pane and divider of a workspace sits.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rects {
    pub panes: Vec<(PaneId, Rect)>,
    pub dividers: Vec<Divider>,
}

impl Rects {
    pub fn pane(&self, pane: PaneId) -> Option<Rect> {
        self.panes.iter().find(|(id, _)| *id == pane).map(|(_, r)| *r)
    }
}

/// The two rectangles of a split inside `r`, and the line between them.
fn divide(axis: Axis, ratio: f32, r: Rect, need_a: f32, need_b: f32) -> (Rect, Rect, Rect) {
    let avail = (r.len(axis) - DIVIDER).max(0.0);
    let first = if avail >= need_a + need_b {
        (avail * ratio).clamp(need_a, (avail - need_b).max(need_a))
    } else {
        // Not enough room for the minimums: share what there is by need.
        avail * need_a / (need_a + need_b)
    };
    let second = avail - first;
    match axis {
        Axis::Row => (
            Rect::new(r.x, r.y, first, r.h),
            Rect::new(r.x + first, r.y, DIVIDER, r.h),
            Rect::new(r.x + first + DIVIDER, r.y, second, r.h),
        ),
        Axis::Column => (
            Rect::new(r.x, r.y, r.w, first),
            Rect::new(r.x, r.y + first, r.w, DIVIDER),
            Rect::new(r.x, r.y + first + DIVIDER, r.w, second),
        ),
    }
}

fn place(node: &Node, r: Rect, path: &mut Path, out: &mut Rects) {
    match node {
        Node::Pane(id) => out.panes.push((*id, r)),
        Node::Split { axis, ratio, a, b } => {
            let (ra, line, rb) = divide(*axis, *ratio, r, a.min_extent(*axis), b.min_extent(*axis));
            out.dividers.push(Divider { rect: line, axis: *axis, path: path.clone(), area: r });
            path.push(false);
            place(a, ra, path, out);
            path.pop();
            path.push(true);
            place(b, rb, path, out);
            path.pop();
        }
    }
}

/// A tree of panes, one of them focused.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PaneTree {
    root: Node,
    focus: PaneId,
    /// The pane filling the whole workspace, if zoomed.
    #[serde(default)]
    zoomed: Option<PaneId>,
}

impl PaneTree {
    pub fn new(pane: PaneId) -> Self {
        Self { root: Node::Pane(pane), focus: pane, zoomed: None }
    }

    pub fn root(&self) -> &Node {
        &self.root
    }

    pub fn focus(&self) -> PaneId {
        self.focus
    }

    pub fn zoomed(&self) -> Option<PaneId> {
        self.zoomed
    }

    /// The panes, left to right and top to bottom.
    pub fn panes(&self) -> Vec<PaneId> {
        let mut out = Vec::new();
        self.root.leaves(&mut out);
        out
    }

    pub fn contains(&self, pane: PaneId) -> bool {
        self.root.contains(pane)
    }

    /// Focuses `pane`, leaving zoom if another pane was zoomed.
    pub fn set_focus(&mut self, pane: PaneId) -> bool {
        if !self.contains(pane) {
            return false;
        }
        self.focus = pane;
        if self.zoomed.is_some_and(|z| z != pane) {
            self.zoomed = None;
        }
        true
    }

    /// Splits `target` in two, the new pane on its right (`Row`) or below (`Column`), and
    /// focuses it. Refused (false) when `target` isn't here or the result wouldn't leave
    /// every pane its minimum size inside `bounds`.
    pub fn split(&mut self, target: PaneId, axis: Axis, new: PaneId, bounds: Rect) -> bool {
        self.split_with(target, axis, new, 0.5, bounds)
    }

    /// [`Self::split`] with `ratio` of the room going to `target` rather than half.
    pub fn split_with(&mut self, target: PaneId, axis: Axis, new: PaneId, ratio: f32, bounds: Rect) -> bool {
        if self.contains(new) || !ratio.is_finite() {
            return false;
        }
        let mut root = self.root.clone();
        if !root.split_leaf(target, axis, new, ratio.clamp(0.0, 1.0)) {
            return false;
        }
        if root.min_extent(Axis::Row) > bounds.w + EPS || root.min_extent(Axis::Column) > bounds.h + EPS {
            return false;
        }
        self.root = root;
        self.focus = new;
        self.zoomed = None;
        true
    }

    /// Splits the whole layout in two, `new` after everything there (right for `Row`, below for
    /// `Column`) with `ratio` of the room before it, and focuses it. Refused like [`Self::split`].
    pub fn split_root(&mut self, axis: Axis, new: PaneId, ratio: f32, bounds: Rect) -> bool {
        if self.contains(new) || !ratio.is_finite() {
            return false;
        }
        let old = std::mem::replace(&mut self.root, Node::Pane(new));
        let root = Node::Split { axis, ratio: ratio.clamp(0.0, 1.0), a: Box::new(old), b: Box::new(Node::Pane(new)) };
        if root.min_extent(Axis::Row) > bounds.w + EPS || root.min_extent(Axis::Column) > bounds.h + EPS {
            let Node::Split { a, .. } = root else { unreachable!() };
            self.root = *a;
            return false;
        }
        self.root = root;
        self.focus = new;
        self.zoomed = None;
        true
    }

    /// Closes `pane`: its sibling takes the space. False if it isn't here or is the only pane.
    pub fn close(&mut self, pane: PaneId) -> bool {
        let Some(next) = self.root.remove(pane) else { return false };
        if self.focus == pane {
            self.focus = next;
        }
        if self.zoomed == Some(pane) {
            self.zoomed = None;
        }
        true
    }

    /// Zooms the focused pane to fill the workspace, or puts the others back.
    pub fn toggle_zoom(&mut self) {
        self.zoomed = match self.zoomed {
            Some(_) => None,
            None if self.panes().len() > 1 => Some(self.focus),
            None => None,
        };
    }

    /// Where everything sits in `bounds`. A zoomed pane takes it all, with no dividers.
    pub fn rects(&self, bounds: Rect) -> Rects {
        match self.zoomed {
            Some(pane) => Rects { panes: vec![(pane, bounds)], dividers: Vec::new() },
            None => self.full_rects(bounds),
        }
    }

    /// [`rects`](Self::rects) as if nothing were zoomed.
    fn full_rects(&self, bounds: Rect) -> Rects {
        let mut out = Rects::default();
        place(&self.root, bounds, &mut Vec::new(), &mut out);
        out
    }

    /// The pane next to `from` in `dir`: the nearest one that overlaps it across, most
    /// overlap first.
    pub fn neighbour(&self, from: PaneId, dir: Direction, bounds: Rect) -> Option<PaneId> {
        let rects = self.full_rects(bounds);
        let c = rects.pane(from)?;
        let mut best: Option<(PaneId, f32, f32)> = None;
        for (id, o) in &rects.panes {
            if *id == from {
                continue;
            }
            let (gap, overlap) = match dir {
                Direction::Right => (o.x - c.right(), overlap(c.y, c.bottom(), o.y, o.bottom())),
                Direction::Left => (c.x - o.right(), overlap(c.y, c.bottom(), o.y, o.bottom())),
                Direction::Down => (o.y - c.bottom(), overlap(c.x, c.right(), o.x, o.right())),
                Direction::Up => (c.y - o.bottom(), overlap(c.x, c.right(), o.x, o.right())),
            };
            if gap < -EPS || overlap <= EPS {
                continue;
            }
            let better = best.is_none_or(|(_, g, ov)| gap < g - EPS || (gap < g + EPS && overlap > ov));
            if better {
                best = Some((*id, gap, overlap));
            }
        }
        best.map(|(id, ..)| id)
    }

    /// Moves focus to the neighbour in `dir`.
    pub fn focus_dir(&mut self, dir: Direction, bounds: Rect) -> bool {
        match self.neighbour(self.focus, dir, bounds) {
            Some(id) => self.set_focus(id),
            None => false,
        }
    }

    /// Focuses the next pane in reading order, wrapping round.
    pub fn focus_next(&mut self) {
        self.step_focus(1);
    }

    pub fn focus_prev(&mut self) {
        self.step_focus(-1);
    }

    fn step_focus(&mut self, by: isize) {
        let panes = self.panes();
        let i = panes.iter().position(|p| *p == self.focus).unwrap_or(0) as isize;
        let next = (i + by).rem_euclid(panes.len() as isize) as usize;
        self.set_focus(panes[next]);
    }

    /// Swaps two panes' places. The focus stays on its pane.
    pub fn swap(&mut self, a: PaneId, b: PaneId) -> bool {
        if a == b || !self.contains(a) || !self.contains(b) {
            return false;
        }
        const TMP: PaneId = PaneId::MAX;
        self.root.rename(a, TMP);
        self.root.rename(b, a);
        self.root.rename(TMP, b);
        true
    }

    /// Moves `pane` to the `side` of `target`, which it splits in half (the room `pane`
    /// leaves goes to its sibling), or swaps the two with `side` `None`. The moved pane
    /// is focused, and a zoom ends. Refused (false, nothing changed) when either isn't
    /// here, they're the same, or a pane would get too small for `bounds`.
    pub fn move_pane(&mut self, pane: PaneId, target: PaneId, side: Option<Direction>, bounds: Rect) -> bool {
        if pane == target || !self.contains(pane) || !self.contains(target) {
            return false;
        }
        match side {
            None => {
                self.swap(pane, target);
            }
            Some(side) => {
                let mut root = self.root.clone();
                if root.remove(pane).is_none() || !root.insert_beside(target, side, pane) {
                    return false;
                }
                if root.min_extent(Axis::Row) > bounds.w + EPS || root.min_extent(Axis::Column) > bounds.h + EPS {
                    return false;
                }
                self.root = root;
            }
        }
        self.focus = pane;
        self.zoomed = None;
        true
    }

    /// Moves `pane` past its neighbour in `dir`, to that neighbour's far side ("Move left"
    /// puts it on the left of the pane on its left). False if it has no neighbour that way
    /// or there's no room.
    pub fn move_dir(&mut self, pane: PaneId, dir: Direction, bounds: Rect) -> bool {
        match self.neighbour(pane, dir, bounds) {
            Some(target) => self.move_pane(pane, target, Some(dir), bounds),
            None => false,
        }
    }

    /// Swaps the focused pane with its neighbour in `dir`.
    pub fn swap_dir(&mut self, dir: Direction, bounds: Rect) -> bool {
        match self.neighbour(self.focus, dir, bounds) {
            Some(other) => self.swap(self.focus, other),
            None => false,
        }
    }

    /// Sets one split's ratio (its [`Divider::path`]), clamped so both sides keep their
    /// minimum size inside `bounds`.
    pub fn set_ratio(&mut self, path: &[bool], ratio: f32, bounds: Rect) -> bool {
        let rects = self.full_rects(bounds);
        let Some(div) = rects.dividers.iter().find(|d| d.path == path) else { return false };
        let (axis, avail) = (div.axis, div.area.len(div.axis) - DIVIDER);
        let Some(Node::Split { ratio: slot, a, b, .. }) = self.root.at_mut(path) else { return false };
        let (need_a, need_b) = (a.min_extent(axis), b.min_extent(axis));
        let ratio = if !ratio.is_finite() {
            return false;
        } else if avail >= need_a + need_b {
            let lo = need_a / avail;
            ratio.clamp(lo, (1.0 - need_b / avail).max(lo))
        } else {
            ratio.clamp(0.0, 1.0)
        };
        *slot = ratio;
        true
    }

    /// Moves `pane`'s edge towards `dir` by `px` pixels (negative to pull it back): the
    /// nearest split along that axis with the edge on that side of the pane.
    pub fn resize(&mut self, pane: PaneId, dir: Direction, px: f32, bounds: Rect) -> bool {
        let Some(path) = self.edge_split(pane, dir) else { return false };
        let rects = self.full_rects(bounds);
        let Some(div) = rects.dividers.iter().find(|d| d.path == path) else { return false };
        let avail = div.area.len(div.axis) - DIVIDER;
        let Some(Node::Split { ratio, .. }) = self.root.at(&path) else { return false };
        let signed = if dir.forward() { px } else { -px };
        let target = *ratio + signed / avail.max(1.0);
        self.set_ratio(&path, target, bounds)
    }

    /// The split whose divider is `pane`'s edge on the `dir` side.
    fn edge_split(&self, pane: PaneId, dir: Direction) -> Option<Path> {
        let mut path = Path::new();
        if !self.contains(pane) {
            return None;
        }
        let mut found = None;
        let mut node = &self.root;
        while let Node::Split { axis, a, b, .. } = node {
            let in_b = b.contains(pane);
            // The divider is on the pane's forward side when it is in the first half.
            if *axis == dir.axis() && in_b != dir.forward() {
                found = Some(path.clone());
            }
            path.push(in_b);
            node = if in_b { b } else { a };
        }
        found
    }

    fn renumber(&mut self, f: &mut impl FnMut(PaneId) -> PaneId) {
        self.root.renumber(f);
        self.focus = f(self.focus);
        self.zoomed = self.zoomed.map(&mut *f);
    }

    fn valid(&self) -> bool {
        let panes = self.panes();
        let mut seen = panes.clone();
        seen.sort_unstable();
        seen.dedup();
        seen.len() == panes.len()
            && self.root.valid()
            && self.contains(self.focus)
            && self.zoomed.is_none_or(|z| self.contains(z))
    }
}

fn overlap(a0: f32, a1: f32, b0: f32, b1: f32) -> f32 {
    a1.min(b1) - a0.max(b0)
}

/// What a workspace shows: one tree of split panes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Layout {
    tree: Option<PaneTree>,
}

#[derive(Deserialize)]
struct LegacyTab {
    #[allow(dead_code)]
    #[serde(default)]
    name: String,
    root: Node,
    focus: PaneId,
    #[serde(default)]
    zoomed: Option<PaneId>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EmptyLayout {}

#[derive(Deserialize)]
#[serde(untagged)]
enum LayoutWire {
    Tree(PaneTree),
    LegacyTabs {
        tabs: Vec<LegacyTab>,
        #[serde(default)]
        active: usize,
    },
    Empty(EmptyLayout),
}

impl Serialize for Layout {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Empty {}
        match &self.tree {
            Some(tree) => tree.serialize(serializer),
            None => (Empty {}).serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for Layout {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match LayoutWire::deserialize(deserializer)? {
            LayoutWire::Tree(tree) => Ok(Self { tree: Some(tree) }),
            LayoutWire::Empty(_) => Ok(Self { tree: None }),
            LayoutWire::LegacyTabs { tabs, active } => {
                if tabs.is_empty() {
                    if active == 0 {
                        return Ok(Self { tree: None });
                    }
                    return Err(serde::de::Error::custom("active tab out of range"));
                }
                if active >= tabs.len() {
                    return Err(serde::de::Error::custom("active tab out of range"));
                }
                for tab in &tabs {
                    let check = PaneTree { root: tab.root.clone(), focus: tab.focus, zoomed: tab.zoomed };
                    if !check.valid() {
                        return Err(serde::de::Error::custom("invalid legacy tab"));
                    }
                }
                let focus = tabs[active].focus;
                let zoomed = if tabs.len() == 1 { tabs[active].zoomed } else { None };
                let mut root = tabs[active].root.clone();
                for (i, tab) in tabs.into_iter().enumerate() {
                    if i != active {
                        root = Node::Split { axis: Axis::Row, ratio: 0.5, a: Box::new(root), b: Box::new(tab.root) };
                    }
                }
                Ok(Self { tree: Some(PaneTree { root, focus, zoomed }) })
            }
        }
    }
}

impl Layout {
    /// A workspace layout holding just `pane`.
    pub fn new(pane: PaneId) -> Self {
        Self { tree: Some(PaneTree::new(pane)) }
    }

    pub fn is_empty(&self) -> bool {
        self.tree.is_none()
    }

    pub fn root(&self) -> Option<&Node> {
        self.tree.as_ref().map(PaneTree::root)
    }

    /// Every pane in the workspace, left to right and top to bottom.
    pub fn panes(&self) -> Vec<PaneId> {
        self.tree.as_ref().map(PaneTree::panes).unwrap_or_default()
    }

    pub fn contains(&self, pane: PaneId) -> bool {
        self.tree.as_ref().is_some_and(|t| t.contains(pane))
    }

    /// The panes in sight: all of them, or the zoomed pane alone.
    pub fn visible(&self) -> Vec<PaneId> {
        match &self.tree {
            Some(t) => t.zoomed().map_or_else(|| t.panes(), |z| vec![z]),
            None => Vec::new(),
        }
    }

    pub fn focus(&self) -> Option<PaneId> {
        self.tree.as_ref().map(PaneTree::focus)
    }

    pub fn zoomed(&self) -> Option<PaneId> {
        self.tree.as_ref().and_then(PaneTree::zoomed)
    }

    pub fn set_focus(&mut self, pane: PaneId) -> bool {
        self.tree.as_mut().is_some_and(|t| t.set_focus(pane))
    }

    pub fn toggle_zoom(&mut self) {
        if let Some(t) = &mut self.tree {
            t.toggle_zoom();
        }
    }

    /// Splits `target` (see [`PaneTree::split`]).
    pub fn split(&mut self, target: PaneId, axis: Axis, new: PaneId, bounds: Rect) -> bool {
        self.split_with(target, axis, new, 0.5, bounds)
    }

    /// [`Self::split`] with `ratio` of the room going to `target` (see [`PaneTree::split_with`]).
    pub fn split_with(&mut self, target: PaneId, axis: Axis, new: PaneId, ratio: f32, bounds: Rect) -> bool {
        self.tree.as_mut().is_some_and(|t| t.split_with(target, axis, new, ratio, bounds))
    }

    /// Splits the whole workspace (see [`PaneTree::split_root`]).
    pub fn split_root(&mut self, axis: Axis, new: PaneId, ratio: f32, bounds: Rect) -> bool {
        self.tree.as_mut().is_some_and(|t| t.split_root(axis, new, ratio, bounds))
    }

    /// Closes `pane`; leaves the layout empty if it was the last one. False if it isn't here.
    pub fn close_pane(&mut self, pane: PaneId) -> bool {
        let Some(t) = &mut self.tree else { return false };
        if !t.contains(pane) {
            return false;
        }
        if !t.close(pane) {
            self.tree = None;
        }
        true
    }

    pub fn swap(&mut self, a: PaneId, b: PaneId) -> bool {
        self.tree.as_mut().is_some_and(|t| t.swap(a, b))
    }

    pub fn move_pane(&mut self, pane: PaneId, target: PaneId, side: Option<Direction>, bounds: Rect) -> bool {
        self.tree.as_mut().is_some_and(|t| t.move_pane(pane, target, side, bounds))
    }

    pub fn move_dir(&mut self, pane: PaneId, dir: Direction, bounds: Rect) -> bool {
        self.tree.as_mut().is_some_and(|t| t.move_dir(pane, dir, bounds))
    }

    pub fn focus_dir(&mut self, dir: Direction, bounds: Rect) -> bool {
        self.tree.as_mut().is_some_and(|t| t.focus_dir(dir, bounds))
    }

    pub fn focus_next(&mut self) {
        if let Some(t) = &mut self.tree {
            t.focus_next();
        }
    }

    pub fn focus_prev(&mut self) {
        if let Some(t) = &mut self.tree {
            t.focus_prev();
        }
    }

    pub fn swap_dir(&mut self, dir: Direction, bounds: Rect) -> bool {
        self.tree.as_mut().is_some_and(|t| t.swap_dir(dir, bounds))
    }

    pub fn rects(&self, bounds: Rect) -> Rects {
        self.tree.as_ref().map(|t| t.rects(bounds)).unwrap_or_default()
    }

    pub fn set_ratio(&mut self, path: &[bool], ratio: f32, bounds: Rect) -> bool {
        self.tree.as_mut().is_some_and(|t| t.set_ratio(path, ratio, bounds))
    }

    pub fn resize(&mut self, pane: PaneId, dir: Direction, px: f32, bounds: Rect) -> bool {
        self.tree.as_mut().is_some_and(|t| t.resize(pane, dir, px, bounds))
    }

    pub fn spot(&self, pane: PaneId) -> Option<Spot> {
        self.tree.as_ref().and_then(|t| t.spot(pane))
    }

    pub fn put_back(&mut self, pane: PaneId, spot: &Spot, bounds: Rect) -> bool {
        self.tree.as_mut().is_some_and(|t| t.put_back(pane, spot, bounds))
    }

    /// Reads a saved layout, or `None` if it is broken (a focus that isn't in the tree,
    /// a pane twice).
    pub fn from_json(json: &str) -> Option<Self> {
        let layout: Self = serde_json::from_str(json).ok()?;
        layout.is_valid().then_some(layout)
    }

    /// Whether the layout holds together: no pane twice, focus (and zoom) in the tree.
    pub fn is_valid(&self) -> bool {
        self.tree.as_ref().is_none_or(PaneTree::valid)
    }

    /// Gives every pane the id `f` returns for it (focus and zoom included), for panes
    /// read from a file whose ids may collide with ones in use. `f` must answer the same
    /// for the same id, and different ids must get different answers.
    pub fn renumber(&mut self, mut f: impl FnMut(PaneId) -> PaneId) {
        if let Some(t) = &mut self.tree {
            t.renumber(&mut f);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const B: Rect = Rect { x: 0.0, y: 0.0, w: 1600.0, h: 900.0 };

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 0.01
    }

    /// Three panes: 1 | (2 over 3).
    fn three() -> PaneTree {
        let mut t = PaneTree::new(1);
        assert!(t.split(1, Axis::Row, 2, B));
        assert!(t.split(2, Axis::Column, 3, B));
        t
    }

    #[test]
    fn a_root_split_runs_along_the_whole_layout() {
        let mut t = three();
        assert!(t.split_root(Axis::Column, 4, 0.75, B));
        assert_eq!((t.panes(), t.focus()), (vec![1, 2, 3, 4], 4));
        let r = t.rects(B);
        let bottom = r.pane(4).unwrap();
        assert!(close(bottom.w, B.w) && close(bottom.y, (B.h - DIVIDER) * 0.75 + DIVIDER));
        assert!(!t.split_root(Axis::Column, 4, 0.5, B), "4 is here already");
        let tiny = Rect::new(0.0, 0.0, 1600.0, 250.0);
        assert!(!t.split_root(Axis::Column, 5, 0.5, tiny));
        assert_eq!(t.panes(), vec![1, 2, 3, 4], "a refused split changes nothing");
    }

    #[test]
    fn split_makes_two_panes_and_focuses_the_new_one() {
        let mut t = PaneTree::new(1);
        assert!(t.split(1, Axis::Row, 2, B));
        assert_eq!(t.panes(), vec![1, 2]);
        assert_eq!(t.focus(), 2);
        let r = t.rects(B);
        let (a, b) = (r.pane(1).unwrap(), r.pane(2).unwrap());
        assert!(a.x < b.x && close(a.y, b.y));
        assert_eq!(r.dividers.len(), 1);
    }

    #[test]
    fn split_down_stacks() {
        let mut t = PaneTree::new(1);
        assert!(t.split(1, Axis::Column, 2, B));
        let r = t.rects(B);
        assert!(r.pane(1).unwrap().y < r.pane(2).unwrap().y);
    }

    #[test]
    fn split_refuses_missing_duplicate_and_cramped() {
        let mut t = PaneTree::new(1);
        assert!(!t.split(9, Axis::Row, 2, B));
        assert!(!t.split(1, Axis::Row, 1, B));
        let small = Rect::new(0.0, 0.0, MIN_W * 1.5, 900.0);
        assert!(!t.split(1, Axis::Row, 2, small));
        assert_eq!(t.panes(), vec![1]);
        assert!(t.split(1, Axis::Column, 2, small));
    }

    #[test]
    fn close_gives_the_space_to_the_sibling() {
        let mut t = three();
        assert!(t.close(3));
        assert_eq!(t.panes(), vec![1, 2]);
        assert_eq!(t.focus(), 2, "the focused pane closed, so its sibling is focused");
        let r = t.rects(B);
        assert!(close(r.pane(2).unwrap().h, B.h));
        assert!(!t.close(9));
        assert!(t.close(2));
        assert!(!t.close(1), "the only pane stays");
    }

    #[test]
    fn a_pane_moves_to_each_side_of_another() {
        // three(): [1 | 2 over 3].
        let at = |t: &PaneTree, p| t.rects(B).pane(p).unwrap();
        let mut t = three();
        assert!(t.move_pane(3, 1, Some(Direction::Left), B));
        assert_eq!(t.panes(), vec![3, 1, 2]);
        assert!(at(&t, 3).x < at(&t, 1).x && close(at(&t, 3).h, B.h));
        assert!(close(at(&t, 2).h, B.h), "the room 3 left went to 2");
        let mut t = three();
        assert!(t.move_pane(3, 1, Some(Direction::Right), B));
        assert_eq!(t.panes(), vec![1, 3, 2]);
        let mut t = three();
        assert!(t.move_pane(3, 1, Some(Direction::Up), B));
        assert_eq!(t.panes(), vec![3, 1, 2]);
        assert!(at(&t, 3).y < at(&t, 1).y && close(at(&t, 3).x, at(&t, 1).x));
        let mut t = three();
        assert!(t.move_pane(3, 1, Some(Direction::Down), B));
        assert_eq!(t.panes(), vec![1, 3, 2]);
        assert!(at(&t, 1).y < at(&t, 3).y);
        // Beside its own sibling: the two trade places along the new axis.
        let mut t = three();
        assert!(t.move_pane(3, 2, Some(Direction::Left), B));
        assert_eq!(t.panes(), vec![1, 3, 2]);
        assert!(close(at(&t, 3).y, at(&t, 2).y));
    }

    #[test]
    fn a_pane_dropped_in_the_middle_swaps() {
        let mut t = three();
        let before = t.rects(B);
        assert!(t.move_pane(1, 3, None, B));
        assert_eq!(t.panes(), vec![3, 2, 1]);
        assert_eq!(t.rects(B).pane(1), before.pane(3));
        assert_eq!(t.focus(), 1);
    }

    #[test]
    fn a_move_without_room_is_refused_and_changes_nothing() {
        let mut t = three();
        // Room for [1 | 2 over 3], not for three side by side.
        let narrow = Rect::new(0.0, 0.0, MIN_W * 2.5, 900.0);
        let before = t.clone();
        assert!(!t.move_pane(3, 2, Some(Direction::Right), narrow));
        assert_eq!(t, before);
        assert!(!t.move_pane(3, 3, Some(Direction::Left), B), "not onto itself");
        assert!(!t.move_pane(3, 9, Some(Direction::Left), B), "not onto a pane that isn't here");
        assert_eq!(t, before);
    }

    #[test]
    fn the_moved_pane_has_the_focus_and_a_zoom_ends() {
        let mut t = three();
        t.set_focus(3);
        assert!(t.move_pane(3, 1, Some(Direction::Left), B));
        assert_eq!(t.focus(), 3, "focus follows the focused pane");
        assert!(t.move_pane(2, 3, Some(Direction::Up), B));
        assert_eq!(t.focus(), 2, "the pane moved takes it");
        t.toggle_zoom();
        assert_eq!(t.zoomed(), Some(2));
        assert!(t.move_pane(2, 1, Some(Direction::Right), B));
        assert_eq!((t.zoomed(), t.focus()), (None, 2));
        assert_eq!(t.rects(B).panes.len(), 3, "every pane shows again");
    }

    #[test]
    fn move_dir_goes_past_the_neighbour() {
        let mut t = three();
        assert!(t.move_dir(3, Direction::Left, B));
        assert_eq!(t.panes(), vec![3, 1, 2]);
        assert!(!t.move_dir(3, Direction::Left, B), "nothing on its left now");
        assert!(t.move_dir(3, Direction::Right, B));
        assert_eq!(t.panes(), vec![1, 3, 2]);
    }

    #[test]
    fn a_layout_moves_panes_and_keeps_the_move_saved() {
        let mut l = Layout::new(1);
        assert!(l.split(1, Axis::Row, 2, B));
        assert!(l.move_pane(2, 1, Some(Direction::Left), B));
        assert_eq!(l.panes(), vec![2, 1]);
        assert!(l.move_dir(2, Direction::Right, B));
        let saved = serde_json::to_string(&l).unwrap();
        assert_eq!(Layout::from_json(&saved), Some(l));
    }

    #[test]
    fn closing_another_pane_keeps_focus() {
        let mut t = three();
        t.set_focus(1);
        assert!(t.close(3));
        assert_eq!(t.focus(), 1);
    }

    #[test]
    fn focus_follows_direction() {
        let mut t = three();
        t.set_focus(1);
        assert!(t.focus_dir(Direction::Right, B));
        assert!(t.focus() == 2, "the first overlapping pane to the right");
        assert!(t.focus_dir(Direction::Down, B));
        assert_eq!(t.focus(), 3);
        assert!(!t.focus_dir(Direction::Down, B));
        assert!(t.focus_dir(Direction::Left, B));
        assert_eq!(t.focus(), 1);
        assert!(!t.focus_dir(Direction::Left, B));
        assert!(!t.focus_dir(Direction::Up, B));
    }

    #[test]
    fn focus_cycles() {
        let mut t = three();
        t.set_focus(1);
        t.focus_next();
        assert_eq!(t.focus(), 2);
        t.focus_next();
        t.focus_next();
        assert_eq!(t.focus(), 1);
        t.focus_prev();
        assert_eq!(t.focus(), 3);
    }

    #[test]
    fn swap_exchanges_places() {
        let mut t = three();
        let (r1, r3) = (t.rects(B).pane(1).unwrap(), t.rects(B).pane(3).unwrap());
        assert!(t.swap(1, 3));
        assert_eq!(t.rects(B).pane(3).unwrap(), r1);
        assert_eq!(t.rects(B).pane(1).unwrap(), r3);
        assert!(!t.swap(1, 1));
        assert!(!t.swap(1, 9));
        t.set_focus(1);
        assert!(!t.swap_dir(Direction::Down, B));
        assert!(t.swap_dir(Direction::Left, B));
        assert_eq!(t.rects(B).pane(1).unwrap(), r1);
    }

    #[test]
    fn zoom_fills_the_workspace_and_ends_on_another_focus() {
        let mut t = three();
        t.toggle_zoom();
        let r = t.rects(B);
        assert_eq!(r.panes, vec![(3, B)]);
        assert!(r.dividers.is_empty());
        t.set_focus(3);
        assert_eq!(t.zoomed(), Some(3));
        t.set_focus(1);
        assert_eq!(t.zoomed(), None);
        t.toggle_zoom();
        t.toggle_zoom();
        assert_eq!(t.zoomed(), None);
        assert_eq!(PaneTree::new(1).zoomed(), None);
        let mut one = PaneTree::new(1);
        one.toggle_zoom();
        assert_eq!(one.zoomed(), None, "a lone pane has nothing to zoom out of");
    }

    #[test]
    fn set_ratio_moves_one_split_and_nested_panes_keep_their_proportions() {
        let mut t = three();
        let before = t.rects(B);
        let (h2, h3) = (before.pane(2).unwrap().h, before.pane(3).unwrap().h);
        assert!(t.set_ratio(&[], 0.25, B));
        let after = t.rects(B);
        assert!(close(after.pane(1).unwrap().w, (B.w - DIVIDER) * 0.25));
        assert!(close(after.pane(2).unwrap().h, h2), "the stacked pair is untouched");
        assert!(close(after.pane(3).unwrap().h, h3));
        assert!(!t.set_ratio(&[true, true], 0.5, B), "no split there");
        assert!(!t.set_ratio(&[], f32::NAN, B));
    }

    #[test]
    fn nested_panes_scale_with_their_parent() {
        let mut t = three();
        t.set_ratio(&[true], 0.25, B);
        let small = Rect::new(0.0, 0.0, 800.0, 700.0);
        let r = t.rects(small);
        let (p2, p3) = (r.pane(2).unwrap(), r.pane(3).unwrap());
        assert!((p2.h / (p2.h + p3.h) - 0.25).abs() < 0.01);
    }

    #[test]
    fn ratio_stops_at_the_minimum_size() {
        let mut t = three();
        assert!(t.set_ratio(&[], 0.0, B));
        assert!(close(t.rects(B).pane(1).unwrap().w, MIN_W));
        assert!(t.set_ratio(&[], 1.0, B));
        assert!(close(t.rects(B).pane(2).unwrap().w, MIN_W));
        assert!(t.set_ratio(&[true], 0.0, B));
        assert!(close(t.rects(B).pane(2).unwrap().h, MIN_H));
    }

    #[test]
    fn resize_moves_the_edge_towards_the_direction() {
        let mut t = three();
        let w = t.rects(B).pane(1).unwrap().w;
        assert!(t.resize(1, Direction::Right, 100.0, B));
        assert!(close(t.rects(B).pane(1).unwrap().w, w + 100.0));
        assert!(t.resize(2, Direction::Left, 50.0, B), "pane 2's left edge is the same divider");
        assert!(close(t.rects(B).pane(1).unwrap().w, w + 50.0));
        assert!(!t.resize(1, Direction::Left, 10.0, B), "pane 1 has no edge on its left");
        assert!(!t.resize(1, Direction::Down, 10.0, B));
        assert!(t.resize(2, Direction::Down, 40.0, B));
        let r = t.rects(B);
        assert!(r.pane(2).unwrap().h > r.pane(3).unwrap().h);
    }

    #[test]
    fn rects_survive_a_window_too_small_for_the_minimums() {
        let t = three();
        let tiny = Rect::new(0.0, 0.0, 100.0, 50.0);
        let r = t.rects(tiny);
        assert_eq!(r.panes.len(), 3);
        assert!(r.panes.iter().all(|(_, p)| p.w >= 0.0 && p.h >= 0.0));
    }

    #[test]
    fn serde_round_trips_and_rejects_broken_layouts() {
        let mut l = Layout::new(1);
        l.split(1, Axis::Row, 2, B);
        l.split(2, Axis::Column, 3, B);
        let json = serde_json::to_string(&l).unwrap();
        assert_eq!(Layout::from_json(&json), Some(l));
        assert_eq!(Layout::from_json("{"), None);
        let twice = json.replace("\"Pane\":3", "\"Pane\":1").replace("\"focus\":3", "\"focus\":1");
        assert_eq!(Layout::from_json(&twice), None);
        assert_eq!(Layout::from_json(r#"{"tabs":[],"active":3}"#), None);
        assert!(Layout::from_json(r#"{"tabs":[],"active":0}"#).is_some());
        assert!(Layout::from_json("{}").is_some());
    }

    #[test]
    fn legacy_multi_tab_json_merges_all_panes_into_one_tree() {
        let legacy = r#"{
            "tabs":[
                {"name":"1","root":{"Split":{"axis":"Row","ratio":0.5,"a":{"Pane":1},"b":{"Pane":2}}},"focus":2,"zoomed":null},
                {"name":"2","root":{"Pane":3},"focus":3,"zoomed":null}
            ],
            "active":0
        }"#;
        let l = Layout::from_json(legacy).expect("legacy multi-tab layout loads");
        assert_eq!(l.panes(), vec![1, 2, 3]);
        assert_eq!(l.focus(), Some(2));
        assert!(l.is_valid());
    }

    #[test]
    fn renumber_rewrites_panes_focus_and_zoom() {
        let mut l = Layout::new(1);
        l.split(1, Axis::Row, 2, B);
        l.split(2, Axis::Column, 3, B);
        l.toggle_zoom();
        l.renumber(|p| p + 100);
        assert_eq!(l.panes(), vec![101, 102, 103]);
        assert_eq!((l.focus(), l.zoomed()), (Some(103), Some(103)));
        assert!(l.is_valid());
    }

    #[test]
    fn a_pane_drops_out_of_a_loaded_layout() {
        let mut l = Layout::new(1);
        l.split(1, Axis::Row, 2, B);
        assert!(l.close_pane(2));
        assert_eq!(l.panes(), vec![1]);
        assert!(l.is_valid());
        assert!(l.close_pane(1));
        assert!(l.is_empty());
    }

    /// A tiny deterministic generator, so the property test needs no dependency.
    struct Lcg(u64);

    impl Lcg {
        fn next(&mut self, n: usize) -> usize {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((self.0 >> 33) as usize) % n
        }
    }

    fn assert_tiles(r: &Rects, bounds: Rect) {
        let mut all: Vec<Rect> = r.panes.iter().map(|(_, p)| *p).collect();
        all.extend(r.dividers.iter().map(|d| d.rect));
        let area: f32 = all.iter().map(|p| p.w * p.h).sum();
        assert!((area - bounds.w * bounds.h).abs() < bounds.w * bounds.h * 1e-4, "area {area} vs {bounds:?}");
        for (i, p) in all.iter().enumerate() {
            assert!(p.x >= bounds.x - EPS && p.y >= bounds.y - EPS, "{p:?} outside");
            assert!(p.right() <= bounds.right() + EPS && p.bottom() <= bounds.bottom() + EPS, "{p:?} outside");
            for q in &all[i + 1..] {
                let (ox, oy) = (overlap(p.x, p.right(), q.x, q.right()), overlap(p.y, p.bottom(), q.y, q.bottom()));
                assert!(ox <= EPS || oy <= EPS, "{p:?} overlaps {q:?}");
            }
        }
        for (id, p) in &r.panes {
            assert!(p.w >= MIN_W - EPS && p.h >= MIN_H - EPS, "pane {id} is {p:?}");
        }
    }

    #[test]
    fn rects_tile_the_bounds_with_no_pane_under_the_minimum() {
        let mut rng = Lcg(7);
        for round in 0..60 {
            let bounds = Rect::new(
                rng.next(50) as f32,
                rng.next(50) as f32,
                (MIN_W as usize + rng.next(1800)) as f32,
                (MIN_H as usize + rng.next(1000)) as f32,
            );
            let mut t = PaneTree::new(0);
            let mut next = 1;
            for _ in 0..40 {
                let panes = t.panes();
                let pick = panes[rng.next(panes.len())];
                match rng.next(6) {
                    0..=2 => {
                        let axis = if rng.next(2) == 0 { Axis::Row } else { Axis::Column };
                        if t.split(pick, axis, next, bounds) {
                            next += 1;
                        }
                    }
                    3 => {
                        t.close(pick);
                    }
                    4 => {
                        let dir = [Direction::Left, Direction::Right, Direction::Up, Direction::Down][rng.next(4)];
                        t.resize(pick, dir, rng.next(400) as f32 - 100.0, bounds);
                    }
                    _ => {
                        let divs = t.rects(bounds).dividers;
                        if !divs.is_empty() {
                            let d = &divs[rng.next(divs.len())];
                            t.set_ratio(&d.path.clone(), rng.next(1001) as f32 / 1000.0, bounds);
                        }
                    }
                }
                let r = t.rects(bounds);
                assert_eq!(r.panes.len(), t.panes().len(), "round {round}");
                assert_eq!(r.dividers.len() + 1, r.panes.len(), "round {round}");
                assert_tiles(&r, bounds);
            }
        }
    }
}
