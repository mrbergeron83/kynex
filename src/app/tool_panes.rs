//! Tool panes: the Sessions & Workspaces tree, Agents pane, Git pane, and plain Terminal panes.

use std::path::Path;

use gpui::{AnyElement, Context, Window, div, prelude::*};
use kynex_core::layout::{Direction, PaneId};
use kynex_core::tiling::{Content, Tool};
use kynex_core::toasts::Toast;

use super::KynexApp;

/// How wide the sessions column is made, in pixels, as far as the window allows.
const SIDE_WIDTH: f32 = 280.0;

impl KynexApp {
    /// Tool pane `pane`'s body.
    pub(super) fn render_tool_pane(
        &mut self,
        pane: PaneId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let (_, ws) = self.tiling_scope()?;
        let t = self.tiles.get(&ws)?;
        match t.content(pane)? {
            Content::Terminal => self.render_terminal_pane(pane, window, cx),
            Content::Git => {
                let width = t.rects(self.layout_bounds(cx)).pane(pane).map_or(f32::MAX, |r| r.w);
                let git = self.git_pane(&ws, window, cx);
                git.update(cx, |g, _| {
                    g.set_width(width);
                    g.set_pane(pane);
                });
                Some(div().size_full().child(git).into_any_element())
            }
            Content::Tree => Some(self.render_sessions_pane(pane, window, cx)),
            Content::Agents => Some(self.render_agents_pane(pane, cx)),
            Content::AgentCli => None,
        }
    }

    fn render_sessions_pane(&mut self, pane: PaneId, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = crate::theme::theme(cx).clone();
        div().size_full().bg(t.panel).child(self.render_sessions_panel(window, cx, Some(pane))).into_any_element()
    }

    pub(super) fn show_sessions_pane(&mut self, target: PaneId, window: &mut Window, cx: &mut Context<Self>) {
        self.open_tool(target, Tool::Tree, 0.7, window, cx);
    }

    pub(super) fn side_share(&self, cx: &Context<Self>) -> f32 {
        if self.tile_area.get().is_none() {
            return 0.2;
        }
        (SIDE_WIDTH / self.layout_bounds(cx).w).clamp(0.12, 0.4)
    }

    /// Every tab of workspace `cwd` gets its Sessions tree pane once.
    pub(super) fn seed_side_panes(&mut self, cwd: &Path, cx: &mut Context<Self>) {
        if self.tiles.get(cwd).is_none_or(|t| t.seeded()) {
            return;
        }
        let (ratio, bounds) = (self.side_share(cx), self.layout_bounds(cx));
        let mut next = self.next_pane;
        if let Some(t) = self.tiles.get_mut(cwd) {
            t.seed_side_panes(
                || {
                    next += 1;
                    next - 1
                },
                ratio,
                bounds,
            );
        }
        self.next_pane = next;
    }

    /// `prefix b` and the toolbar button: toggle the tab's Sessions tree pane.
    pub(super) fn toggle_side_panes(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some((_, cwd)) = self.tiling_scope() else { return false };
        let (ratio, bounds) = (self.side_share(cx), self.layout_bounds(cx));
        let mut next = self.next_pane;
        let Some(t) = self.tiles.get_mut(&cwd) else { return false };
        if t.has_side_panes() {
            t.remove_side_panes();
        } else {
            t.add_side_panes(
                || {
                    next += 1;
                    next - 1
                },
                ratio,
                bounds,
            );
        }
        self.next_pane = next;
        self.show_focused_pane(window, cx);
        self.save_layouts(cx);
        cx.notify();
        true
    }

    pub(super) fn sessions_hidden(&self) -> bool {
        self.tiling_scope().and_then(|(_, ws)| self.tiles.get(&ws)).is_some_and(|t| !t.has_side_panes())
    }

    pub(super) fn tool_pane(&self, kind: Content) -> Option<PaneId> {
        let (_, ws) = self.tiling_scope()?;
        self.tiles.get(&ws)?.panes_of(kind).first().copied()
    }

    pub(super) fn open_tool(
        &mut self,
        target: PaneId,
        tool: Tool,
        ratio: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(p) = self.tool_pane(tool.content()) {
            self.focus_pane(p, window, cx);
            return;
        }
        let Some((_, ws)) = self.tiling_scope() else { return };
        let id = self.next_pane;
        let bounds = self.layout_bounds(cx);
        let Some(t) = self.tiles.get_mut(&ws) else { return };
        if !t.split_tool(target, Direction::Right, id, tool, ratio, bounds) {
            self.toast(Toast::error("No room to split here: the panes would get too small.".to_string()));
            cx.notify();
            return;
        }
        self.next_pane += 1;
        self.show_focused_pane(window, cx);
        self.save_layouts(cx);
    }

    pub(super) fn close_tool_pane(&mut self, pane: PaneId, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, ws)) = self.tiling_scope() else { return };
        let Some(t) = self.tiles.get_mut(&ws) else { return };
        if !matches!(t.close_pane(pane), Some(Content::Git | Content::Tree | Content::Agents)) {
            return;
        }
        self.show_focused_pane(window, cx);
        self.save_layouts(cx);
        cx.notify();
    }
}
