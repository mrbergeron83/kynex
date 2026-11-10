//! What one workspace shows: its [`Layout`] (tabs of split panes) and the session each pane
//! holds. The app keeps this in step with the sessions that are open and the one the
//! window talks to ([`Live::current`](crate::live::Live::current)): picking a session
//! somewhere else (the sessions panel, a workspace) shows it in the focused pane, or
//! focuses the pane that already holds it.

use std::collections::HashMap;

use crate::layout::{Axis, Direction, Layout, PaneId, Rect, Rects};
use crate::live::Key;

/// One tab of a workspace, for the tab bar.
#[derive(Clone, Debug, PartialEq)]
pub struct TabInfo {
    pub name: String,
    /// The sessions in its panes, in reading order.
    pub sessions: Vec<Key>,
    pub active: bool,
}

pub struct Tiling {
    layout: Layout,
    keys: HashMap<PaneId, Key>,
}

impl Tiling {
    /// One tab with one pane `pane`, showing session `key`.
    pub fn new(pane: PaneId, key: Key) -> Self {
        Self { layout: Layout::new("1", pane), keys: HashMap::from([(pane, key)]) }
    }

    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    pub fn has(&self, pane: PaneId) -> bool {
        self.layout.tab_of(pane).is_some()
    }

    pub fn key_of(&self, pane: PaneId) -> Option<Key> {
        self.keys.get(&pane).copied()
    }

    /// The focused pane of the active tab.
    pub fn focus(&self) -> Option<PaneId> {
        self.layout.focus()
    }

    /// The panes in sight, with their sessions, in reading order.
    pub fn visible(&self) -> Vec<(PaneId, Key)> {
        self.layout.visible().into_iter().filter_map(|p| Some((p, self.key_of(p)?))).collect()
    }

    /// The sessions in the panes of every tab, in reading order.
    pub fn sessions(&self) -> Vec<Key> {
        self.layout.panes().into_iter().filter_map(|p| self.key_of(p)).collect()
    }

    /// Every pane in every tab.
    pub fn panes(&self) -> Vec<PaneId> {
        self.layout.panes()
    }

    /// The tabs, in order, for the tab bar.
    pub fn tabs(&self) -> Vec<TabInfo> {
        let active = self.layout.active();
        self.layout
            .tabs()
            .iter()
            .enumerate()
            .map(|(i, t)| TabInfo {
                name: t.name.clone(),
                sessions: t.panes().into_iter().filter_map(|p| self.key_of(p)).collect(),
                active: i == active,
            })
            .collect()
    }

    /// A new tab after the active one, holding `pane` (showing `key`), and active. It is
    /// named with the next number. False if `key` or `pane` is here already.
    pub fn new_tab(&mut self, pane: PaneId, key: Key) -> bool {
        if self.keys.values().any(|k| *k == key) {
            return false;
        }
        let name = self.next_tab_name();
        if !self.layout.new_tab(name, pane) {
            return false;
        }
        self.keys.insert(pane, key);
        true
    }

    /// Shows tab `i`. False if there is no such tab.
    pub fn select_tab(&mut self, i: usize) -> bool {
        self.layout.select(i)
    }

    /// Closes tab `i` unless it is the only one. Returns the panes it held with their
    /// sessions (which stay open, off screen).
    pub fn close_tab(&mut self, i: usize) -> Vec<(PaneId, Key)> {
        if self.layout.tabs().len() < 2 {
            return Vec::new();
        }
        let panes = self.layout.close_tab(i);
        panes.into_iter().filter_map(|p| Some((p, self.keys.remove(&p)?))).collect()
    }

    /// Names tab `i`; a blank name leaves it as it was.
    pub fn rename_tab(&mut self, i: usize, name: &str) {
        let name = name.trim();
        if !name.is_empty() {
            self.layout.rename_tab(i, name);
        }
    }

    /// Moves tab `from` to place `to`; the active tab stays the same tab.
    pub fn move_tab(&mut self, from: usize, to: usize) {
        self.layout.move_tab(from, to);
    }

    /// Focuses `pane` (its tab comes up). False if it isn't here.
    pub fn focus_pane(&mut self, pane: PaneId) -> bool {
        let Some(i) = self.layout.tab_of(pane) else { return false };
        self.layout.select(i);
        self.layout.tab_mut().is_some_and(|t| t.set_focus(pane))
    }

    /// Where the panes and dividers of the active tab sit in `bounds`.
    pub fn rects(&self, bounds: Rect) -> Rects {
        self.layout.tab().map(|t| t.rects(bounds)).unwrap_or_default()
    }

    /// Moves the divider of the split at `path` to `ratio` of its split (kept inside the panes'
    /// minimum sizes). False if there is no such split.
    pub fn set_ratio(&mut self, path: &[bool], ratio: f32, bounds: Rect) -> bool {
        self.layout.tab_mut().is_some_and(|t| t.set_ratio(path, ratio, bounds))
    }

    /// Splits the focused pane, the new one (showing `key`) on its right (`Row`) or below
    /// (`Column`) and focused. False if it would leave a pane too small for `bounds`, or
    /// `key` is on show already.
    pub fn split(&mut self, axis: Axis, pane: PaneId, key: Key, bounds: Rect) -> bool {
        if self.keys.values().any(|k| *k == key) {
            return false;
        }
        let Some(target) = self.focus() else { return false };
        if !self.layout.split(target, axis, pane, bounds) {
            return false;
        }
        self.keys.insert(pane, key);
        true
    }

    /// The pane showing session `key`, in any tab.
    pub fn pane_of(&self, key: Key) -> Option<PaneId> {
        self.layout.panes().into_iter().find(|p| self.key_of(*p) == Some(key))
    }

    /// Puts a new pane (showing `key`) at `side` of pane `target`, and focuses it. False if
    /// it would leave a pane too small for `bounds`, `target` isn't here or `key` is on show.
    pub fn split_at(&mut self, target: PaneId, side: Direction, pane: PaneId, key: Key, bounds: Rect) -> bool {
        if self.keys.values().any(|k| *k == key) {
            return false;
        }
        let axis = match side {
            Direction::Left | Direction::Right => Axis::Row,
            Direction::Up | Direction::Down => Axis::Column,
        };
        if !self.layout.split(target, axis, pane, bounds) {
            return false;
        }
        // A split puts the new pane after its target; before it is the two swapped.
        if matches!(side, Direction::Left | Direction::Up)
            && let Some(t) = self.layout.tab_mut()
        {
            t.swap(target, pane);
        }
        self.keys.insert(pane, key);
        true
    }

    /// Shows session `key` in pane `pane` in place of what it had. False if the pane isn't
    /// here or the session is in another pane.
    pub fn show_in(&mut self, pane: PaneId, key: Key) -> bool {
        if !self.has(pane) || self.pane_of(key).is_some_and(|p| p != pane) {
            return false;
        }
        self.keys.insert(pane, key);
        true
    }

    /// The pane filling the active tab, if one is zoomed.
    pub fn zoomed(&self) -> Option<PaneId> {
        self.layout.tab().and_then(|t| t.zoomed())
    }

    /// Zooms the focused pane to fill its tab, or puts the others back.
    pub fn toggle_zoom(&mut self) {
        if let Some(t) = self.layout.tab_mut() {
            t.toggle_zoom();
        }
    }

    /// Closes `pane` when the window has others to keep (its session stays open, off
    /// screen). Returns the session it showed.
    pub fn close_pane(&mut self, pane: PaneId) -> Option<Key> {
        if self.layout.panes().len() < 2 || !self.layout.close_pane(pane) {
            return None;
        }
        self.keys.remove(&pane)
    }

    /// Moves `pane` to tab `tab`, beside that tab's focused pane. False if it is there
    /// already or has no room.
    pub fn move_to_tab(&mut self, pane: PaneId, tab: usize, bounds: Rect) -> bool {
        self.layout.move_to_tab(pane, tab, bounds)
    }

    /// Moves `pane` to a new tab of its own. False if it is alone in its tab already.
    pub fn move_to_new_tab(&mut self, pane: PaneId) -> bool {
        let Some(from) = self.layout.tab_of(pane) else { return false };
        if self.layout.tabs()[from].panes().len() < 2 {
            return false;
        }
        let name = self.next_tab_name();
        self.layout.take(pane) && self.layout.new_tab(name, pane)
    }

    fn next_tab_name(&self) -> String {
        let numbers = self.layout.tabs().iter().filter_map(|t| t.name.parse::<usize>().ok());
        (numbers.max().unwrap_or(0).max(self.layout.tabs().len()) + 1).to_string()
    }

    /// Closes the focused pane when the window has others to keep (its session stays open,
    /// off screen). Returns the pane that closed.
    pub fn close_focused(&mut self) -> Option<PaneId> {
        let pane = self.focus()?;
        self.close_pane(pane).map(|_| pane)
    }

    /// Brings the panes in line with the sessions: a pane whose session closed goes (the
    /// last one stays), and `current`, the session the window talks to, ends up in the
    /// focused pane: focusing the pane already showing it, else taking the focused pane's
    /// place. Returns the panes that went.
    pub fn reconcile(&mut self, open: &[Key], current: Key) -> Vec<PaneId> {
        let mut gone = Vec::new();
        for pane in self.layout.panes() {
            let alive = self.key_of(pane).is_some_and(|k| open.contains(&k));
            if !alive && self.layout.panes().len() > 1 && self.layout.close_pane(pane) {
                self.keys.remove(&pane);
                gone.push(pane);
            }
        }
        match self.keys.iter().find(|(_, k)| **k == current).map(|(p, _)| *p) {
            Some(pane) => {
                self.focus_pane(pane);
            }
            None => {
                if let Some(focus) = self.focus() {
                    self.keys.insert(focus, current);
                }
            }
        }
        gone
    }
}

/// Which side of the pane at `r` a drop at (`x`, `y`) is for: the nearest edge when it is
/// within the outer 30%, else `None` (the middle: the pane itself).
pub fn drop_side(r: Rect, x: f32, y: f32) -> Option<Direction> {
    if r.w <= 0.0 || r.h <= 0.0 {
        return None;
    }
    let (fx, fy) = ((x - r.x) / r.w, (y - r.y) / r.h);
    let sides = [(fx, Direction::Left), (1.0 - fx, Direction::Right), (fy, Direction::Up), (1.0 - fy, Direction::Down)];
    let (near, side) = sides.into_iter().min_by(|a, b| a.0.total_cmp(&b.0))?;
    (near < 0.3).then_some(side)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BIG: Rect = Rect { x: 0.0, y: 0.0, w: 2000.0, h: 1000.0 };

    fn three() -> Tiling {
        let mut t = Tiling::new(1, 10);
        assert!(t.split(Axis::Row, 2, 20, BIG));
        assert!(t.split(Axis::Column, 3, 30, BIG));
        t
    }

    #[test]
    fn the_sessions_are_those_of_every_pane() {
        assert_eq!(three().sessions(), [10, 20, 30]);
    }

    #[test]
    fn a_split_shows_a_new_session_in_the_new_focused_pane() {
        let t = three();
        assert_eq!(t.focus(), Some(3));
        assert_eq!(t.visible(), [(1, 10), (2, 20), (3, 30)]);
    }

    #[test]
    fn a_session_already_on_show_cannot_be_split_in_again() {
        let mut t = Tiling::new(1, 10);
        assert!(!t.split(Axis::Row, 2, 10, BIG));
        assert_eq!(t.panes(), [1]);
    }

    #[test]
    fn picking_a_session_that_is_on_show_focuses_its_pane() {
        let mut t = three();
        assert!(t.reconcile(&[10, 20, 30], 10).is_empty());
        assert_eq!(t.focus(), Some(1));
        assert_eq!(t.visible().len(), 3, "nothing was replaced");
    }

    #[test]
    fn picking_a_session_that_is_not_on_show_replaces_the_focused_pane() {
        let mut t = three();
        t.focus_pane(2);
        assert!(t.reconcile(&[10, 20, 30, 40], 40).is_empty());
        assert_eq!(t.visible(), [(1, 10), (2, 40), (3, 30)]);
        assert_eq!(t.focus(), Some(2));
    }

    #[test]
    fn a_pane_whose_session_closed_goes() {
        let mut t = three();
        // 30 closed; 20 is where the window is.
        assert_eq!(t.reconcile(&[10, 20], 20), [3]);
        assert_eq!(t.visible(), [(1, 10), (2, 20)]);
        assert_eq!(t.focus(), Some(2));
    }

    #[test]
    fn the_last_pane_stays_and_takes_the_current_session() {
        let mut t = Tiling::new(1, 10);
        assert!(t.reconcile(&[11], 11).is_empty());
        assert_eq!(t.visible(), [(1, 11)]);
    }

    #[test]
    fn closing_the_focused_pane_keeps_the_last_one() {
        let mut t = three();
        assert_eq!(t.close_focused(), Some(3));
        assert_eq!(t.close_focused(), Some(2));
        assert_eq!(t.close_focused(), None);
        assert_eq!(t.visible(), [(1, 10)]);
    }

    #[test]
    fn a_new_tab_is_numbered_and_active() {
        let mut t = three();
        assert!(t.new_tab(4, 40));
        let tabs = t.tabs();
        assert_eq!(tabs.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["1", "2"]);
        assert_eq!(tabs[0].sessions, [10, 20, 30]);
        assert_eq!((tabs[1].sessions.clone(), tabs[1].active), (vec![40], true));
        assert_eq!(t.visible(), [(4, 40)]);
        // A session that is on show somewhere can't open a second time.
        assert!(!t.new_tab(5, 10));
        // Numbers aren't reused while a higher one is around.
        t.rename_tab(1, "review");
        assert!(t.new_tab(5, 50));
        assert_eq!(t.tabs().iter().map(|t| t.name.clone()).collect::<Vec<_>>(), ["1", "review", "3"]);
    }

    #[test]
    fn selecting_a_tab_shows_its_panes_and_picking_a_session_brings_its_tab_up() {
        let mut t = three();
        t.new_tab(4, 40);
        assert!(t.select_tab(0));
        assert_eq!(t.visible().len(), 3);
        assert!(!t.select_tab(7));
        // 40 becomes the session the window talks to: its tab comes up.
        t.reconcile(&[10, 20, 30, 40], 40);
        assert_eq!(t.visible(), [(4, 40)]);
    }

    #[test]
    fn closing_a_tab_gives_back_its_panes_and_keeps_the_last_tab() {
        let mut t = three();
        t.new_tab(4, 40);
        assert_eq!(t.close_tab(0), [(1, 10), (2, 20), (3, 30)]);
        assert_eq!(t.tabs().len(), 1);
        assert_eq!(t.visible(), [(4, 40)]);
        assert!(t.close_tab(0).is_empty(), "the only tab stays");
    }

    #[test]
    fn tabs_are_renamed_and_moved() {
        let mut t = three();
        t.new_tab(4, 40);
        t.rename_tab(0, "  agents ");
        t.rename_tab(1, "   ");
        assert_eq!(t.tabs().iter().map(|t| t.name.clone()).collect::<Vec<_>>(), ["agents", "2"]);
        t.move_tab(1, 0);
        let tabs = t.tabs();
        assert_eq!(tabs.iter().map(|t| t.name.clone()).collect::<Vec<_>>(), ["2", "agents"]);
        assert!(tabs[0].active, "the active tab is still the same tab");
    }

    #[test]
    fn a_pane_goes_at_any_side_of_another() {
        let mut t = Tiling::new(1, 10);
        assert!(t.split_at(1, Direction::Left, 2, 20, BIG));
        assert_eq!(t.visible(), [(2, 20), (1, 10)], "left of it comes first");
        assert_eq!(t.focus(), Some(2));
        assert!(t.split_at(1, Direction::Down, 3, 30, BIG));
        assert_eq!(t.visible(), [(2, 20), (1, 10), (3, 30)]);
        assert!(t.split_at(1, Direction::Up, 4, 40, BIG));
        assert_eq!(t.visible(), [(2, 20), (4, 40), (1, 10), (3, 30)]);
        // Not a session that's on show, nor beside a pane that isn't there.
        assert!(!t.split_at(1, Direction::Right, 5, 20, BIG));
        assert!(!t.split_at(9, Direction::Right, 5, 50, BIG));
        assert_eq!(t.pane_of(40), Some(4));
    }

    #[test]
    fn a_session_takes_a_panes_place() {
        let mut t = three();
        assert!(t.show_in(2, 40));
        assert_eq!(t.visible(), [(1, 10), (2, 40), (3, 30)]);
        assert!(!t.show_in(2, 10), "10 is in another pane");
        assert!(t.show_in(2, 40), "showing what it shows is fine");
        assert!(!t.show_in(9, 50));
    }

    #[test]
    fn zoom_shows_one_pane_and_back() {
        let mut t = three();
        t.toggle_zoom();
        assert_eq!(t.zoomed(), Some(3));
        assert_eq!(t.visible(), [(3, 30)]);
        t.toggle_zoom();
        assert_eq!((t.zoomed(), t.visible().len()), (None, 3));
    }

    #[test]
    fn any_pane_closes_but_the_last() {
        let mut t = three();
        assert_eq!(t.close_pane(1), Some(10));
        assert_eq!(t.close_pane(1), None);
        assert_eq!(t.close_pane(2), Some(20));
        assert_eq!(t.close_pane(3), None, "the last pane stays");
    }

    #[test]
    fn panes_move_between_tabs() {
        let mut t = three();
        assert!(t.move_to_new_tab(3));
        assert_eq!(t.tabs().iter().map(|t| t.sessions.clone()).collect::<Vec<_>>(), [vec![10, 20], vec![30]]);
        assert!(!t.move_to_new_tab(3), "alone in its tab already");
        assert!(t.move_to_tab(2, 1, BIG));
        assert_eq!(t.tabs().iter().map(|t| t.sessions.clone()).collect::<Vec<_>>(), [vec![10], vec![30, 20]]);
        assert!(!t.move_to_tab(2, 1, BIG));
    }

    #[test]
    fn a_drop_near_an_edge_is_for_that_side() {
        let r = Rect::new(100.0, 50.0, 400.0, 200.0);
        assert_eq!(drop_side(r, 120.0, 150.0), Some(Direction::Left));
        assert_eq!(drop_side(r, 480.0, 150.0), Some(Direction::Right));
        assert_eq!(drop_side(r, 300.0, 60.0), Some(Direction::Up));
        assert_eq!(drop_side(r, 300.0, 240.0), Some(Direction::Down));
        assert_eq!(drop_side(r, 300.0, 150.0), None, "the middle is the pane itself");
        assert_eq!(drop_side(Rect::new(0.0, 0.0, 0.0, 0.0), 1.0, 1.0), None);
    }
}
