//! The workspace's Git view and the timeline as panes: the changes next to the session
//! that made them, the timeline beside the session it outlines (the session pane focused
//! last). One of each per workspace; asking for another focuses it. A project's Git view
//! (the toolbar's Git switch) is still the Git view filling the window.

use std::path::Path;

use gpui::{AnyElement, Context, Window, div, prelude::*};
use kynex_core::layout::{Direction, PaneId};
use kynex_core::tiling::{Content, Tool};
use kynex_core::toasts::Toast;

use super::KynexApp;
use super::sessions_panel::Parts;
use super::timeline::WIDTH;

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
        let (_, ws, _) = self.tiling_scope()?;
        let t = self.tiles.get(&ws)?;
        match t.content(pane)? {
            Content::Terminal => self.render_terminal_pane(pane, window, cx),
            // The modal over the window draws this folder's view: one place at a time.
            Content::Git if self.git_modal.as_deref().is_some_and(|m| kynex_core::workspace::same_folder(m, &ws)) => {
                Some(div().size_full().bg(crate::theme::theme(cx).panel).into_any_element())
            }
            Content::Git => {
                let git = self.git_view(&ws, cx);
                Some(div().size_full().child(git).into_any_element())
            }
            Content::Timeline => {
                let key = t.session_focus().and_then(|p| t.key_of(p))?;
                self.render_timeline(key, true, cx)
            }
            Content::Sessions => {
                Some(self.render_sessions_pane(pane, Parts { workspaces: false, sessions: true }, window, cx))
            }
            Content::Workspaces => {
                Some(self.render_sessions_pane(pane, Parts { workspaces: true, sessions: false }, window, cx))
            }
            Content::Session(_) => None,
        }
    }

    /// The sessions list or the workspaces, in a pane: the sidebar's own views, one of them each.
    fn render_sessions_pane(
        &mut self,
        pane: PaneId,
        parts: Parts,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = crate::theme::theme(cx).clone();
        div()
            .size_full()
            .bg(t.panel)
            .child(self.render_sessions_panel(window, cx, parts, Some(pane)))
            .into_any_element()
    }

    /// The sessions list on the right of pane `target`. The sidebar leaves out what a pane shows.
    pub(super) fn show_sessions_pane(&mut self, target: PaneId, window: &mut Window, cx: &mut Context<Self>) {
        self.open_tool(target, Tool::Sessions, 0.7, window, cx);
    }

    /// The workspaces on the right of pane `target`.
    pub(super) fn show_workspaces_pane(&mut self, target: PaneId, window: &mut Window, cx: &mut Context<Self>) {
        self.open_tool(target, Tool::Workspaces, 0.7, window, cx);
    }

    /// The share of the width the sessions column gets: about [`SIDE_WIDTH`] pixels.
    pub(super) fn side_share(&self, cx: &Context<Self>) -> f32 {
        if self.tile_area.get().is_none() {
            return 0.2;
        }
        (SIDE_WIDTH / self.layout_bounds(cx).w).clamp(0.12, 0.4)
    }

    /// Every tab of the workspace `cwd` gets its Sessions and Workspaces panes, once: what the
    /// sidebar was. A tab where they were closed stays without.
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

    /// Ctrl+B b and the toolbar's button: the tab's Sessions and Workspaces panes away, or back.
    /// False outside a project's panes (chat and the Git view have the sidebar).
    pub(super) fn toggle_side_panes(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some((_, cwd, _)) = self.tiling_scope() else { return false };
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
        self.focus_input(window, cx);
        cx.notify();
        true
    }

    /// Whether the sessions list and workspaces are out of sight: the tab has none of them as
    /// panes (a project's panes), or the sidebar is folded (everywhere else).
    pub(super) fn sessions_hidden(&self) -> bool {
        match self.tiling_scope().and_then(|(_, ws, _)| self.tiles.get(&ws)) {
            Some(t) => !t.has_side_panes(),
            None => self.saved.sessions_collapsed,
        }
    }

    /// The workspace's pane holding `kind`, in any tab.
    fn tool_pane(&self, kind: Content) -> Option<PaneId> {
        let (_, ws, _) = self.tiling_scope()?;
        self.tiles.get(&ws)?.panes_of(kind).first().copied()
    }

    /// Puts `tool` on the right of pane `target`, `ratio` of the room left to `target`, or
    /// focuses the workspace's pane that has it already.
    fn open_tool(&mut self, target: PaneId, tool: Tool, ratio: f32, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(p) = self.tool_pane(tool.content()) {
            self.focus_pane(p, window, cx);
            self.focus_input(window, cx);
            return;
        }
        let Some((_, ws, _)) = self.tiling_scope() else { return };
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
        self.focus_input(window, cx);
    }

    /// The workspace's changes on the right of pane `target`, sharing its room.
    pub(super) fn show_git_pane(&mut self, target: PaneId, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((_, ws, _)) = self.tiling_scope() {
            let git = self.git_view(&ws, cx);
            git.update(cx, |g, cx| g.refresh(cx));
        }
        self.open_tool(target, Tool::Git, 0.5, window, cx);
    }

    /// The timeline on the right of pane `target`, the timeline's usual width.
    pub(super) fn show_timeline_pane(&mut self, target: PaneId, window: &mut Window, cx: &mut Context<Self>) {
        let ratio = self.side_ratio(target, cx);
        self.open_tool(target, Tool::Timeline, ratio, window, cx);
    }

    /// The share of pane `target`'s width it keeps when the timeline opens beside it.
    fn side_ratio(&self, target: PaneId, cx: &Context<Self>) -> f32 {
        let bounds = self.layout_bounds(cx);
        let w = self
            .tiling_scope()
            .and_then(|(_, ws, _)| self.tiles.get(&ws))
            .and_then(|t| t.rects(bounds).pane(target))
            .map_or(bounds.w, |r| r.w);
        if w <= 2. * WIDTH { 0.5 } else { 1. - WIDTH / w }
    }

    /// Ctrl+Shift+T in a project: closes the tab's timeline pane, else opens one beside the
    /// session. False outside a project's panes (chat has the panel beside it).
    pub(super) fn toggle_timeline_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some((_, ws, _)) = self.tiling_scope() else { return false };
        let Some(t) = self.tiles.get(&ws) else { return false };
        match (t.tab_panes_of(Content::Timeline).first().copied(), t.session_focus()) {
            (Some(p), _) => self.close_tool_pane(p, window, cx),
            (None, Some(target)) => self.show_timeline_pane(target, window, cx),
            (None, None) => {}
        }
        cx.notify();
        true
    }

    /// Whether the timeline is up: the tab's pane in a project, the panel in chat.
    pub(super) fn timeline_shown(&self) -> bool {
        match self.tiling_scope().and_then(|(_, ws, _)| self.tiles.get(&ws)) {
            Some(t) => !t.tab_panes_of(Content::Timeline).is_empty(),
            None => self.saved.timeline_open,
        }
    }

    /// Closes the Git or timeline pane `pane`.
    pub(super) fn close_tool_pane(&mut self, pane: PaneId, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, ws, _)) = self.tiling_scope() else { return };
        let Some(t) = self.tiles.get_mut(&ws) else { return };
        if !matches!(
            t.close_pane(pane),
            Some(Content::Git | Content::Timeline | Content::Sessions | Content::Workspaces)
        ) {
            return;
        }
        self.show_focused_pane(window, cx);
        self.focus_input(window, cx);
        cx.notify();
    }

    /// The timeline was open beside the transcript before it was a pane: the first
    /// workspace shown gets a timeline pane, once.
    pub(super) fn migrate_timeline(&mut self, ws: &Path, cx: &mut Context<Self>) {
        if self.saved.timeline_migrated {
            return;
        }
        self.saved.timeline_migrated = true;
        self.save();
        if !self.saved.timeline_open {
            return;
        }
        let Some(target) = self.tiles.get(ws).and_then(|t| t.session_focus()) else { return };
        let ratio = self.side_ratio(target, cx);
        let id = self.next_pane;
        let bounds = self.layout_bounds(cx);
        let Some(t) = self.tiles.get_mut(ws) else { return };
        let focus = t.focus();
        if t.split_tool(target, Direction::Right, id, Tool::Timeline, ratio, bounds) {
            self.next_pane += 1;
            if let Some(f) = focus {
                t.focus_pane(f);
            }
        }
    }
}
