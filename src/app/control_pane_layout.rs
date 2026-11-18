//! The control socket's pane layout commands: move, swap, resize and zoom a pane of the shown
//! workspace, through the same layout functions the keyboard and mouse use. Moving a pane to
//! another workspace isn't one of them.

use gpui::{Context, Window};
use kynex_core::control::protocol::PaneTarget;
use kynex_core::layout::{Direction, PaneId};
use serde_json::{Value, json};

use super::KynexApp;
use super::pane_keys::RESIZE_STEP;

fn dir_word(dir: Direction) -> &'static str {
    match dir {
        Direction::Left => "left",
        Direction::Right => "right",
        Direction::Up => "up",
        Direction::Down => "down",
    }
}

impl KynexApp {
    fn shown_pane(&self, pane: PaneId) -> Result<(), Option<String>> {
        if self.tiles_have(pane) { Ok(()) } else { Err(Some(format!("no pane {pane} in the shown workspace"))) }
    }

    /// The shown workspace's pane layout, if any.
    fn layout_tab_of(&self, pane: PaneId) -> Option<usize> {
        let (_, ws, _) = self.tiling_scope()?;
        self.tiles.get(&ws)?.layout().tab_of(pane)
    }

    /// The panes of tab `i` of the shown workspace.
    fn tab_panes(&self, i: usize) -> Vec<PaneId> {
        self.tiling_scope()
            .and_then(|(_, ws, _)| self.tiles.get(&ws))
            .and_then(|t| t.layout().tabs().get(i).map(|tab| tab.panes()))
            .unwrap_or_default()
    }

    pub(super) fn control_pane_move(
        &mut self,
        pane: PaneId,
        to: PaneTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Value, Option<String>> {
        self.shown_pane(pane)?;
        match to {
            PaneTarget::Dir(dir) => {
                let bounds = self.layout_bounds(cx);
                let can = self
                    .tiling_scope()
                    .and_then(|(_, ws, _)| self.tiles.get(&ws))
                    .is_some_and(|t| t.can_move_dir(pane, dir, bounds));
                if !can {
                    return Err(Some(format!("pane {pane} can't move {}", dir_word(dir))));
                }
                self.move_pane_dir(pane, dir, window, cx);
                Ok(json!({ "pane": pane, "moved": dir_word(dir) }))
            }
            PaneTarget::Tab(n) => {
                let tabs = self
                    .tiling_scope()
                    .and_then(|(_, ws, _)| self.tiles.get(&ws))
                    .map_or(0, |t| t.layout().tabs().len());
                if n == 0 || n > tabs {
                    return Err(Some(format!("no tab {n} (the workspace has {tabs})")));
                }
                if self.layout_tab_of(pane) == Some(n - 1) {
                    return Err(Some(format!("pane {pane} is in tab {n} already")));
                }
                // A pane of the target tab, to find it again: tab numbers shift when the moved
                // pane's own tab empties and goes.
                let anchor = self.tab_panes(n - 1).first().copied();
                self.move_pane(pane, Some(n - 1), window, cx);
                let now = self.layout_tab_of(pane);
                if now.is_none() || now != anchor.and_then(|a| self.layout_tab_of(a)) {
                    return Err(Some(format!("no room for pane {pane} in tab {n}")));
                }
                Ok(json!({ "pane": pane, "tab": now.map_or(n, |t| t + 1) }))
            }
            PaneTarget::NewTab => {
                // Alone in its tab, it would only make another tab of one.
                let from = self.layout_tab_of(pane);
                if from.is_some_and(|t| self.tab_panes(t).len() < 2) {
                    return Err(Some(format!("pane {pane} is alone in its tab already")));
                }
                self.move_pane(pane, None, window, cx);
                match self.layout_tab_of(pane) {
                    Some(t) if self.tab_panes(t) == [pane] => Ok(json!({ "pane": pane, "tab": t + 1 })),
                    _ => Err(Some(format!("couldn't move pane {pane} to a new tab"))),
                }
            }
        }
    }

    pub(super) fn control_pane_swap(
        &mut self,
        pane: PaneId,
        other: PaneId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Value, Option<String>> {
        self.shown_pane(pane)?;
        self.shown_pane(other)?;
        if pane == other {
            return Err(Some(format!("pane {pane} can't swap with itself")));
        }
        if self.layout_tab_of(pane) != self.layout_tab_of(other) {
            return Err(Some(format!("panes {pane} and {other} are in different tabs")));
        }
        let Some((_, ws, _)) = self.tiling_scope() else { return Err(None) };
        let bounds = self.layout_bounds(cx);
        let swapped = self.tiles.get_mut(&ws).is_some_and(|t| t.move_pane(pane, other, None, bounds));
        self.after_move(swapped, window, cx);
        if swapped { Ok(json!({ "swapped": [pane, other] })) } else { Err(Some("couldn't swap them".into())) }
    }

    pub(super) fn control_pane_resize(
        &mut self,
        pane: PaneId,
        dir: Direction,
        by: Option<u32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Value, Option<String>> {
        self.shown_pane(pane)?;
        self.focus_pane(pane, window, cx);
        let px = by.map_or(RESIZE_STEP, |b| b as f32);
        let bounds = self.layout_bounds(cx);
        let Some((_, ws, _)) = self.tiling_scope() else { return Err(None) };
        let grown = self.tiles.get_mut(&ws).is_some_and(|t| t.resize_focused(dir, px, bounds));
        cx.notify();
        if grown {
            Ok(json!({ "pane": pane, "grew": dir_word(dir), "by": px }))
        } else {
            Err(Some(format!("pane {pane} can't grow {}", dir_word(dir))))
        }
    }

    pub(super) fn control_pane_zoom(
        &mut self,
        pane: PaneId,
        off: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Value, Option<String>> {
        self.shown_pane(pane)?;
        let zoomed = self.tiling_scope().and_then(|(_, ws, _)| self.tiles.get(&ws)?.zoomed());
        // Off on a pane that isn't zoomed: nothing to do, and another zoomed pane stays.
        if off && zoomed != Some(pane) {
            return Ok(json!({ "zoomed": false }));
        }
        self.focus_pane(pane, window, cx);
        // Zooming toggles the focused pane; with another pane zoomed, the first toggle only
        // puts it back, so look again after each.
        for _ in 0..2 {
            let zoomed = self.tiling_scope().and_then(|(_, ws, _)| self.tiles.get(&ws)?.zoomed());
            if (zoomed == Some(pane)) == !off {
                break;
            }
            self.toggle_pane_zoom(window, cx);
        }
        let zoomed = self.tiling_scope().and_then(|(_, ws, _)| self.tiles.get(&ws)?.zoomed()) == Some(pane);
        if !off && !zoomed {
            return Err(Some(format!("pane {pane} can't zoom: it's alone in its tab")));
        }
        Ok(json!({ "zoomed": zoomed }))
    }
}
