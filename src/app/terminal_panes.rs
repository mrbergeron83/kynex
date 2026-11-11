//! Terminals as panes of a workspace: each terminal pane draws a [`TerminalPanel`] (its
//! shells, in tabs). Ctrl+` shows one below everything in the tab, focuses it, or hides
//! it; a hidden one keeps its shells and comes back where it was asked for next. The play
//! buttons on the ```bash / ```powershell commands Claude suggests run there.

use std::path::Path;

use gpui::{AnyElement, Context, Entity, Focusable, Window, prelude::*};
use kynex_core::layout::{Direction, PaneId};
use kynex_core::terminal::ShellKind;
use kynex_core::tiling::{Content, TERMINAL_RATIO};
use kynex_core::toasts::Toast;
use kynex_core::workspace::same_folder;

use super::KynexApp;
use super::keys::ToggleTerminal;
use super::state::{Active, View};
use crate::terminal::{PanelEvent, TerminalPanel};

/// A terminal opened by Ctrl+` is never smaller than this, nor leaves less above it.
const MIN_HEIGHT: f32 = 120.;

impl KynexApp {
    /// The panel of terminal pane `pane`, in folder `cwd`: the one it has, else the folder's
    /// hidden one, else a new one.
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
        // A command's pane runs its own program, never the folder's hidden shells.
        let command =
            self.tiling_scope().and_then(|(_, ws, _)| self.tiles.get(&ws)?.command_of(pane).map(str::to_string));
        if command.is_some() {
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

    /// A panel with no shells yet in `cwd`. Closing its last shell hides its pane.
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

    /// Whether the tab on screen has a terminal, for the toolbar's button.
    pub(super) fn has_terminal(&self) -> bool {
        self.tiling_scope().and_then(|(_, ws, _)| self.tiles.get(&ws)).is_some_and(|t| !t.tab_terminals().is_empty())
    }

    /// The focused pane's panel, when a terminal has the focus.
    pub(super) fn focused_terminal(&self) -> Option<&Entity<TerminalPanel>> {
        let (_, ws, _) = self.tiling_scope()?;
        let focus = self.tiles.get(&ws)?.focus()?;
        self.term_panes.get(&focus)
    }

    /// How much of the tab is above a terminal shown with Ctrl+`: what the old dock's height
    /// leaves.
    fn terminal_ratio(&self, window: &Window) -> f32 {
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

    /// Ctrl+`: hides the focused terminal, else focuses the tab's terminal, else shows one.
    pub(super) fn toggle_terminal(&mut self, _: &ToggleTerminal, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, ws, _)) = self.tiling_scope() else { return };
        let Some(t) = self.tiles.get(&ws) else { return };
        let focus = t.focus();
        // A custom command's pane isn't the terminal Ctrl+` means.
        let shell = focus.filter(|f| t.is_terminal(*f) && t.command_of(*f).is_none());
        match (shell, t.tab_terminals().first().copied()) {
            (Some(f), _) => self.hide_terminal(f, window, cx),
            (None, Some(p)) => {
                self.focus_pane(p, window, cx);
                self.focus_input(window, cx);
            }
            (None, None) => {
                self.show_terminal(window, cx);
            }
        }
        cx.notify();
    }

    /// A terminal pane below everything in the tab on screen, focused, with a shell running.
    pub(super) fn show_terminal(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<TerminalPanel>> {
        let (_, ws, _) = self.tiling_scope()?;
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
        cx.notify();
        Some(panel)
    }

    /// A new terminal pane at `side` of pane `target`, focused.
    pub(super) fn split_terminal(
        &mut self,
        target: PaneId,
        side: Direction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((_, ws, _)) = self.tiling_scope() else { return };
        let id = self.next_pane;
        let bounds = self.layout_bounds(cx);
        let Some(t) = self.tiles.get_mut(&ws) else { return };
        if !t.split_terminal(target, side, id, bounds) {
            self.toast(Toast::error("No room to split here: the panes would get too small.".to_string()));
            cx.notify();
            return;
        }
        self.next_pane += 1;
        // A pane of its own gets shells of its own, not the folder's hidden ones.
        let panel = Self::new_term_panel(&ws, window, cx);
        self.term_panes.insert(id, panel.clone());
        panel.update(cx, |p, cx| {
            p.ensure_open(window, cx);
            p.focus_active(window, cx);
        });
        cx.notify();
    }

    /// Closes terminal pane `pane`. Its shells are kept for the folder's next terminal while
    /// any are running.
    pub(super) fn hide_terminal(&mut self, pane: PaneId, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, ws, _)) = self.tiling_scope() else { return };
        let Some(t) = self.tiles.get_mut(&ws) else { return };
        if t.close_pane(pane) != Some(Content::Terminal) {
            return;
        }
        if let Some(panel) = self.term_panes.remove(&pane) {
            self.keep_hidden(ws, panel, cx);
        }
        self.show_focused_pane(window, cx);
        self.focus_input(window, cx);
        cx.notify();
    }

    /// The toolbar's terminal button: from the Git view it switches to Code and shows a
    /// terminal; otherwise it does what Ctrl+` does.
    pub(super) fn terminal_button(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Active::Project(i) = self.saved.active
            && let Some(p) = self.saved.projects.get_mut(i)
            && p.view != View::Code
        {
            p.view = View::Code;
            self.save();
            self.show_terminal(window, cx);
            return;
        }
        self.toggle_terminal(&ToggleTerminal, window, cx);
    }

    /// Runs a command Claude suggested in the tab's terminal, showing one if needed.
    pub(super) fn run_in_terminal(
        &mut self,
        kind: ShellKind,
        command: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((_, ws, _)) = self.tiling_scope() else { return };
        let existing = self.tiles.get(&ws).and_then(|t| t.tab_terminals().first().copied());
        let panel = match existing {
            Some(p) => {
                self.focus_pane(p, window, cx);
                Some(self.term_panel(p, &ws, window, cx))
            }
            None => self.show_terminal(window, cx),
        };
        if let Some(panel) = panel {
            panel.update(cx, |p, cx| p.run(kind, &command, window, cx));
        }
        cx.notify();
    }

    /// Terminal pane `pane`'s body: its panel, with a shell started the first time it's
    /// drawn (a pane restored from last time).
    pub(super) fn render_terminal_pane(
        &mut self,
        pane: PaneId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let (_, ws, _) = self.tiling_scope()?;
        let panel = self.term_panel(pane, &ws, window, cx);
        if panel.read(cx).is_empty() {
            match self.tiles.get(&ws).and_then(|t| t.command_of(pane)).map(str::to_string) {
                Some(name) => self.restart_command_pane(pane, name, window, cx),
                None => panel.update(cx, |p, cx| p.ensure_open(window, cx)),
            }
        }
        Some(gpui::div().size_full().child(panel).into_any_element())
    }

    /// The old dock was open: the first workspace shown after the update gets its terminal
    /// back below everything, without the focus. Once: from then on the layout has it.
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

    /// Keeps `panel`, whose pane closed, for folder `cwd`'s next terminal if it has shells.
    fn keep_hidden(&mut self, cwd: std::path::PathBuf, panel: Entity<TerminalPanel>, cx: &Context<Self>) {
        // A command's program ends with its pane.
        if !panel.read(cx).is_empty() && !panel.read(cx).is_command() {
            self.terminals.push((cwd, panel));
        }
    }

    /// The panels of terminal panes no workspace has any more (a tab closed): hidden, like
    /// Ctrl+` leaves them, while their workspace is still around; else they end.
    pub(super) fn drop_gone_terminals(&mut self, cx: &Context<Self>) {
        let live: Vec<PaneId> = self.tiles.values().flat_map(|t| t.terminals()).collect();
        self.command_starting.retain(|id| live.contains(id));
        let gone: Vec<PaneId> = self.term_panes.keys().copied().filter(|id| !live.contains(id)).collect();
        for id in gone {
            let Some(panel) = self.term_panes.remove(&id) else { continue };
            let cwd = panel.read(cx).cwd().to_path_buf();
            if self.tile_projects.keys().any(|f| same_folder(f, &cwd)) {
                self.keep_hidden(cwd, panel, cx);
            }
        }
    }

    /// Where the keys go when a tool pane has the focus: a terminal's shell; the Git view
    /// and the timeline keep what they have, short of the message box. False when a session
    /// pane has the focus.
    pub(super) fn focus_tool_input(&self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some((_, ws, _)) = self.tiling_scope() else { return false };
        let Some(t) = self.tiles.get(&ws) else { return false };
        if !t.focus().is_some_and(|f| t.is_tool(f)) {
            return false;
        }
        let Some(panel) = self.focused_terminal() else {
            if self.pane.composer.read(cx).is_focused(window) {
                self.focus.focus(window, cx);
            }
            return true;
        };
        let handle = panel.read(cx).focus_handle(cx);
        let focused = handle.contains_focused(window, cx);
        if !focused {
            panel.clone().update(cx, |p, cx| p.focus_active(window, cx));
        }
        true
    }
}
