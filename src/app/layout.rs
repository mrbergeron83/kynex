//! Top-level workspace layout: toolbar, tiled panes (or an empty-state
//! prompt to open a project), overlays, shortcuts, and divider resize drags.

use gpui::{AnyElement, Context, FontWeight, MouseButton, MouseMoveEvent, Stateful, Window, div, prelude::*};

use super::KynexApp;
use super::keys;
use crate::theme::theme;
use crate::ui::mark::mark;
use crate::ui::{Lamp, primary_button, px};

impl KynexApp {
    /// The main area below the title bar: either an empty-state welcome screen when no
    /// projects are open, or the active project's toolbar and tiled panes.
    pub(super) fn render_code_main(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx).clone();
        if self.active_project().is_none() {
            return self.render_no_projects(cx);
        }
        let body = self.render_panes(window, cx).unwrap_or_else(|| div().flex_1().into_any_element());
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .bg(t.editor)
            .child(self.render_toolbar(cx))
            .child(body)
            .into_any_element()
    }

    fn render_no_projects(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx).clone();
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .items_center()
            .justify_center()
            .gap_4()
            .bg(t.editor)
            .child(mark("welcome-mark", gpui::px(56.), Lamp::Idle, cx))
            .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child("Open a project folder"))
            .child(
                div()
                    .max_w(px(400.))
                    .text_center()
                    .text_sm()
                    .text_color(t.text_muted)
                    .child("Kynex runs Claude Code, Antigravity, Grok, and other agent CLIs side by side in GPU-accelerated terminal panes."),
            )
            .child(
                primary_button("welcome-open-project", "Open project…", cx)
                    .on_click(cx.listener(|this, _, _, cx| this.pick_project(cx))),
            )
            .into_any_element()
    }

    /// `root` with toasts, popups, modals, palette, and context menus over it.
    pub(super) fn with_overlays(
        &mut self,
        root: Stateful<gpui::Div>,
        toasts: Option<AnyElement>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<gpui::Div> {
        let menu = self.draw_leaving(|s| &mut s.menu, |e| &mut e.menu, window, cx, |s, l, _, cx| s.render_menu(l, cx));
        let app_settings =
            self.draw_leaving(|s| &mut s.app_settings, |e| &mut e.app_settings, window, cx, Self::render_app_settings);
        let about = self.render_about(window, cx);
        let palette = self.draw_leaving(|s| &mut s.palette, |e| &mut e.palette, window, cx, Self::render_palette);
        let git_file =
            self.draw_leaving(|s| &mut s.git_file, |e| &mut e.git_file, window, cx, Self::render_git_file_modal);
        let popup = self.render_command_popup(cx);
        root.children(toasts)
            .children(popup)
            .children(git_file)
            .children(app_settings)
            .children(about)
            .children(palette)
            .children(menu)
    }

    pub(super) fn with_resize_drag(&self, root: Stateful<gpui::Div>, cx: &mut Context<Self>) -> Stateful<gpui::Div> {
        root.when(self.tile_drag.is_some(), |d| {
            d.on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| this.drag_tiles(ev.position, cx)))
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.tile_drag = None;
                        this.save_layouts(cx);
                        cx.notify();
                    }),
                )
        })
    }

    /// `root` with the window's shortcuts.
    pub(super) fn with_actions(&self, root: Stateful<gpui::Div>, cx: &mut Context<Self>) -> Stateful<gpui::Div> {
        let root = root
            .on_action(
                cx.listener(|this, a: &keys::OpenFileLink, w, cx| this.open_file_view(a.path.clone(), a.line, w, cx)),
            )
            .on_action(cx.listener(Self::toggle_palette))
            .on_action(cx.listener(Self::open_project))
            .on_action(cx.listener(Self::open_settings_action))
            .on_action(cx.listener(|_, _: &keys::Quit, _, cx| cx.quit()))
            .on_action(cx.listener(|this, _: &keys::ZoomIn, _, cx| this.zoom_by(1, cx)))
            .on_action(cx.listener(|this, _: &keys::ZoomOut, _, cx| this.zoom_by(-1, cx)))
            .on_action(cx.listener(|this, _: &keys::ZoomReset, _, cx| this.zoom_by(0, cx)))
            .on_action(cx.listener(Self::new_session_action))
            .on_action(cx.listener(Self::next_session))
            .on_action(cx.listener(Self::prev_session))
            .on_action(cx.listener(Self::close_tab))
            .on_action(cx.listener(Self::next_workspace))
            .on_action(cx.listener(Self::prev_workspace))
            .on_action(cx.listener(Self::next_tab))
            .on_action(cx.listener(Self::prev_tab))
            .on_action(cx.listener(Self::move_tab_left))
            .on_action(cx.listener(Self::move_tab_right))
            .on_action(cx.listener(Self::toggle_sessions))
            .on_action(cx.listener(Self::interrupt_action))
            .on_action(cx.listener(Self::toggle_terminal))
            .on_action(cx.listener(Self::toggle_git))
            .on_action(cx.listener(Self::toggle_agents))
            .on_action(cx.listener(Self::run_command_action));
        self.with_pane_actions(root, cx)
    }

    /// What each frame settles before drawing.
    pub(super) fn sync_before_render(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sync_tiles(window, cx);
        self.sync_git_pane(window, cx);
        self.sync_pane_mode();
        self.sync_awake();
        self.sync_toasts(window, cx);
        self.load_project_settings();
        if window.focused(cx).is_none() {
            self.focus.focus(window, cx);
        }
    }
}
