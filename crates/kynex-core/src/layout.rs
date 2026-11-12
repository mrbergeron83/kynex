//! The window's layout model, after herdr: a workspace holds **tabs**, and each tab holds
//! one tree of **panes**. Leaves are pane ids (the caller says what a pane shows, say a
//! session [`Key`](crate::live::Key); ids must be unique across the workspaces it moves
//! panes between), splits hold an [`Axis`] and a ratio. Splits are manual: split right or
//! down from a pane, nothing is laid out for you.
//!
//! [`Tab::rects`] turns a tree and the pixels it gets into each pane's rectangle and each
//! divider's: they tile the bounds exactly, and no pane drops under [`MIN_W`] x [`MIN_H`]
//! (a split that wouldn't fit is refused, a ratio is clamped). Ratios are fractions, so
//! resizing one split leaves the proportions inside its sides alone.

use serde::{Deserialize, Serialize};

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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
    /// Which split it belongs to: pass it to [`Tab::set_ratio`].
    pub path: Path,
    /// Everything the split covers, both sides and the line.
    pub area: Rect,
}

/// Where every pane and divider of a tab sits.
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

/// One tab: a name and a tree of panes, one of them focused.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tab {
    pub name: String,
    root: Node,
    focus: PaneId,
    /// The pane filling the whole tab, if zoomed.
    zoomed: Option<PaneId>,
}

impl Tab {
    pub fn new(name: impl Into<String>, pane: PaneId) -> Self {
        Self { name: name.into(), root: Node::Pane(pane), focus: pane, zoomed: None }
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

    /// Splits the whole tab in two, `new` after everything there (right for `Row`, below for
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

    /// Closes `pane`: its sibling takes the space. False if it isn't here or is the only
    /// pane (close the tab instead).
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

    /// Zooms the focused pane to fill the tab, or puts the others back.
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
        self.root.valid() && self.contains(self.focus) && self.zoomed.is_none_or(|z| self.contains(z))
    }
}

fn overlap(a0: f32, a1: f32, b0: f32, b1: f32) -> f32 {
    a1.min(b1) - a0.max(b0)
}

/// What a workspace shows: tabs, one of them active.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Layout {
    tabs: Vec<Tab>,
    active: usize,
}

impl Layout {
    /// One tab named `name` holding just `pane`.
    pub fn new(name: impl Into<String>, pane: PaneId) -> Self {
        Self { tabs: vec![Tab::new(name, pane)], active: 0 }
    }

    pub fn tabs(&self) -> &[Tab] {
        &self.tabs
    }

    pub fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }

    pub fn active(&self) -> usize {
        self.active
    }

    pub fn tab(&self) -> Option<&Tab> {
        self.tabs.get(self.active)
    }

    pub fn tab_mut(&mut self) -> Option<&mut Tab> {
        self.tabs.get_mut(self.active)
    }

    /// Every pane in every tab.
    pub fn panes(&self) -> Vec<PaneId> {
        self.tabs.iter().flat_map(Tab::panes).collect()
    }

    /// The tab holding `pane`.
    pub fn tab_of(&self, pane: PaneId) -> Option<usize> {
        self.tabs.iter().position(|t| t.contains(pane))
    }

    /// The panes in sight: the active tab's.
    pub fn visible(&self) -> Vec<PaneId> {
        match self.tab() {
            Some(t) => t.zoomed().map_or_else(|| t.panes(), |z| vec![z]),
            None => Vec::new(),
        }
    }

    /// The focused pane of the active tab.
    pub fn focus(&self) -> Option<PaneId> {
        self.tab().map(Tab::focus)
    }

    /// A new tab after the active one, active now. False if `pane` is already somewhere here.
    pub fn new_tab(&mut self, name: impl Into<String>, pane: PaneId) -> bool {
        if self.tab_of(pane).is_some() {
            return false;
        }
        let at = if self.tabs.is_empty() { 0 } else { self.active + 1 };
        self.tabs.insert(at, Tab::new(name, pane));
        self.active = at;
        true
    }

    /// Closes tab `i`; the panes it held are returned.
    pub fn close_tab(&mut self, i: usize) -> Vec<PaneId> {
        if i >= self.tabs.len() {
            return Vec::new();
        }
        let gone = self.tabs.remove(i);
        if i < self.active || self.active >= self.tabs.len() {
            self.active = self.active.saturating_sub(1);
        }
        gone.panes()
    }

    pub fn rename_tab(&mut self, i: usize, name: impl Into<String>) {
        if let Some(t) = self.tabs.get_mut(i) {
            t.name = name.into();
        }
    }

    /// Moves tab `from` to place `to`; the active tab stays the same tab.
    pub fn move_tab(&mut self, from: usize, to: usize) {
        if from >= self.tabs.len() || to >= self.tabs.len() || from == to {
            return;
        }
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
        self.active = match self.active {
            a if a == from => to,
            a if from < a && a <= to => a - 1,
            a if to <= a && a < from => a + 1,
            a => a,
        };
    }

    pub fn select(&mut self, i: usize) -> bool {
        if i >= self.tabs.len() {
            return false;
        }
        self.active = i;
        true
    }

    /// Jumps to tab `n`, counted from 1 as the keys do.
    pub fn select_number(&mut self, n: usize) -> bool {
        n.checked_sub(1).is_some_and(|i| self.select(i))
    }

    pub fn next_tab(&mut self) {
        if !self.tabs.is_empty() {
            self.active = (self.active + 1) % self.tabs.len();
        }
    }

    pub fn prev_tab(&mut self) {
        if !self.tabs.is_empty() {
            self.active = (self.active + self.tabs.len() - 1) % self.tabs.len();
        }
    }

    /// Splits `target` in its tab (see [`Tab::split`]) and makes that tab active.
    pub fn split(&mut self, target: PaneId, axis: Axis, new: PaneId, bounds: Rect) -> bool {
        self.split_with(target, axis, new, 0.5, bounds)
    }

    /// [`Self::split`] with `ratio` of the room going to `target` (see [`Tab::split_with`]).
    pub fn split_with(&mut self, target: PaneId, axis: Axis, new: PaneId, ratio: f32, bounds: Rect) -> bool {
        if self.tab_of(new).is_some() {
            return false;
        }
        let Some(i) = self.tab_of(target) else { return false };
        let done = self.tabs[i].split_with(target, axis, new, ratio, bounds);
        if done {
            self.active = i;
        }
        done
    }

    /// Splits the whole active tab (see [`Tab::split_root`]).
    pub fn split_root(&mut self, axis: Axis, new: PaneId, ratio: f32, bounds: Rect) -> bool {
        self.tab_of(new).is_none() && self.tab_mut().is_some_and(|t| t.split_root(axis, new, ratio, bounds))
    }

    /// Closes a pane; the tab goes with it if it was the last one. False if it isn't here.
    pub fn close_pane(&mut self, pane: PaneId) -> bool {
        let Some(i) = self.tab_of(pane) else { return false };
        if self.tabs[i].close(pane) {
            return true;
        }
        self.close_tab(i);
        true
    }

    /// Takes a pane out of its tab without closing the session behind it (the tab goes
    /// if it held only this pane). For moving it somewhere else.
    pub fn take(&mut self, pane: PaneId) -> bool {
        self.close_pane(pane)
    }

    /// Puts `pane` into tab `tab`, as a split of that tab's focused pane. With no tabs
    /// at all it starts one named `name`. False if the tab has no room for it.
    pub fn adopt(&mut self, tab: usize, pane: PaneId, axis: Axis, name: &str, bounds: Rect) -> bool {
        if self.tab_of(pane).is_some() {
            return false;
        }
        if self.tabs.is_empty() {
            return self.new_tab(name, pane);
        }
        let Some(t) = self.tabs.get_mut(tab) else { return false };
        let focus = t.focus();
        let done = t.split(focus, axis, pane, bounds);
        if done {
            self.active = tab;
        }
        done
    }

    /// Moves `pane` to tab `tab` of this layout. Nothing changes if it has no room there.
    pub fn move_to_tab(&mut self, pane: PaneId, tab: usize, bounds: Rect) -> bool {
        let Some(from) = self.tab_of(pane) else { return false };
        if from == tab || tab >= self.tabs.len() {
            return false;
        }
        let target = self.tabs[tab].focus();
        // Along the focused pane's longer side (in units of a pane's least size), else the other.
        let r = self.tabs[tab].rects(bounds).pane(target).unwrap_or(bounds);
        let first = if r.w / MIN_W >= r.h / MIN_H { Axis::Row } else { Axis::Column };
        let second = if first == Axis::Row { Axis::Column } else { Axis::Row };
        let mut trial = self.tabs[tab].clone();
        if !trial.split(target, first, pane, bounds) {
            trial = self.tabs[tab].clone();
            if !trial.split(target, second, pane, bounds) {
                return false;
            }
        }
        self.tabs[tab] = trial;
        let last = self.tabs[from].panes().len() == 1;
        if last {
            self.tabs.remove(from);
            self.active = if from < tab { tab - 1 } else { tab };
        } else {
            self.tabs[from].close(pane);
            self.active = tab;
        }
        true
    }

    /// Reads a saved layout, or `None` if it is broken (a focus that isn't in its tab,
    /// a pane twice).
    pub fn from_json(json: &str) -> Option<Self> {
        let layout: Self = serde_json::from_str(json).ok()?;
        layout.is_valid().then_some(layout)
    }

    /// Whether the layout holds together: no pane twice, every tab's focus (and zoom) in
    /// it, the active tab there. What a layout read from a file has to pass.
    pub fn is_valid(&self) -> bool {
        let panes = self.panes();
        let mut seen = panes.clone();
        seen.sort_unstable();
        seen.dedup();
        seen.len() == panes.len()
            && self.tabs.iter().all(Tab::valid)
            && (self.tabs.is_empty() && self.active == 0 || self.active < self.tabs.len())
    }

    /// Gives every pane the id `f` returns for it (focus and zoom included), for panes
    /// read from a file whose ids may collide with ones in use. `f` must answer the same
    /// for the same id, and different ids must get different answers.
    pub fn renumber(&mut self, mut f: impl FnMut(PaneId) -> PaneId) {
        for tab in &mut self.tabs {
            tab.renumber(&mut f);
        }
    }
}

/// Moves `pane` from one workspace's layout into the active tab of another's.
/// Nothing changes (false) if `to` has no room for it or already has that pane.
pub fn move_between(from: &mut Layout, to: &mut Layout, pane: PaneId, name: &str, bounds: Rect) -> bool {
    if from.tab_of(pane).is_none() || to.tab_of(pane).is_some() {
        return false;
    }
    let tab = to.active();
    if !to.adopt(tab, pane, Axis::Row, name, bounds) {
        return false;
    }
    from.take(pane)
}

#[cfg(test)]
mod tests {
    use super::*;

    const B: Rect = Rect { x: 0.0, y: 0.0, w: 1600.0, h: 900.0 };

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 0.01
    }

    /// Three panes: 1 | (2 over 3).
    fn three() -> Tab {
        let mut t = Tab::new("t", 1);
        assert!(t.split(1, Axis::Row, 2, B));
        assert!(t.split(2, Axis::Column, 3, B));
        t
    }

    #[test]
    fn a_root_split_runs_along_the_whole_tab() {
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
        let mut t = Tab::new("t", 1);
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
        let mut t = Tab::new("t", 1);
        assert!(t.split(1, Axis::Column, 2, B));
        let r = t.rects(B);
        assert!(r.pane(1).unwrap().y < r.pane(2).unwrap().y);
    }

    #[test]
    fn split_refuses_missing_duplicate_and_cramped() {
        let mut t = Tab::new("t", 1);
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
    fn zoom_fills_the_tab_and_ends_on_another_focus() {
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
        assert_eq!(Tab::new("t", 1).zoomed(), None);
        let mut one = Tab::new("t", 1);
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
    fn tabs_open_close_rename_and_move() {
        let mut l = Layout::new("one", 1);
        assert!(l.new_tab("two", 2));
        assert!(l.new_tab("three", 3));
        assert!(!l.new_tab("dup", 2));
        assert_eq!(l.active(), 2);
        l.rename_tab(0, "uno");
        assert_eq!(l.tabs()[0].name, "uno");
        l.move_tab(2, 0);
        assert_eq!(l.tabs().iter().map(|t| t.focus()).collect::<Vec<_>>(), vec![3, 1, 2]);
        assert_eq!(l.active(), 0, "the active tab moved with it");
        l.move_tab(1, 2);
        assert_eq!(l.tabs().iter().map(|t| t.focus()).collect::<Vec<_>>(), vec![3, 2, 1]);
        assert_eq!(l.active(), 0);
        assert_eq!(l.close_tab(0), vec![3]);
        assert_eq!(l.active(), 0);
        assert_eq!(l.panes(), vec![2, 1]);
        assert!(l.close_tab(9).is_empty());
    }

    #[test]
    fn closing_tabs_keeps_the_active_one() {
        let mut l = Layout::new("a", 1);
        l.new_tab("b", 2);
        l.new_tab("c", 3);
        l.select(1);
        l.close_tab(0);
        assert_eq!(l.focus(), Some(2));
        l.close_tab(1);
        assert_eq!(l.focus(), Some(2));
        l.close_tab(0);
        assert!(l.is_empty());
        assert_eq!(l.focus(), None);
        assert!(l.visible().is_empty());
        l.next_tab();
        l.prev_tab();
    }

    #[test]
    fn tab_navigation() {
        let mut l = Layout::new("a", 1);
        l.new_tab("b", 2);
        l.new_tab("c", 3);
        l.next_tab();
        assert_eq!(l.active(), 0);
        l.prev_tab();
        assert_eq!(l.active(), 2);
        assert!(l.select_number(2));
        assert_eq!(l.active(), 1);
        assert!(!l.select_number(0));
        assert!(!l.select_number(4));
        assert!(!l.select(3));
        assert_eq!(l.active(), 1);
    }

    #[test]
    fn splitting_a_pane_in_another_tab_switches_to_it() {
        let mut l = Layout::new("a", 1);
        l.new_tab("b", 2);
        assert!(l.split(1, Axis::Row, 3, B));
        assert_eq!(l.active(), 0);
        assert_eq!(l.visible(), vec![1, 3]);
        assert!(!l.split(1, Axis::Row, 2, B), "2 is already in another tab");
        assert!(!l.split(9, Axis::Row, 4, B));
    }

    #[test]
    fn closing_the_last_pane_closes_the_tab() {
        let mut l = Layout::new("a", 1);
        l.new_tab("b", 2);
        l.split(2, Axis::Row, 3, B);
        assert!(l.close_pane(3));
        assert_eq!(l.tabs().len(), 2);
        assert!(l.close_pane(2));
        assert_eq!(l.tabs().len(), 1);
        assert!(!l.close_pane(2));
    }

    #[test]
    fn a_pane_moves_to_another_tab() {
        let mut l = Layout::new("a", 1);
        l.split(1, Axis::Row, 2, B);
        l.new_tab("b", 3);
        assert!(l.move_to_tab(2, 1, B));
        assert_eq!(l.tabs()[0].panes(), vec![1]);
        assert_eq!(l.tabs()[1].panes(), vec![3, 2]);
        assert_eq!(l.active(), 1);
        assert!(!l.move_to_tab(2, 1, B), "already there");
        assert!(!l.move_to_tab(2, 7, B));
        assert!(l.move_to_tab(1, 1, B));
        assert_eq!(l.tabs().len(), 1, "the emptied tab went");
        assert_eq!(l.tabs()[0].panes(), vec![3, 2, 1]);
        assert_eq!(l.active(), 0);
    }

    #[test]
    fn a_moved_pane_splits_the_focused_pane_along_its_longer_side() {
        let narrow = Rect::new(0.0, 0.0, MIN_W * 2.0 + DIVIDER, 900.0);
        let mut l = Layout::new("a", 1);
        l.split(1, Axis::Row, 2, narrow);
        l.new_tab("b", 3);
        assert!(l.move_to_tab(3, 0, narrow), "no room beside 2, so under it");
        let r = l.tabs()[0].rects(narrow);
        let (two, three) = (r.pane(2).unwrap(), r.pane(3).unwrap());
        assert!(three.y > two.y && close(three.x, two.x));
    }

    #[test]
    fn a_pane_without_room_stays_put() {
        let tiny = Rect::new(0.0, 0.0, MIN_W * 2.0 + DIVIDER, MIN_H);
        let mut l = Layout::new("a", 1);
        l.split(1, Axis::Row, 2, tiny);
        l.new_tab("b", 3);
        l.select(0);
        assert!(!l.move_to_tab(3, 0, tiny));
        assert_eq!(l.tabs().len(), 2);
    }

    #[test]
    fn a_pane_moves_to_another_workspace() {
        let mut a = Layout::new("a", 1);
        a.split(1, Axis::Row, 2, B);
        let mut b = Layout::default();
        assert!(move_between(&mut a, &mut b, 2, "moved", B));
        assert_eq!(a.panes(), vec![1]);
        assert_eq!(b.panes(), vec![2]);
        assert_eq!(b.tabs()[0].name, "moved");
        assert!(move_between(&mut a, &mut b, 1, "x", B), "the last pane takes its tab with it");
        assert!(a.is_empty());
        assert_eq!(b.visible(), vec![2, 1]);
        assert!(!move_between(&mut a, &mut b, 1, "x", B));
    }

    #[test]
    fn serde_round_trips_and_rejects_broken_layouts() {
        let mut l = Layout::new("a", 1);
        l.split(1, Axis::Row, 2, B);
        l.split(2, Axis::Column, 3, B);
        l.new_tab("b", 4);
        l.select(0);
        let json = serde_json::to_string(&l).unwrap();
        assert_eq!(Layout::from_json(&json), Some(l));
        assert_eq!(Layout::from_json("{"), None);
        let twice = json.replace("\"Pane\":4", "\"Pane\":1").replace("\"focus\":4", "\"focus\":1");
        assert_eq!(Layout::from_json(&twice), None);
        assert_eq!(Layout::from_json(r#"{"tabs":[],"active":3}"#), None);
        assert!(Layout::from_json(r#"{"tabs":[],"active":0}"#).is_some());
    }

    #[test]
    fn renumber_rewrites_panes_focus_and_zoom() {
        let mut l = Layout::new("a", 1);
        l.split(1, Axis::Row, 2, B);
        l.split(2, Axis::Column, 3, B);
        l.tab_mut().unwrap().toggle_zoom();
        l.new_tab("b", 4);
        l.select(0);
        l.renumber(|p| p + 100);
        assert_eq!(l.panes(), vec![101, 102, 103, 104]);
        let t = &l.tabs()[0];
        assert_eq!((t.focus(), t.zoomed()), (103, Some(103)));
        assert!(l.is_valid());
        assert_eq!(l.focus(), Some(103));
    }

    #[test]
    fn a_pane_drops_out_of_a_loaded_layout() {
        let mut l = Layout::new("a", 1);
        l.split(1, Axis::Row, 2, B);
        l.new_tab("b", 3);
        assert!(l.close_pane(2));
        assert!(l.close_pane(3));
        assert_eq!(l.panes(), vec![1]);
        assert!(l.is_valid());
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
            let mut t = Tab::new("t", 0);
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
