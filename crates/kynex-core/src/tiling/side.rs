//! The tree pane (the sessions under their workspaces) of a [`Tiling`]: seeded on the left,
//! toggled with Ctrl+B b.

use super::{Content, Direction, PaneId, Rect, Tiling, Tool};

impl Tiling {
    /// Whether `pane` holds the tree.
    pub fn is_side(&self, pane: PaneId) -> bool {
        self.content(pane) == Some(Content::Tree)
    }

    /// Whether the workspace has the tree as a pane.
    pub fn has_side_panes(&self) -> bool {
        self.layout.panes().into_iter().any(|p| self.is_side(p))
    }

    /// Whether the workspace got its tree pane already.
    pub fn seeded(&self) -> bool {
        self.seeded
    }

    /// Puts the tree on the left of the workspace, `ratio` of its width (`ids` gives the new
    /// pane's id). Does nothing if the workspace has one, or there is no room.
    pub fn add_side_panes(&mut self, mut ids: impl FnMut() -> PaneId, ratio: f32, bounds: Rect) -> bool {
        if self.has_side_panes() {
            return false;
        }
        let Some(target) = self.agent_focus().or_else(|| self.layout.panes().first().copied()) else {
            return false;
        };
        let tree = ids();
        if !self.split_tool(target, Direction::Left, tree, Tool::Tree, ratio, bounds) {
            return false;
        }
        self.focus_pane(target);
        true
    }

    /// Seeds the workspace's tree pane once.
    pub fn seed_side_panes(&mut self, mut ids: impl FnMut() -> PaneId, ratio: f32, bounds: Rect) {
        self.add_side_panes(&mut ids, ratio, bounds);
        self.seeded = true;
    }

    /// Takes the workspace's tree pane away. Returns how many went.
    pub fn remove_side_panes(&mut self) -> usize {
        let side: Vec<PaneId> = self.layout.panes().into_iter().filter(|p| self.is_side(*p)).collect();
        side.into_iter().filter(|p| self.close_pane(*p).is_some()).count()
    }
}

#[cfg(test)]
mod tests;
