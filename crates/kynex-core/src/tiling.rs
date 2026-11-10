//! What one context shows: its [`Layout`] (tabs of split panes) and the session each pane
//! holds. The app keeps this in step with the sessions that are open and the one the
//! window talks to ([`Live::current`](crate::live::Live::current)): picking a session
//! somewhere else (the sessions panel, a context) shows it in the focused pane, or
//! focuses the pane that already holds it.

use std::collections::HashMap;

use crate::layout::{Axis, Layout, PaneId, Rect, Rects};
use crate::live::Key;

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

    /// Every pane in every tab.
    pub fn panes(&self) -> Vec<PaneId> {
        self.layout.panes()
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

    /// Closes the focused pane when the window has others to keep (its session stays open,
    /// off screen). Returns the pane that closed.
    pub fn close_focused(&mut self) -> Option<PaneId> {
        if self.layout.panes().len() < 2 {
            return None;
        }
        let pane = self.focus()?;
        self.layout.close_pane(pane).then(|| {
            self.keys.remove(&pane);
            pane
        })
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
}
