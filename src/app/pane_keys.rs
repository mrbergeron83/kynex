//! The prefix keys (Ctrl+B, then a key): moving between, swapping, zooming and resizing
//! panes, workspaces and projects.

use std::path::PathBuf;

use gpui::{Anchor, Context, KeystrokeEvent, Stateful, Window, point, prelude::*};
use kynex_core::layout::{Axis, Direction};
use kynex_core::toasts::Toast;
use kynex_core::workspace;

use super::KynexApp;
use super::keys::{self, FocusPane, SwapPane};
use super::menus::Menu;

pub(super) const RESIZE_STEP: f32 = 40.0;

/// Where resize mode was started: workspace, focused pane, active project index.
pub(super) type ResizePlace = (PathBuf, kynex_core::layout::PaneId, Option<usize>);

impl KynexApp {
    pub(super) fn with_pane_actions(&self, root: Stateful<gpui::Div>, cx: &mut Context<Self>) -> Stateful<gpui::Div> {
        root.on_action(cx.listener(|_, _: &keys::SendPrefix, _, _| {}))
            .on_action(cx.listener(|_, _: &keys::CopyMode, _, _| {}))
            .on_action(cx.listener(|this, _: &keys::SplitRight, w, cx| this.split_pane(Axis::Row, w, cx)))
            .on_action(cx.listener(|this, _: &keys::SplitDown, w, cx| this.split_pane(Axis::Column, w, cx)))
            .on_action(cx.listener(|this, a: &FocusPane, w, cx| this.focus_pane_toward(a.0, w, cx)))
            .on_action(cx.listener(|this, a: &SwapPane, w, cx| this.swap_pane_toward(a.0, w, cx)))
            .on_action(cx.listener(|this, _: &keys::NextPane, w, cx| this.step_pane(1, w, cx)))
            .on_action(cx.listener(|this, _: &keys::ZoomPane, w, cx| this.toggle_pane_zoom(w, cx)))
            .on_action(cx.listener(|this, _: &keys::ClosePane, w, cx| this.close_pane(w, cx)))
            .on_action(cx.listener(Self::close_pane_or_tab))
            .on_action(cx.listener(Self::start_resize_mode))
            .on_action(cx.listener(|this, _: &keys::PickWorkspace, w, cx| this.open_picker(Menu::Workspaces, w, cx)))
            .on_action(cx.listener(|this, _: &keys::NewWorkspace, w, cx| this.open_new_workspace(w, cx)))
            .on_action(cx.listener(Self::close_workspace))
            .on_action(cx.listener(|this, _: &keys::KeyboardSettings, _, cx| {
                this.open_settings_on(super::app_settings::KEYBOARD_SECTION, cx)
            }))
            .on_action(cx.listener(Self::next_needs_you))
            .on_action(cx.listener(|this, _: &keys::UndoLayout, w, cx| this.undo_layout(w, cx)))
            .on_action(cx.listener(|this, _: &keys::RedoLayout, w, cx| this.redo_layout(w, cx)))
            .on_action(cx.listener(|this, _: &keys::ReopenPane, w, cx| this.reopen_pane(w, cx)))
            .on_action(cx.listener(|this, _: &keys::LayoutSnapshots, w, cx| this.open_picker(Menu::Layouts, w, cx)))
            .on_action(cx.listener(|this, _: &keys::PickProject, w, cx| this.open_picker(Menu::Projects, w, cx)))
    }

    pub(super) fn open_picker(&mut self, which: Menu, window: &mut Window, cx: &mut Context<Self>) {
        let at = point(window.viewport_size().width / 2.0, gpui::px(96.));
        self.menu = None;
        self.menu_pick = 0;
        self.open_menu(which, at, Anchor::TopCenter, cx);
    }

    fn focus_pane_toward(&mut self, dir: Direction, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, cwd)) = self.tiling_scope() else { return };
        let bounds = self.layout_bounds(cx);
        if self.tiles.get_mut(&cwd).is_some_and(|t| t.focus_dir(dir, bounds)) {
            self.show_focused_pane(window, cx);
        }
    }

    fn swap_pane_toward(&mut self, dir: Direction, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, cwd)) = self.tiling_scope() else { return };
        let bounds = self.layout_bounds(cx);
        if self.tiles.get_mut(&cwd).is_some_and(|t| t.swap_dir(dir, bounds)) {
            self.focus_input(window, cx);
            self.save_layouts(cx);
            cx.notify();
        }
    }

    fn close_pane_or_tab(&mut self, _: &keys::ClosePaneOrTab, window: &mut Window, cx: &mut Context<Self>) {
        let several = self
            .tiling_scope()
            .and_then(|(_, cwd)| self.tiles.get(&cwd).map(|t| t.visible_panes().len() > 1))
            .unwrap_or(false);
        if several {
            self.close_pane(window, cx);
        } else {
            cx.propagate();
        }
    }

    fn close_workspace(&mut self, _: &keys::CloseWorkspace, _: &mut Window, cx: &mut Context<Self>) {
        let (Some(project), Some(cwd)) = (self.workspace_project(), self.workspace_cwd()) else { return };
        if workspace::same_folder(&project, &cwd) {
            self.toast(Toast::warning("The project's own folder can't be removed as a workspace."));
            return cx.notify();
        }
        self.remove_workspace(cwd, cx);
    }

    fn has_several_panes(&self) -> bool {
        self.tiling_scope()
            .and_then(|(_, cwd)| self.tiles.get(&cwd).map(|t| t.visible_panes().len() > 1))
            .unwrap_or(false)
    }

    fn resize_place(&self) -> Option<ResizePlace> {
        let (_, cwd) = self.tiling_scope()?;
        let t = self.tiles.get(&cwd)?;
        Some((cwd, t.focus()?, self.saved.active_project))
    }

    pub(super) fn sync_resize_mode(&mut self) {
        if self.resize_mode.is_some() && self.resize_mode != self.resize_place() {
            self.resize_mode = None;
        }
    }

    fn start_resize_mode(&mut self, _: &keys::ResizeMode, _: &mut Window, cx: &mut Context<Self>) {
        if !self.has_several_panes() {
            return;
        }
        self.resize_mode = self.resize_place();
        self.toast(Toast::info("Resize: h j k l or arrows, Esc when done"));
        cx.notify();
    }

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
        let Some((_, cwd)) = self.tiling_scope().filter(|_| self.has_several_panes()) else {
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
        self.save_layouts(cx);
        cx.notify();
        if plain && matches!(k.key.as_str(), "escape" | "enter") {
            self.focus_input(window, cx);
            return true;
        }
        false
    }

    fn picker_key(&mut self, ev: &KeystrokeEvent, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(which) = self.open_picker_menu() else { return false };
        if self.layout_name.is_some() {
            return false;
        }
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
        let snapshots = if which == Menu::Layouts { self.layout_entries() } else { Vec::new() };
        let rows = match which {
            Menu::Workspaces => workspaces.len(),
            Menu::Layouts => snapshots.len(),
            _ => self.saved.projects.len() + 1,
        };
        match k.key.as_str() {
            "up" | "k" => self.menu_pick = self.menu_pick.saturating_sub(1),
            "down" | "j" => self.menu_pick = (self.menu_pick + 1).min(rows.saturating_sub(1)),
            "enter" => {
                let at = self.menu_pick;
                if which == Menu::Layouts {
                    if let Some(s) = snapshots.get(at) {
                        self.menu = None;
                        self.restore_layout(s, window, cx);
                    }
                } else if which == Menu::Workspaces {
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

    pub(super) fn swallow_resize_key(&mut self, _: &gpui::KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        if std::mem::take(&mut self.resize_swallow) {
            cx.stop_propagation();
        }
    }

    pub(super) fn jump_to_project(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        self.select_project(i, window, cx);
    }
}
