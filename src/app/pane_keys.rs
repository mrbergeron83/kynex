//! The prefix keys (Ctrl+B, then a key; `keys.rs` lists them): moving between, swapping,
//! zooming and resizing panes, pane tabs, workspaces and projects.

use std::path::PathBuf;

use gpui::{Anchor, Context, KeystrokeEvent, Stateful, Window, point, prelude::*};
use kynex_core::layout::{Axis, Direction};
use kynex_core::toasts::Toast;
use kynex_core::workspace;

use super::KynexApp;
use super::keys::{self, FocusPane, JumpPaneTab, SwapPane};
use super::menus::Menu;
use super::state::Active;

/// How far one press of a resize key moves a pane's edge, in logical pixels.
const RESIZE_STEP: f32 = 40.0;

/// Where resize mode was started: the workspace, its focused pane and tab, the project tab.
/// Moving anywhere else ends it.
pub(super) type ResizePlace = (PathBuf, kynex_core::layout::PaneId, usize, Active);

impl KynexApp {
    /// `root` with the prefix keys' actions: each to the method that runs it.
    pub(super) fn with_pane_actions(&self, root: Stateful<gpui::Div>, cx: &mut Context<Self>) -> Stateful<gpui::Div> {
        root
            // Only a terminal does anything with it (see `terminal::view`).
            .on_action(cx.listener(|_, _: &keys::SendPrefix, _, _| {}))
            .on_action(cx.listener(|this, _: &keys::SplitRight, w, cx| this.split_pane(Axis::Row, w, cx)))
            .on_action(cx.listener(|this, _: &keys::SplitDown, w, cx| this.split_pane(Axis::Column, w, cx)))
            .on_action(cx.listener(|this, a: &FocusPane, w, cx| this.focus_pane_toward(a.0, w, cx)))
            .on_action(cx.listener(|this, a: &SwapPane, w, cx| this.swap_pane_toward(a.0, w, cx)))
            .on_action(cx.listener(|this, _: &keys::NextPane, w, cx| this.step_pane(1, w, cx)))
            .on_action(cx.listener(|this, _: &keys::ZoomPane, w, cx| this.toggle_pane_zoom(w, cx)))
            .on_action(cx.listener(|this, _: &keys::ClosePane, w, cx| this.close_pane(w, cx)))
            .on_action(cx.listener(Self::close_pane_or_tab))
            .on_action(cx.listener(Self::start_resize_mode))
            .on_action(cx.listener(|this, _: &keys::NewPaneTab, w, cx| this.new_pane_tab(w, cx)))
            .on_action(cx.listener(|this, _: &keys::NextPaneTab, w, cx| this.step_pane_tab(1, w, cx)))
            .on_action(cx.listener(|this, _: &keys::PrevPaneTab, w, cx| this.step_pane_tab(-1, w, cx)))
            .on_action(cx.listener(|this, a: &JumpPaneTab, w, cx| this.select_pane_tab(a.0.saturating_sub(1), w, cx)))
            .on_action(cx.listener(Self::rename_active_pane_tab))
            .on_action(cx.listener(|this, _: &keys::ClosePaneTab, w, cx| this.close_active_pane_tab(w, cx)))
            .on_action(cx.listener(|this, _: &keys::PickWorkspace, w, cx| this.open_picker(Menu::Workspaces, w, cx)))
            .on_action(cx.listener(|this, _: &keys::NewWorkspace, w, cx| this.open_new_workspace(w, cx)))
            .on_action(cx.listener(Self::close_workspace))
            .on_action(cx.listener(|this, _: &keys::KeyboardSettings, _, cx| this.open_settings_on(2, cx)))
            .on_action(cx.listener(|this, _: &keys::PickProject, w, cx| this.open_picker(Menu::Projects, w, cx)))
    }

    /// A picker menu, at the top middle of the window.
    fn open_picker(&mut self, which: Menu, window: &mut Window, cx: &mut Context<Self>) {
        let at = point(window.viewport_size().width / 2.0, gpui::px(96.));
        self.menu = None;
        self.menu_pick = 0;
        self.open_menu(which, at, Anchor::TopCenter, cx);
    }

    /// Focuses the pane next to the focused one, `dir` of it.
    fn focus_pane_toward(&mut self, dir: Direction, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, cwd, _)) = self.tiling_scope() else { return };
        let bounds = self.layout_bounds(cx);
        if self.tiles.get_mut(&cwd).is_some_and(|t| t.focus_dir(dir, bounds)) {
            self.show_focused_pane(window, cx);
            self.focus_input(window, cx);
        }
    }

    /// Swaps the focused pane with the one `dir` of it; the focus goes with it.
    fn swap_pane_toward(&mut self, dir: Direction, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, cwd, _)) = self.tiling_scope() else { return };
        let bounds = self.layout_bounds(cx);
        if self.tiles.get_mut(&cwd).is_some_and(|t| t.swap_dir(dir, bounds)) {
            self.focus_input(window, cx);
            cx.notify();
        }
    }

    /// Ctrl+W in a pane: closes it while its tab has others, else the project tab as ever.
    fn close_pane_or_tab(&mut self, _: &keys::ClosePaneOrTab, window: &mut Window, cx: &mut Context<Self>) {
        let several = self
            .tiling_scope()
            .and_then(|(_, cwd, _)| self.tiles.get(&cwd).map(|t| t.visible().len() > 1))
            .unwrap_or(false);
        if several {
            self.close_pane(window, cx);
        } else {
            cx.propagate();
        }
    }

    /// Renames the tab that's showing (the tab bar is there once there are two).
    fn rename_active_pane_tab(&mut self, _: &keys::RenamePaneTab, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, cwd, _)) = self.tiling_scope() else { return };
        let Some(tabs) = self.tiles.get(&cwd).map(|t| t.tabs()).filter(|t| t.len() > 1) else { return };
        if let Some(at) = tabs.iter().position(|t| t.active) {
            self.start_tab_rename(at, tabs[at].name.clone(), window, cx);
        }
    }

    /// Removes the workspace you're in, asking first when it has work in it.
    fn close_workspace(&mut self, _: &keys::CloseWorkspace, _: &mut Window, cx: &mut Context<Self>) {
        let (Some(project), Some(cwd)) = (self.workspace_project(), self.workspace_cwd()) else { return };
        if workspace::same_folder(&project, &cwd) {
            self.toast(Toast::warning("The project's own folder can't be removed as a workspace."));
            return cx.notify();
        }
        self.remove_workspace(cwd, cx);
    }

    /// Whether the tab that's showing has several panes on screen.
    fn has_several_panes(&self) -> bool {
        self.tiling_scope().and_then(|(_, cwd, _)| self.tiles.get(&cwd).map(|t| t.visible().len() > 1)).unwrap_or(false)
    }

    /// Where the focus is now, as far as resize mode cares.
    fn resize_place(&self) -> Option<ResizePlace> {
        let (_, cwd, _) = self.tiling_scope()?;
        let t = self.tiles.get(&cwd)?;
        let tab = t.tabs().iter().position(|t| t.active)?;
        Some((cwd, t.focus()?, tab, self.saved.active))
    }

    /// Each frame: resize mode ends once the focused pane, tab, workspace or project changed
    /// (by mouse too).
    pub(super) fn sync_resize_mode(&mut self) {
        if self.resize_mode.is_some() && self.resize_mode != self.resize_place() {
            self.resize_mode = None;
        }
    }

    /// Resize mode: h / j / k / l or the arrows grow the focused pane that way.
    fn start_resize_mode(&mut self, _: &keys::ResizeMode, _: &mut Window, cx: &mut Context<Self>) {
        if !self.has_several_panes() {
            return;
        }
        self.resize_mode = self.resize_place();
        self.toast(Toast::info("Resize: h j k l or arrows, Esc when done"));
        cx.notify();
    }

    /// Resize mode's keys, before any binding or the message box sees them (Esc, Enter and
    /// the arrows are all bound to something else). Other keys leave the mode and go on.
    pub(super) fn watch_resize_keys(cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        cx.intercept_keystrokes(move |ev, window, cx| {
            this.update(cx, |this, cx| {
                let took = if this.resize_mode.is_some() {
                    this.resize_key(ev, window, cx)
                } else {
                    this.picker_key(ev, window, cx)
                };
                if took {
                    this.resize_swallow = true;
                    cx.stop_propagation();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Whether resize mode took the key.
    fn resize_key(&mut self, ev: &KeystrokeEvent, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let k = &ev.keystroke;
        if matches!(k.key.as_str(), "shift" | "control" | "alt" | "platform" | "function") {
            return false;
        }
        let m = k.modifiers;
        let plain = !(m.control || m.alt || m.platform || m.function);
        let dir = match k.key.as_str() {
            "h" | "left" => Some(Direction::Left),
            "j" | "down" => Some(Direction::Down),
            "k" | "up" => Some(Direction::Up),
            "l" | "right" => Some(Direction::Right),
            _ => None,
        };
        let Some((_, cwd, _)) = self.tiling_scope().filter(|_| self.has_several_panes()) else {
            self.resize_mode = None;
            return false;
        };
        if let Some(dir) = dir.filter(|_| plain) {
            let bounds = self.layout_bounds(cx);
            if let Some(t) = self.tiles.get_mut(&cwd) {
                t.resize_focused(dir, RESIZE_STEP, bounds);
            }
            cx.notify();
            return true;
        }
        self.resize_mode = None;
        cx.notify();
        if plain && matches!(k.key.as_str(), "escape" | "enter") {
            self.focus_input(window, cx);
            return true;
        }
        false
    }

    /// ↑ / ↓ (or k / j) and Enter in the workspace and project pickers; Esc closes any menu.
    fn picker_key(&mut self, ev: &KeystrokeEvent, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(which) = self.open_picker_menu() else { return false };
        let k = &ev.keystroke;
        let m = k.modifiers;
        if m.control || m.alt || m.platform || m.function {
            return false;
        }
        let workspaces = if which == Menu::Workspaces {
            self.workspace_project().map(|p| self.workspaces_of(&p, cx)).unwrap_or_default()
        } else {
            Vec::new()
        };
        let rows = if which == Menu::Workspaces { workspaces.len() } else { self.saved.projects.len() + 1 };
        match k.key.as_str() {
            "up" | "k" => self.menu_pick = self.menu_pick.saturating_sub(1),
            "down" | "j" => self.menu_pick = (self.menu_pick + 1).min(rows.saturating_sub(1)),
            "enter" => {
                let at = self.menu_pick;
                if which == Menu::Workspaces {
                    if let Some((c, _)) = workspaces.get(at) {
                        self.menu = None;
                        self.select_workspace(&c.path, cx);
                        self.focus_input(window, cx);
                    }
                } else if at < self.saved.projects.len() {
                    self.jump_to_project(at, window, cx);
                } else {
                    self.menu = None;
                    self.pick_project(cx);
                }
            }
            _ => return false,
        }
        cx.notify();
        true
    }

    /// A key resize mode took has done its work; the views under it don't get it too.
    pub(super) fn swallow_resize_key(&mut self, _: &gpui::KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        if std::mem::take(&mut self.resize_swallow) {
            cx.stop_propagation();
        }
    }

    /// The project picker's rows: each open project to jump to.
    pub(super) fn jump_to_project(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        self.select(Active::Project(i), window, cx);
        self.focus_input(window, cx);
    }
}
