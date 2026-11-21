//! The control socket's pane layout commands: move, swap, resize and zoom a pane of the shown
//! workspace, through the same layout functions the keyboard and mouse use.

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
                    .and_then(|(_, ws)| self.tiles.get(&ws))
                    .is_some_and(|t| t.can_move_dir(pane, dir, bounds));
                if !can {
                    return Err(Some(format!("pane {pane} can't move {}", dir_word(dir))));
                }
                self.move_pane_dir(pane, dir, window, cx);
                Ok(json!({ "pane": pane, "moved": dir_word(dir) }))
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
        let Some((_, ws)) = self.tiling_scope() else { return Err(None) };
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
        let Some((_, ws)) = self.tiling_scope() else { return Err(None) };
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
        let zoomed = self.tiling_scope().and_then(|(_, ws)| self.tiles.get(&ws)?.zoomed());
        if off && zoomed != Some(pane) {
            return Ok(json!({ "zoomed": false }));
        }
        self.focus_pane(pane, window, cx);
        for _ in 0..2 {
            let zoomed = self.tiling_scope().and_then(|(_, ws)| self.tiles.get(&ws)?.zoomed());
            if (zoomed == Some(pane)) == !off {
                break;
            }
            self.toggle_pane_zoom(window, cx);
        }
        let zoomed = self.tiling_scope().and_then(|(_, ws)| self.tiles.get(&ws)?.zoomed()) == Some(pane);
        if !off && !zoomed {
            return Err(Some(format!("pane {pane} can't zoom: it's the only pane")));
        }
        Ok(json!({ "zoomed": zoomed }))
    }
}
