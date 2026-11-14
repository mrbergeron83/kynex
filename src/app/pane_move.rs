//! Moving a pane inside its tab: dragged by its handle onto another pane (an edge puts it
//! on that side, the middle swaps the two, and the target shows where it will land), or
//! the pane menu's "Move left / right / up / down". The move itself is
//! `kynex_core::tiling::Tiling::move_pane`.

use gpui::{AnyElement, Context, DragMoveEvent, Window, div, prelude::*};
use kynex_core::layout::{Direction, PaneId, Rect};
use kynex_core::tiling::drop_side;

use super::KynexApp;
use super::pane_tabs::DraggedPane;
use crate::theme::theme;
use crate::ui::px;

/// The pane menu's moves, with their labels.
pub(super) const MOVES: [(Direction, &str); 4] = [
    (Direction::Left, "Move left"),
    (Direction::Right, "Move right"),
    (Direction::Up, "Move up"),
    (Direction::Down, "Move down"),
];

impl KynexApp {
    /// Where a pane dragged at window point `pos` would land on the tile at `tile` (in the
    /// panes' area): which side, or `None` for a swap.
    fn landing(&self, tile: Rect, pos: gpui::Point<gpui::Pixels>) -> Option<Option<Direction>> {
        let area = self.tile_area.get()?;
        let z = self.saved.zoom;
        let (x, y) = (f32::from(pos.x - area.origin.x) / z, f32::from(pos.y - area.origin.y) / z);
        let inside = x >= tile.x && x <= tile.x + tile.w && y >= tile.y && y <= tile.y + tile.h;
        inside.then(|| drop_side(tile, x, y))
    }

    /// A pane dragged over the tile of pane `target` at `tile`: remembers where it would land.
    pub(super) fn drag_pane_over(
        &mut self,
        target: PaneId,
        tile: Rect,
        ev: &DragMoveEvent<DraggedPane>,
        cx: &mut Context<Self>,
    ) {
        let dragged = ev.drag(cx).pane;
        let hit = self.landing(tile, ev.event.position).filter(|_| dragged != target);
        let was = self.pane_drop;
        match hit {
            Some(side) => self.pane_drop = Some((target, side)),
            // Off this tile: forget it only if it was this tile's.
            None if was.is_some_and(|(p, _)| p == target) => self.pane_drop = None,
            None => {}
        }
        if self.pane_drop != was {
            cx.notify();
        }
    }

    /// Pane `d` let go over pane `target`'s tile at `tile`.
    pub(super) fn drop_pane_on(
        &mut self,
        d: &DraggedPane,
        target: PaneId,
        tile: Rect,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pane_drop = None;
        let Some(side) = self.landing(tile, window.mouse_position()) else { return };
        if d.pane == target {
            return;
        }
        let Some((_, ws, _)) = self.tiling_scope() else { return };
        let bounds = self.layout_bounds(cx);
        let moved = self.tiles.get_mut(&ws).is_some_and(|t| t.move_pane(d.pane, target, side, bounds));
        self.after_move(moved, window, cx);
    }

    /// The pane menu's "Move left" and the rest: `pane` past its neighbour on the `dir` side.
    pub(super) fn move_pane_dir(&mut self, pane: PaneId, dir: Direction, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, ws, _)) = self.tiling_scope() else { return };
        let bounds = self.layout_bounds(cx);
        let moved = self.tiles.get_mut(&ws).is_some_and(|t| t.move_dir(pane, dir, bounds));
        self.after_move(moved, window, cx);
    }

    fn after_move(&mut self, moved: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !moved {
            self.toast(kynex_core::toasts::Toast::error(
                "No room to move it there: the panes would get too small.".to_string(),
            ));
        }
        self.show_focused_pane(window, cx);
        self.focus_input(window, cx);
        cx.notify();
    }

    /// The wash on pane `pane`'s tile (`w` x `h`) while a pane dragged over it would land
    /// there: the half it would take, or all of it for a swap. Gone once the drag ends.
    pub(super) fn pane_drop_wash(
        &mut self,
        pane: PaneId,
        w: f32,
        h: f32,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if self.pane_drop.is_some() && !cx.has_active_drag() {
            self.pane_drop = None;
        }
        let (target, side) = self.pane_drop?;
        if target != pane {
            return None;
        }
        let r = match side {
            None => Rect::new(0.0, 0.0, w, h),
            Some(Direction::Left) => Rect::new(0.0, 0.0, w / 2.0, h),
            Some(Direction::Right) => Rect::new(w / 2.0, 0.0, w / 2.0, h),
            Some(Direction::Up) => Rect::new(0.0, 0.0, w, h / 2.0),
            Some(Direction::Down) => Rect::new(0.0, h / 2.0, w, h / 2.0),
        };
        let t = theme(cx);
        Some(
            div()
                .absolute()
                .left(px(r.x))
                .top(px(r.y))
                .w(px(r.w))
                .h(px(r.h))
                .rounded_md()
                .bg(t.border_focused.opacity(0.18))
                .border_2()
                .border_color(t.border_focused)
                .into_any_element(),
        )
    }
}
