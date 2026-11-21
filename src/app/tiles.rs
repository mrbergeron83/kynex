//! The split panes of a workspace: each workspace (a folder) has a [`Tiling`] of
//! panes holding `agent_cli` terminals and tool panes (Sessions tree, Agents, Git, Terminal).

use std::path::PathBuf;

use gpui::{AnyElement, ClickEvent, Context, DragMoveEvent, MouseButton, Window, canvas, div, prelude::*};
use kynex_core::layout::{Axis, DIVIDER, Divider, PaneId, Rect};
use kynex_core::tile_style::TileStyle;
use kynex_core::tiling::{AgentCliPane, Content, Tiling};

use super::KynexApp;
use super::session_groups::DraggedSession;
use crate::theme::theme;
use crate::ui::pane_drag::DraggedPane;
use crate::ui::px;

/// Room for a split when the area hasn't been measured yet.
const UNMEASURED: Rect = Rect { x: 0.0, y: 0.0, w: 4000.0, h: 4000.0 };

impl KynexApp {
    /// The active project's selected workspace: `(project folder, workspace folder)`.
    pub(super) fn tiling_scope(&self) -> Option<(PathBuf, PathBuf)> {
        let project = self.active_project()?.path.clone();
        let cwd = self.workspace_cwd().unwrap_or_else(|| project.clone());
        Some((project, cwd))
    }

    fn tiled(&self) -> Option<&Tiling> {
        let (_, cwd) = self.tiling_scope()?;
        self.tiles.get(&cwd)
    }

    /// Focuses pane `id` of the workspace you're in, and moves keyboard focus into it.
    pub(super) fn focus_pane(&mut self, id: PaneId, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, cwd)) = self.tiling_scope() else { return };
        let Some(t) = self.tiles.get_mut(&cwd) else { return };
        if !t.focus_pane(id) {
            return;
        }
        if let Some(&agent_id) = self.pane_agents.get(&id) {
            self.mark_agent_seen(agent_id, cx);
        }
        self.focus_input(window, cx);
        cx.notify();
    }

    /// Syncs focus after a layout change (split, close, tab switch).
    pub(super) fn show_focused_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_input(window, cx);
        cx.notify();
    }

    /// The area the panes fill, in logical pixels (what the layout works in).
    pub(super) fn tile_bounds(&self) -> Rect {
        match self.tile_area.get() {
            Some(b) => {
                let z = self.saved.zoom;
                Rect::new(0.0, 0.0, f32::from(b.size.width) / z, f32::from(b.size.height) / z)
            }
            None => UNMEASURED,
        }
    }

    /// Splits the focused pane: a new pending `agent_cli` pane (showing the provider picker)
    /// beside (`Row`) or under (`Column`) it.
    pub(super) fn split_pane(&mut self, axis: Axis, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, cwd)) = self.tiling_scope() else { return };
        let id = self.next_pane;
        let bounds = self.tile_bounds();
        let Some(t) = self.tiles.get_mut(&cwd) else { return };
        if !t.split_agent(axis, id, AgentCliPane::pending(), bounds) {
            self.toast(kynex_core::toasts::Toast::error(
                "No room to split here: the panes would get too small.".to_string(),
            ));
            cx.notify();
            return;
        }
        self.next_pane += 1;
        self.agent_pickers.insert(id, super::agent_picker::AgentPickerState::default());
        self.focus_input(window, cx);
        self.save_layouts(cx);
        cx.notify();
    }

    /// Closes the focused pane (the last `agent_cli` pane in a workspace resets to the picker).
    pub(super) fn close_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, cwd)) = self.tiling_scope() else { return };
        let Some(focus) = self.tiles.get(&cwd).and_then(Tiling::focus) else { return };
        self.close_pane_id(focus, window, cx);
    }

    /// Focuses the next (`1`) or previous (`-1`) pane of the tab.
    pub(super) fn step_pane(&mut self, by: isize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(t) = self.tiled() else { return };
        let panes = t.visible_panes();
        if panes.is_empty() {
            return;
        }
        let at = t.focus().and_then(|f| panes.iter().position(|p| *p == f)).unwrap_or(0) as isize;
        let next = panes[(at + by).rem_euclid(panes.len() as isize) as usize];
        self.focus_pane(next, window, cx);
    }

    /// What each frame settles for the panes: ensures the active workspace has a `Tiling`,
    /// restores saved layouts, seeds the Sessions tree pane, and cleans up closed panes.
    pub(super) fn sync_tiles(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.agent_rules_asked {
            self.agent_rules_asked = true;
            self.refresh_agent_rules(cx);
            self.prune_scrollback(cx);
        }
        let Some((project, cwd)) = self.tiling_scope() else { return };
        if !self.tiles.contains_key(&cwd) {
            let tiling = match self.restore_tiling(&project, &cwd) {
                Some(t) => t,
                None => {
                    let id = self.next_pane;
                    self.next_pane += 1;
                    self.agent_pickers.insert(id, super::agent_picker::AgentPickerState::default());
                    Tiling::new(id, AgentCliPane::pending())
                }
            };
            self.tiles.insert(cwd.clone(), tiling);
            self.tile_projects.insert(cwd.clone(), project.clone());
            self.seed_side_panes(&cwd, cx);
            self.note_histories(&cwd);
            self.migrate_terminal_dock(&cwd, window, cx);
        }
        self.keep_shape(&cwd);
        self.drop_gone_agents(cx);
        self.drop_gone_terminals(cx);
    }

    fn core_bounds(&self, gap: f32) -> Rect {
        let b = self.tile_bounds();
        let half = gap / 2.0;
        Rect::new(half, half, (b.w - gap).max(0.0), (b.h - gap).max(0.0))
    }

    pub(super) fn layout_bounds(&self, cx: &Context<Self>) -> Rect {
        self.core_bounds(self.tile_style(cx).gap)
    }

    fn tile_style(&self, cx: &Context<Self>) -> TileStyle {
        self.saved.tiles.resolve(theme(cx).tile)
    }

    fn handle_of(d: &Divider, gap: f32) -> Rect {
        let grow = (gap / 2.0).max(3.0);
        let r = d.rect;
        match d.axis {
            Axis::Row => Rect::new(r.x - grow, r.y, r.w + 2.0 * grow, r.h),
            Axis::Column => Rect::new(r.x, r.y - grow, r.w, r.h + 2.0 * grow),
        }
    }

    fn start_tile_drag(&mut self, splits: Vec<Resizing>, cx: &mut Context<Self>) {
        self.tile_drag = Some(splits);
        cx.notify();
    }

    pub(super) fn drag_tiles(&mut self, pos: gpui::Point<gpui::Pixels>, cx: &mut Context<Self>) {
        let Some(drag) = self.tile_drag.clone() else { return };
        let Some(area) = self.tile_area.get() else { return };
        let Some((_, cwd)) = self.tiling_scope() else { return };
        let z = self.saved.zoom;
        let (x, y) = (f32::from(pos.x - area.origin.x) / z, f32::from(pos.y - area.origin.y) / z);
        let bounds = self.core_bounds(self.tile_style(cx).gap);
        let Some(t) = self.tiles.get_mut(&cwd) else { return };
        for Resizing { path, axis } in drag {
            let rects = t.rects(bounds);
            let Some(d) = rects.dividers.iter().find(|d| d.path == path) else { continue };
            let (at, from, len) = match axis {
                Axis::Row => (x, d.area.x, d.area.w),
                Axis::Column => (y, d.area.y, d.area.h),
            };
            let avail = len - DIVIDER;
            if avail > 0.0 {
                t.set_ratio(&path, (at - from - DIVIDER / 2.0) / avail, bounds);
            }
        }
        cx.notify();
    }

    fn even_out(&mut self, path: &[bool], cx: &mut Context<Self>) {
        let Some((_, cwd)) = self.tiling_scope() else { return };
        let bounds = self.core_bounds(self.tile_style(cx).gap);
        if let Some(t) = self.tiles.get_mut(&cwd) {
            t.set_ratio(path, 0.5, bounds);
        }
        cx.notify();
    }

    /// Renders all panes of the active workspace as tiles.
    pub(super) fn render_panes(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (_, ws) = self.tiling_scope()?;
        let t = theme(cx).clone();
        let style = self.tile_style(cx);
        let bounds = self.core_bounds(style.gap);
        let tiling = self.tiles.get(&ws)?;
        let rects = tiling.rects(bounds);
        let measured = self.tile_area.get().is_some();
        let panes: Vec<(PaneId, Content, Rect)> =
            rects.panes.iter().filter_map(|(id, r)| Some((*id, tiling.content(*id)?, *r))).collect();
        let focus = tiling.focus();
        let several = tiling.visible_panes().len() > 1 || tiling.zoomed().is_some();
        let focus_colour = t.tile_border.active.or_else(|| self.active_colour(&t)).unwrap_or(t.border_focused);
        let area = self.tile_area.clone();
        let probe = canvas(
            move |bounds, window, _| {
                if area.get() != Some(bounds) {
                    area.set(Some(bounds));
                    window.refresh();
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0();
        let flush = style.gap == 0.0;
        let half = style.gap / 2.0;
        let (bw, fw) = if flush { (0, 0) } else { (style.border as u32, (style.border as u32 + 1).min(4)) };
        let radius = if flush { 0.0 } else { style.radius };
        let mut children: Vec<AnyElement> = Vec::new();
        if measured {
            for (id, content, r) in panes {
                let focused = Some(id) == focus;
                let body = match content {
                    Content::AgentCli => self.render_agent_cli_pane(id, &ws, several, focused, window, cx),
                    _ => {
                        let panel = self.render_tool_pane(id, window, cx);
                        div()
                            .size_full()
                            .on_mouse_down(
                                MouseButton::Right,
                                cx.listener(move |this, ev: &gpui::MouseDownEvent, _, cx| {
                                    this.open_menu(super::menus::Menu::Pane(id), ev.position, gpui::Anchor::TopLeft, cx)
                                }),
                            )
                            .children(panel)
                            .into_any_element()
                    }
                };
                let width = if focused { fw } else { bw };
                let surface = bordered(div().size_full().overflow_hidden().bg(t.editor), width)
                    .border_color(if focused { focus_colour } else { t.tile_border.inactive.unwrap_or(t.border) })
                    .rounded(px(radius))
                    .child(body);
                let tile = Rect::new(r.x + half, r.y + half, (r.w - style.gap).max(0.0), (r.h - style.gap).max(0.0));
                let wash = self.pane_drop_wash(id, tile.w, tile.h, cx);
                let tile_el =
                    div()
                        .absolute()
                        .left(px(r.x + half))
                        .top(px(r.y + half))
                        .w(px((r.w - style.gap).max(0.0)))
                        .h(px((r.h - style.gap).max(0.0)))
                        .when(!focused && fw > bw, |d| d.p_px())
                        .capture_any_mouse_down(cx.listener(move |this, _, window, cx| {
                            this.focus_pane(id, window, cx);
                        }))
                        .drag_over::<DraggedSession>(|style, _, _, _| style.opacity(0.7))
                        .on_drop(cx.listener(move |this, d: &DraggedSession, window, cx| {
                            this.drop_session_on_pane(d, id, Some(tile), window, cx)
                        }))
                        .on_drag_move(cx.listener(move |this, ev: &DragMoveEvent<DraggedPane>, _, cx| {
                            this.drag_pane_over(id, tile, ev, cx)
                        }))
                        .on_drop(cx.listener(move |this, d: &DraggedPane, window, cx| {
                            this.drop_pane_on(d, id, tile, window, cx)
                        }))
                        .child(surface)
                        .children(wash);
                children.push(tile_el.into_any_element());
            }
            if flush {
                for d in &rects.dividers {
                    let r = d.rect;
                    children.push(
                        div()
                            .absolute()
                            .left(px(r.x))
                            .top(px(r.y))
                            .w(px(r.w))
                            .h(px(r.h))
                            .bg(t.border)
                            .into_any_element(),
                    );
                }
            }
            let hits: Vec<Rect> = rects.dividers.iter().map(|d| Self::handle_of(d, style.gap)).collect();
            let splits: Vec<Resizing> =
                rects.dividers.iter().map(|d| Resizing { path: d.path.clone(), axis: d.axis }).collect();
            let mut handles: Vec<(Rect, Vec<Resizing>, Option<Axis>)> = Vec::new();
            for (i, d) in rects.dividers.iter().enumerate() {
                handles.push((hits[i], vec![splits[i].clone()], Some(d.axis)));
            }
            for i in 0..hits.len() {
                for j in i + 1..hits.len() {
                    if rects.dividers[i].axis != rects.dividers[j].axis
                        && let Some(both) = overlap(hits[i], hits[j])
                    {
                        handles.push((both, vec![splits[i].clone(), splits[j].clone()], None));
                    }
                }
            }
            for (n, (r, drag, axis)) in handles.into_iter().enumerate() {
                let first = drag[0].path.clone();
                let el = div().id(("tile-handle", n)).absolute().left(px(r.x)).top(px(r.y)).w(px(r.w)).h(px(r.h));
                let el = match axis {
                    Some(Axis::Row) => el.cursor_col_resize(),
                    Some(Axis::Column) => el.cursor_row_resize(),
                    None => el.cursor_move(),
                };
                children.push(
                    el.on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| this.start_tile_drag(drag.clone(), cx)),
                    )
                    .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                        if ev.click_count() == 2 {
                            this.even_out(&first, cx);
                        }
                    }))
                    .into_any_element(),
                );
            }
        }
        Some(
            div()
                .relative()
                .flex_1()
                .min_h_0()
                .min_w_0()
                .bg(if flush { t.editor } else { t.panel })
                .child(probe)
                .children(children)
                .into_any_element(),
        )
    }
}

#[derive(Clone)]
pub(super) struct Resizing {
    path: Vec<bool>,
    axis: Axis,
}

fn bordered(d: gpui::Div, w: u32) -> gpui::Div {
    match w {
        0 => d.border_0(),
        1 => d.border_1(),
        2 => d.border_2(),
        3 => d.border_3(),
        _ => d.border_4(),
    }
}

fn overlap(a: Rect, b: Rect) -> Option<Rect> {
    let (x, y) = (a.x.max(b.x), a.y.max(b.y));
    let (r, bottom) = ((a.x + a.w).min(b.x + b.w), (a.y + a.h).min(b.y + b.h));
    (r > x && bottom > y).then(|| Rect::new(x, y, r - x, bottom - y))
}
