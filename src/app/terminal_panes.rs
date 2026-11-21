//! Plain shell & custom command terminal panes of a workspace (distinct from `agent_cli` panes).

use std::path::Path;

use gpui::{AnyElement, Context, Entity, Focusable, Window, prelude::*};
use kynex_core::layout::{Direction, PaneId};
use kynex_core::terminal::ShellKind;
use kynex_core::tiling::{Content, TERMINAL_RATIO, TermPane};
use kynex_core::toasts::Toast;
use kynex_core::workspace::same_folder;

use super::KynexApp;
use super::keys::ToggleTerminal;
use crate::terminal::{PanelEvent, TerminalPanel};

const MIN_HEIGHT: f32 = 120.;

impl KynexApp {
    pub(super) fn term_panel(
        &mut self,
        pane: PaneId,
        cwd: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<TerminalPanel> {
        if let Some(p) = self.term_panes.get(&pane) {
            return p.clone();
        }
        let own = self.tiles.get(cwd).and_then(|t| t.term_pane(pane)).map(|t| !t.is_shell());
        if own == Some(true) {
            let panel = Self::new_term_panel(cwd, window, cx);
            self.term_panes.insert(pane, panel.clone());
            return panel;
        }
        let hidden = self.terminals.iter().rposition(|(k, _)| same_folder(k, cwd));
        let panel = match hidden.map(|i| self.terminals.remove(i).1) {
            Some(p) => p,
            None => Self::new_term_panel(cwd, window, cx),
        };
        self.term_panes.insert(pane, panel.clone());
        panel
    }

    pub(super) fn new_term_panel(cwd: &Path, window: &mut Window, cx: &mut Context<Self>) -> Entity<TerminalPanel> {
        let panel = cx.new(|cx| TerminalPanel::new(cwd.to_path_buf(), cx));
        cx.subscribe_in(&panel, window, |this, panel, ev: &PanelEvent, window, cx| match ev {
            PanelEvent::Hide => {
                let pane = this.term_panes.iter().find(|(_, p)| *p == panel).map(|(id, _)| *id);
                if let Some(pane) = pane {
                    this.hide_terminal(pane, window, cx);
                }
            }
            PanelEvent::Collapsed => cx.notify(),
        })
        .detach();
        panel
    }

    pub(super) fn has_terminal(&self) -> bool {
        self.tiling_scope().and_then(|(_, ws)| self.tiles.get(&ws)).is_some_and(|t| !t.shell_terminals().is_empty())
    }

    pub(super) fn focused_terminal(&self) -> Option<&Entity<TerminalPanel>> {
        let (_, ws) = self.tiling_scope()?;
        let focus = self.tiles.get(&ws)?.focus()?;
        self.term_panes.get(&focus)
    }

    pub(super) fn terminal_ratio(&self, window: &Window) -> f32 {
        let area = match self.tile_area.get() {
            Some(_) => self.tile_bounds().h,
            None => f32::from(window.viewport_size().height) / self.saved.zoom - 100.,
        };
        if area <= 2. * MIN_HEIGHT {
            return TERMINAL_RATIO;
        }
        let h = self.saved.terminal_height.clamp(MIN_HEIGHT, area - MIN_HEIGHT);
        1. - h / area
    }

    pub(super) fn toggle_terminal(&mut self, _: &ToggleTerminal, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, ws)) = self.tiling_scope() else { return };
        let Some(t) = self.tiles.get(&ws) else { return };
        let focus = t.focus();
        let shell = focus.filter(|f| t.term_pane(*f).is_some_and(TermPane::is_shell));
        match (shell, t.shell_terminals().first().copied()) {
            (Some(f), _) => self.hide_terminal(f, window, cx),
            (None, Some(p)) => self.focus_pane(p, window, cx),
            (None, None) => {
                self.show_terminal(window, cx);
            }
        }
        cx.notify();
    }

    pub(super) fn show_terminal(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<TerminalPanel>> {
        let (_, ws) = self.tiling_scope()?;
        let id = self.next_pane;
        let ratio = self.terminal_ratio(window);
        let bounds = self.layout_bounds(cx);
        if !self.tiles.get_mut(&ws)?.add_terminal(id, ratio, bounds) {
            self.toast(Toast::error("No room for a terminal here: the panes would get too small.".to_string()));
            cx.notify();
            return None;
        }
        self.next_pane += 1;
        let panel = self.term_panel(id, &ws, window, cx);
        panel.update(cx, |p, cx| {
            p.ensure_open(window, cx);
            p.focus_active(window, cx);
        });
        self.save_layouts(cx);
        cx.notify();
        Some(panel)
    }

    pub(super) fn split_terminal(
        &mut self,
        target: PaneId,
        side: Direction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((_, ws)) = self.tiling_scope() else { return };
        let id = self.next_pane;
        let bounds = self.layout_bounds(cx);
        let Some(t) = self.tiles.get_mut(&ws) else { return };
        if !t.split_terminal(target, side, id, bounds) {
            self.toast(Toast::error("No room to split here: the panes would get too small.".to_string()));
            cx.notify();
            return;
        }
        self.next_pane += 1;
        let panel = Self::new_term_panel(&ws, window, cx);
        self.term_panes.insert(id, panel.clone());
        panel.update(cx, |p, cx| {
            p.ensure_open(window, cx);
            p.focus_active(window, cx);
        });
        self.save_layouts(cx);
        cx.notify();
    }

    pub(super) fn hide_terminal(&mut self, pane: PaneId, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, ws)) = self.tiling_scope() else { return };
        let Some(t) = self.tiles.get_mut(&ws) else { return };
        if t.close_pane(pane) != Some(Content::Terminal) {
            return;
        }
        if let Some(panel) = self.term_panes.remove(&pane) {
            self.keep_hidden(ws, panel, cx);
        }
        self.show_focused_pane(window, cx);
        self.save_layouts(cx);
        cx.notify();
    }

    pub(super) fn terminal_button(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.toggle_terminal(&ToggleTerminal, window, cx);
    }

    pub(super) fn render_terminal_pane(
        &mut self,
        pane: PaneId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let (_, ws) = self.tiling_scope()?;
        let panel = self.start_terminal_pane(pane, &ws, window, cx);
        Some(gpui::div().size_full().child(panel).into_any_element())
    }

    pub(super) fn start_terminal_pane(
        &mut self,
        pane: PaneId,
        ws: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<TerminalPanel> {
        let panel = self.term_panel(pane, ws, window, cx);
        panel.update(cx, |p, _| p.set_pane(pane));
        let tp = self.tiles.get(ws).and_then(|t| t.term_pane(pane)).cloned().unwrap_or_default();
        if panel.read(cx).is_empty() {
            match &tp.command {
                Some(name) => self.restart_command_pane(pane, name.clone(), ws.to_path_buf(), window, cx),
                None => match tp.history.clone().filter(|_| self.saved.terminal_history) {
                    Some(id) => self.restore_shell(pane, id, window, cx),
                    None => panel.update(cx, |p, cx| p.ensure_open(window, cx)),
                },
            }
        }
        panel
    }

    pub(super) fn migrate_terminal_dock(&mut self, ws: &Path, window: &Window, cx: &mut Context<Self>) {
        if !self.saved.terminal_open {
            return;
        }
        self.saved.terminal_open = false;
        self.save();
        let id = self.next_pane;
        let ratio = self.terminal_ratio(window);
        let bounds = self.layout_bounds(cx);
        let Some(t) = self.tiles.get_mut(ws) else { return };
        let focus = t.focus();
        if t.add_terminal(id, ratio, bounds) {
            self.next_pane += 1;
            if let Some(f) = focus {
                t.focus_pane(f);
            }
        }
    }

    pub(super) fn note_histories(&mut self, ws: &Path) {
        let Some(t) = self.tiles.get(ws) else { return };
        for p in t.terminals() {
            if let Some(id) = t.term_pane(p).and_then(|tp| tp.history.clone()) {
                self.term_histories.insert(p, (ws.to_path_buf(), id));
            }
        }
    }

    fn restore_shell(&mut self, pane: PaneId, id: String, window: &mut Window, cx: &mut Context<Self>) {
        if !self.command_starting.insert(pane) {
            return;
        }
        let task = cx.background_spawn(async move {
            let shell = kynex_core::terminal::resolve_shell(ShellKind::default_shell());
            shell.map(|mut s| {
                s.history = kynex_core::scrollback::load(&id);
                s
            })
        });
        cx.spawn_in(window, async move |this, cx| {
            let shell = task.await;
            this.update_in(cx, |this, window, cx| {
                this.command_starting.remove(&pane);
                let Some(panel) = this.term_panes.get(&pane).cloned() else { return };
                match shell {
                    Ok(s) => {
                        panel.update(cx, |p, cx| p.open_spec(s, window, cx));
                    }
                    Err(e) => panel.update(cx, |p, cx| p.show_error(e, cx)),
                }
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn prune_scrollback(&mut self, cx: &mut Context<Self>) {
        let projects: Vec<std::path::PathBuf> = self.saved.projects.iter().map(|p| p.path.clone()).collect();
        let mut keep: Vec<String> = self.term_histories.values().map(|(_, id)| id.clone()).collect();
        cx.background_spawn(async move {
            for p in projects {
                for layout in kynex_core::workspace::Workspaces::load(&p).layouts.values() {
                    let tools = layout.tools.iter().flat_map(|t| t.values());
                    keep.extend(tools.filter_map(|t| match t {
                        kynex_core::tiling::Tool::Terminal(tp) => tp.history.clone(),
                        _ => None,
                    }));
                }
            }
            kynex_core::scrollback::prune(&keep);
        })
        .detach();
    }

    pub(super) fn save_scrollback(&mut self, cx: &mut Context<Self>) {
        if !self.saved.terminal_history {
            return;
        }
        let mut writes: Vec<(String, String)> = Vec::new();
        for t in self.tiles.values_mut() {
            for p in t.terminals() {
                let Some(tp) = t.term_pane(p).filter(|tp| tp.is_shell()) else { continue };
                let Some(text) = self.term_panes.get(&p).and_then(|panel| panel.read(cx).history_text(cx)) else {
                    continue;
                };
                let id = tp.history.clone().unwrap_or_else(kynex_core::scrollback::new_id);
                t.set_history(p, id.clone());
                writes.push((id, text));
            }
        }
        for (id, text) in writes {
            if let Err(e) = kynex_core::scrollback::save(&id, &text) {
                eprintln!("kynex: scrollback: {e}");
            }
        }
    }

    fn keep_hidden(&mut self, cwd: std::path::PathBuf, panel: Entity<TerminalPanel>, cx: &Context<Self>) {
        if !panel.read(cx).is_empty() && !panel.read(cx).is_command() {
            self.terminals.push((cwd, panel));
        }
    }

    pub(super) fn drop_gone_terminals(&mut self, cx: &Context<Self>) {
        let live: Vec<PaneId> = self.tiles.values().flat_map(|t| t.terminals()).collect();
        self.command_starting.retain(|id| live.contains(id));
        let tracked = |ws: &Path| self.tile_projects.keys().any(|f| same_folder(f, ws));
        let gone: Vec<String> = self
            .term_histories
            .iter()
            .filter(|(p, (ws, _))| !live.contains(p) && tracked(ws))
            .map(|(_, (_, id))| id.clone())
            .collect();
        self.term_histories.retain(|p, _| live.contains(p));
        if !gone.is_empty() {
            cx.background_spawn(async move { gone.iter().for_each(|id| kynex_core::scrollback::remove(id)) }).detach();
        }
        let gone: Vec<PaneId> = self.term_panes.keys().copied().filter(|id| !live.contains(id)).collect();
        for id in gone {
            let Some(panel) = self.term_panes.remove(&id) else { continue };
            let cwd = panel.read(cx).cwd().to_path_buf();
            if self.tile_projects.keys().any(|f| same_folder(f, &cwd)) {
                self.keep_hidden(cwd, panel, cx);
            }
        }
    }

    pub(super) fn focus_tool_input(&self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some((_, ws)) = self.tiling_scope() else { return false };
        let Some(t) = self.tiles.get(&ws) else { return false };
        if !t.focus().is_some_and(|f| t.is_tool(f)) {
            return false;
        }
        let Some(panel) = self.focused_terminal() else {
            return true;
        };
        let handle = panel.read(cx).focus_handle(cx);
        if !handle.contains_focused(window, cx) {
            panel.clone().update(cx, |p, cx| p.focus_active(window, cx));
        }
        true
    }
}
