//! The tree pane (the sessions under their workspaces) of a [`Tiling`]: seeded on the left of
//! every tab, toggled with Ctrl+B b.

use super::{Content, Direction, PaneId, Rect, Tiling, Tool};

impl Tiling {
    /// Whether `pane` holds the tree.
    pub fn is_side(&self, pane: PaneId) -> bool {
        self.content(pane) == Some(Content::Tree)
    }

    /// Whether the active tab has the tree as a pane.
    pub fn has_side_panes(&self) -> bool {
        self.layout.tab().is_some_and(|t| t.panes().into_iter().any(|p| self.is_side(p)))
    }

    /// Whether the tabs got their tree pane already.
    pub fn seeded(&self) -> bool {
        self.seeded
    }

    /// Puts the tree on the left of the active tab, `ratio` of its width (`ids` gives the new
    /// pane's id). Does nothing if the tab has one, or there is no room.
    pub fn add_side_panes(&mut self, mut ids: impl FnMut() -> PaneId, ratio: f32, bounds: Rect) -> bool {
        if self.has_side_panes() {
            return false;
        }
        let Some(target) = self.session_focus().or_else(|| self.layout.tab().and_then(|t| t.panes().first().copied()))
        else {
            return false;
        };
        let tree = ids();
        if !self.split_tool(target, Direction::Left, tree, Tool::Tree, ratio, bounds) {
            return false;
        }
        // The focus goes back to the session.
        self.focus_pane(target);
        true
    }

    /// Every tab gets its tree pane, once (what the sidebar used to be). The active tab stays
    /// active.
    pub fn seed_side_panes(&mut self, mut ids: impl FnMut() -> PaneId, ratio: f32, bounds: Rect) {
        let active = self.layout.active();
        for i in 0..self.layout.tabs().len() {
            self.layout.select(i);
            self.add_side_panes(&mut ids, ratio, bounds);
        }
        self.layout.select(active);
        self.seeded = true;
    }

    /// Takes the active tab's tree pane away. Returns how many went.
    pub fn remove_side_panes(&mut self) -> usize {
        let side: Vec<PaneId> =
            self.layout.tab().map(|t| t.panes().into_iter().filter(|p| self.is_side(*p)).collect()).unwrap_or_default();
        side.into_iter().filter(|p| self.close_pane(*p).is_some()).count()
    }
}

#[cfg(test)]
mod tests;
