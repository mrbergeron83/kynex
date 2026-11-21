//! Pane menus, opening a past session beside/inside a pane, closing, zooming, and moving panes.

use gpui::{AnyElement, Context, Window, div, prelude::*};
use kynex_core::layout::{Axis, Direction, PaneId, Rect};
use kynex_core::tiling::{AgentCliPane, Content};
use kynex_core::toasts::Toast;

use super::KynexApp;
use super::pane_move::MOVES;
use super::session_groups::DraggedSession;
use crate::ui::{divider, menu_item};

type Action = Box<dyn Fn(&mut KynexApp, &mut Window, &mut Context<KynexApp>)>;

#[derive(Clone)]
enum There {
    Shell,
    Command(String),
}

impl KynexApp {
    /// Opens past session `id` at `side` of `target` (or in `target` when `side` is `None`).
    pub(super) fn open_beside(
        &mut self,
        id: String,
        target: Option<PaneId>,
        side: Option<Direction>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((_, ws)) = self.tiling_scope() else { return };
        let cwd = self.folder_of_listed(&id).unwrap_or_else(|| ws.clone());
        if !kynex_core::workspace::same_folder(&cwd, &ws) {
            return self.show_in_workspace(&cwd, &id, window, cx);
        }
        self.edit_groups(&cwd, |g| {
            g.unarchive(&id);
        });
        if let Some(agent_id) = self.live_agent_of_session(&cwd, &id) {
            if let Some(pane) = self.pane_showing_agent(agent_id)
                && self.tiles.get(&ws).is_some_and(|t| t.has(pane))
            {
                self.focus_pane(pane, window, cx);
                return;
            }
            if side.is_none() {
                let target = target.or_else(|| self.tiles.get(&ws).and_then(|t| t.focus()));
                if let Some(pane) = target.and_then(|tgt| {
                    let t = self.tiles.get(&ws)?;
                    if t.is_agent(tgt) { Some(tgt) } else { t.agent_focus() }
                }) {
                    self.mount_agent_in_pane(pane, &ws, agent_id, window, cx);
                    return;
                }
            }
        }
        let Some(target) = target.or_else(|| self.tiles.get(&ws).and_then(|t| t.focus())) else { return };
        let provider = self
            .sessions
            .get(&cwd)
            .and_then(|l| l.items.iter().find(|i| i.id == id))
            .map(|i| i.provider.clone())
            .unwrap_or_else(|| self.groups_for(&cwd).provider_of(&id));
        let model = self.groups_for(&cwd).model_of(&id).map(str::to_string);

        let pane = match side {
            Some(side) => {
                let pane = self.next_pane;
                let bounds = self.tile_bounds();
                let Some(t) = self.tiles.get_mut(&ws) else { return };
                if !t.split_agent_at(target, side, pane, AgentCliPane::pending(), bounds) {
                    self.toast(Toast::error("No room to split here: the panes would get too small.".to_string()));
                    cx.notify();
                    return;
                }
                self.next_pane += 1;
                pane
            }
            None => {
                let Some(t) = self.tiles.get_mut(&ws) else { return };
                if t.is_agent(target) {
                    t.focus_pane(target);
                    target
                } else {
                    // Dropping in the middle of a tool pane opens in the focused agent pane.
                    let Some(agent_pane) = t.agent_focus() else { return };
                    t.focus_pane(agent_pane);
                    agent_pane
                }
            }
        };
        if let Some(agent_id) = self.live_agent_of_session(&cwd, &id) {
            self.mount_agent_in_pane(pane, &ws, agent_id, window, cx);
        } else {
            self.launch_agent_cli(pane, cwd, provider, Some(id), model, window, cx);
        }
    }

    pub(super) fn drop_session_on_pane(
        &mut self,
        d: &DraggedSession,
        pane: PaneId,
        rect: Option<Rect>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(area) = self.tile_area.get() else { return };
        let Some(id) = d.ids.first().cloned() else { return };
        let z = self.saved.zoom;
        let pos = window.mouse_position();
        let (x, y) = (f32::from(pos.x - area.origin.x) / z, f32::from(pos.y - area.origin.y) / z);
        let rect = rect.unwrap_or_else(|| {
            let b = self.tile_bounds();
            Rect::new(0.0, 0.0, b.w, b.h)
        });
        self.open_beside(id, Some(pane), kynex_core::tiling::drop_side(rect, x, y), window, cx);
    }

    /// Closes pane `id` (the last `agent_cli` pane stays and resets to the provider picker).
    pub(super) fn close_pane_id(&mut self, id: PaneId, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, ws)) = self.tiling_scope() else { return };
        match self.tiles.get(&ws).and_then(|t| t.content(id)) {
            Some(Content::Terminal) => return self.hide_terminal(id, window, cx),
            Some(Content::Git | Content::Tree | Content::Agents) => {
                return self.close_tool_pane(id, window, cx);
            }
            Some(Content::AgentCli) => {}
            None => return,
        }
        let closed = self.tiles.get_mut(&ws).and_then(|t| t.close_pane(id));
        if closed.is_none() {
            // Last agent pane in the workspace: reset it to the provider picker.
            self.reset_agent_pane_to_picker(id, window, cx);
            return;
        }
        self.drop_gone_agents(cx);
        self.show_focused_pane(window, cx);
        self.save_layouts(cx);
    }

    pub(super) fn toggle_pane_zoom(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, ws)) = self.tiling_scope() else { return };
        if let Some(t) = self.tiles.get_mut(&ws) {
            t.toggle_zoom();
        }
        self.focus_input(window, cx);
        cx.notify();
    }

    /// Launches `provider_id` in the workspace's focused/main `agent_cli` pane (parking any
    /// currently running CLI in the background rather than splitting a new pane).
    pub(super) fn open_provider_pane(
        &mut self,
        provider_id: String,
        target: Option<PaneId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((_, ws)) = self.tiling_scope() else {
            self.toast(Toast::error("Open a project folder first.".to_string()));
            cx.notify();
            return;
        };
        let pane = target
            .filter(|&p| self.tiles.get(&ws).is_some_and(|t| t.is_agent(p)))
            .or_else(|| self.tiles.get(&ws).and_then(|t| t.agent_focus()));
        let pane = match pane {
            Some(p) => {
                if let Some(t) = self.tiles.get_mut(&ws) {
                    t.focus_pane(p);
                }
                p
            }
            None => {
                let p = self.next_pane;
                let bounds = self.layout_bounds(cx);
                let Some(t) = self.tiles.get_mut(&ws) else { return };
                if !t.split_agent(Axis::Row, p, AgentCliPane::pending(), bounds) {
                    self.toast(Toast::error("No room to split here: the panes would get too small.".to_string()));
                    cx.notify();
                    return;
                }
                self.next_pane += 1;
                p
            }
        };
        self.launch_agent_cli(pane, ws, provider_id, None, None, window, cx);
    }

    fn menu_action(n: usize, label: String, cx: &mut Context<Self>, f: Action) -> AnyElement {
        menu_item(("pane-menu", n), label, false, cx)
            .on_click(cx.listener(move |this, _, window, cx| {
                this.menu = None;
                f(this, window, cx);
                cx.notify();
            }))
            .into_any_element()
    }

    pub(super) fn pane_menu_items(&mut self, pane: PaneId, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some((_, ws)) = self.tiling_scope() else { return Vec::new() };
        let bounds = self.layout_bounds(cx);
        let Some(t) = self.tiles.get(&ws) else { return Vec::new() };
        let moves: Vec<_> = MOVES.into_iter().filter(|(dir, _)| t.can_move_dir(pane, *dir, bounds)).collect();
        let zoomed = t.zoomed().is_some();
        let several = t.visible_panes().len() > 1 || zoomed;
        let terminal = t.is_terminal(pane);
        let tool = t.is_tool(pane);
        let is_agent = t.is_agent(pane);
        let has_provider = t.agent(pane).is_some_and(|m| m.provider.is_some());
        let closable = tool || t.agent_panes().len() > 1;
        let split = move |axis: Axis| -> Action {
            Box::new(move |this: &mut KynexApp, window: &mut Window, cx: &mut Context<KynexApp>| {
                if terminal {
                    let side = if axis == Axis::Row { Direction::Right } else { Direction::Down };
                    this.split_terminal(pane, side, window, cx);
                } else {
                    this.focus_pane(pane, window, cx);
                    this.split_pane(axis, window, cx);
                }
            })
        };
        let mut items = Vec::new();
        if terminal || is_agent {
            items.push(Self::menu_action(0, "Split right".into(), cx, split(Axis::Row)));
            items.push(Self::menu_action(1, "Split down".into(), cx, split(Axis::Column)));
        }
        if is_agent && has_provider {
            items.push(Self::menu_action(
                50,
                "Restart CLI".into(),
                cx,
                Box::new(move |this, window, cx| this.restart_agent_cli(pane, window, cx)),
            ));
            items.push(Self::menu_action(
                51,
                "Change provider…".into(),
                cx,
                Box::new(move |this, window, cx| this.reset_agent_pane_to_picker(pane, window, cx)),
            ));
        }
        if !tool {
            items.push(div().my_1().child(divider(cx)).into_any_element());
            items.push(Self::menu_action(
                5,
                "Changes beside".into(),
                cx,
                Box::new(move |this, window, cx| this.show_git_pane(pane, window, cx)),
            ));
            items.push(Self::menu_action(
                7,
                "Sessions beside".into(),
                cx,
                Box::new(move |this, window, cx| this.show_sessions_pane(pane, window, cx)),
            ));
            items.push(Self::menu_action(
                8,
                "Agents beside".into(),
                cx,
                Box::new(move |this, window, cx| this.show_agents_pane(pane, window, cx)),
            ));
            items.push(Self::menu_action(
                9,
                "Terminal beside".into(),
                cx,
                Box::new(move |this, window, cx| this.split_terminal(pane, Direction::Right, window, cx)),
            ));
        }
        if several {
            items.push(div().my_1().child(divider(cx)).into_any_element());
            items.push(Self::menu_action(
                2,
                if zoomed { "Unzoom".into() } else { "Zoom".into() },
                cx,
                Box::new(move |this, window, cx| {
                    this.focus_pane(pane, window, cx);
                    this.toggle_pane_zoom(window, cx);
                }),
            ));
        }
        for (i, (dir, label)) in moves.into_iter().enumerate() {
            items.push(Self::menu_action(
                400 + i,
                label.into(),
                cx,
                Box::new(move |this, window, cx| this.move_pane_dir(pane, dir, window, cx)),
            ));
        }
        if terminal {
            let tp = t.term_pane(pane).cloned().unwrap_or_default();
            let command = match tp.command {
                Some(name) => There::Command(name),
                None => There::Shell,
            };
            for (i, (path, label)) in self.other_workspaces(&ws, cx).into_iter().enumerate() {
                let what = match &command {
                    There::Command(name) => format!("Run {name} in {label}"),
                    There::Shell => format!("Open a terminal in {label}"),
                };
                let command = command.clone();
                items.push(Self::menu_action(
                    200 + i,
                    what,
                    cx,
                    Box::new(move |this, window, cx| this.terminal_in(&path, command.clone(), window, cx)),
                ));
            }
        }
        if closable {
            items.push(div().my_1().child(divider(cx)).into_any_element());
            items.push(Self::menu_action(
                4,
                if terminal { "Hide terminal".into() } else { "Close pane".into() },
                cx,
                Box::new(move |this, window, cx| this.close_pane_id(pane, window, cx)),
            ));
        }
        items
    }

    fn other_workspaces(&mut self, ws: &std::path::Path, cx: &mut Context<Self>) -> Vec<(std::path::PathBuf, String)> {
        let Some((project, _)) = self.tiling_scope() else { return Vec::new() };
        self.workspaces_of(&project, cx)
            .into_iter()
            .map(|(w, _)| (w.path, w.label))
            .filter(|(p, _)| !kynex_core::workspace::same_folder(p, ws))
            .collect()
    }

    fn terminal_in(&mut self, path: &std::path::Path, what: There, window: &mut Window, cx: &mut Context<Self>) {
        self.select_workspace(path, cx);
        self.sync_tiles(window, cx);
        match what {
            There::Command(name) => self.run_user_command(name, window, cx),
            There::Shell => {
                self.show_terminal(window, cx);
            }
        }
    }
}
