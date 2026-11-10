//! The window's layout pieces `render` stacks: the Code view, the Git view,
//! the overlays over them, and the drag that resizes the sessions panel and
//! the terminal.

use gpui::{AnyElement, Context, MouseButton, MouseMoveEvent, Stateful, Window, div, prelude::*};

use super::KynexApp;
use super::keys::{self, CopySelection, copy_selection};
use super::state::{Active, View};
use super::transcript::Scroll;
use crate::theme::theme;
use crate::ui::motion::{self, fade_in};

impl KynexApp {
    /// The Git view beside the sessions panel.
    pub(super) fn render_git_main(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx).clone();
        let path = self.active_project().map(|p| p.path.clone()).unwrap();
        let git = self.git_view(&path, cx);
        // The sessions panel stays, so the toolbar (and its Code / Git switch)
        // doesn't move when the view changes.
        div()
            .flex()
            .flex_1()
            .min_h_0()
            .children(self.render_sessions_column(window, cx))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .bg(t.editor)
                    .child(self.render_toolbar(cx))
                    // Fades in when you switch to it, as the transcript does.
                    .child(fade_in(
                        ("git-view-in", git.entity_id().as_u64() as usize),
                        div().flex_1().min_h_0().child(git),
                        motion::NORMAL,
                    )),
            )
            .into_any_element()
    }

    /// The conversation: header, transcript, composer, timeline and terminal.
    pub(super) fn render_code_main(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx).clone();
        let key = self.focused_key();
        div()
            .flex()
            .flex_1()
            .min_h_0()
            .children(self.render_sessions_column(window, cx))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .bg(t.editor)
                    .child(self.render_toolbar(cx))
                    .children(key.and_then(|key| self.render_session_header(key, cx)))
                    .children(self.render_reporter(window, cx))
                    // The timeline runs the full height beside the transcript
                    // and the composer under it, down to the terminal dock.
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_h_0()
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .min_w_0()
                                    .child(
                                        div()
                                            .relative()
                                            .flex()
                                            .flex_col()
                                            .flex_1()
                                            .min_h_0()
                                            .child(match key {
                                                Some(key) => self.render_thread(key, window, cx),
                                                None => self.render_empty_thread(cx),
                                            })
                                            .children(self.render_find(cx))
                                            .child(self.toast_room_probe()),
                                    )
                                    .child(self.render_composer(key, window, cx)),
                            )
                            .children(key.and_then(|key| self.render_timeline(key, cx))),
                    )
                    .children(self.render_terminal_dock(window, cx)),
            )
            .into_any_element()
    }

    /// `root` with the toasts, modals, palette and menu over it.
    pub(super) fn with_overlays(
        &mut self,
        root: Stateful<gpui::Div>,
        toasts: Option<AnyElement>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<gpui::Div> {
        let menu = self.draw_leaving(|s| &mut s.menu, |e| &mut e.menu, window, cx, |s, l, _, cx| s.render_menu(l, cx));
        let settings = self.draw_leaving(|s| &mut s.settings, |e| &mut e.settings, window, cx, Self::render_settings);
        let app_settings =
            self.draw_leaving(|s| &mut s.app_settings, |e| &mut e.app_settings, window, cx, Self::render_app_settings);
        let extensions =
            self.draw_leaving(|s| &mut s.extensions, |e| &mut e.extensions, window, cx, Self::render_extensions);
        let about = self.render_about(window, cx);
        let palette = self.draw_leaving(|s| &mut s.palette, |e| &mut e.palette, window, cx, Self::render_palette);
        let git_modal =
            self.draw_leaving(|s| &mut s.git_modal, |e| &mut e.git_modal, window, cx, Self::render_git_modal);
        let scratch = self.draw_leaving(|s| &mut s.scratch, |e| &mut e.scratch, window, cx, Self::render_scratch_modal);
        let viewer =
            self.draw_leaving(|s| &mut s.viewer, |e| &mut e.viewer, window, cx, |s, l, _, _| s.render_image_viewer(l));
        root.children(toasts)
            .children(git_modal)
            .children(scratch)
            .children(settings)
            .children(app_settings)
            .children(extensions)
            .children(about)
            .children(palette)
            .children(menu)
            .children(viewer)
    }

    /// `root` while the sessions panel's or the terminal's edge is dragged:
    /// the pointer moves it, letting go keeps it.
    pub(super) fn with_resize_drag(&self, root: Stateful<gpui::Div>, cx: &mut Context<Self>) -> Stateful<gpui::Div> {
        root.when(self.resizing_sessions, |d| {
            d.cursor_col_resize()
                .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                    this.saved.sessions_width = (f32::from(ev.position.x) / this.saved.zoom).clamp(180., 480.);
                    cx.notify();
                }))
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.resizing_sessions = false;
                        this.save();
                        cx.notify();
                    }),
                )
        })
        .when(self.resizing_terminal, |d| {
            d.cursor_row_resize()
                .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, window, cx| {
                    this.drag_terminal_edge(f32::from(ev.position.y), window);
                    cx.notify();
                }))
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.resizing_terminal = false;
                        this.save();
                        cx.notify();
                    }),
                )
        })
    }

    /// `root` with the window's shortcuts: each action to the method that runs it.
    pub(super) fn with_actions(&self, root: Stateful<gpui::Div>, cx: &mut Context<Self>) -> Stateful<gpui::Div> {
        root.on_action(cx.listener(Self::toggle_palette))
            .on_action(cx.listener(Self::open_find))
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
            .on_action(cx.listener(Self::next_context))
            .on_action(cx.listener(Self::prev_context))
            .on_action(cx.listener(Self::next_tab))
            .on_action(cx.listener(Self::prev_tab))
            .on_action(cx.listener(Self::move_tab_left))
            .on_action(cx.listener(Self::move_tab_right))
            .on_action(cx.listener(Self::toggle_sessions))
            .on_action(cx.listener(Self::cycle_mode))
            .on_action(cx.listener(Self::interrupt_action))
            .on_action(cx.listener(Self::toggle_terminal))
            .on_action(cx.listener(Self::search_history))
            .on_action(cx.listener(Self::stash_action))
            .on_action(cx.listener(Self::pick_model))
            .on_action(cx.listener(Self::expand_tools))
            .on_action(cx.listener(Self::edit_in_editor_action))
            .on_action(cx.listener(Self::rewind_action))
            .on_action(cx.listener(Self::toggle_timeline))
            .on_action(cx.listener(Self::prev_turn))
            .on_action(cx.listener(Self::next_turn))
            .on_action(cx.listener(|this, _: &keys::PageUp, _, cx| this.scroll_keys(Scroll::Page(-1.), cx)))
            .on_action(cx.listener(|this, _: &keys::PageDown, _, cx| this.scroll_keys(Scroll::Page(1.), cx)))
            .on_action(cx.listener(|this, _: &keys::LineUp, _, cx| this.scroll_keys(Scroll::Line(-1.), cx)))
            .on_action(cx.listener(|this, _: &keys::LineDown, _, cx| this.scroll_keys(Scroll::Line(1.), cx)))
            .on_action(cx.listener(|this, _: &keys::ScrollTop, _, cx| this.scroll_keys(Scroll::Top, cx)))
            .on_action(cx.listener(|this, _: &keys::ScrollBottom, _, cx| this.scroll_keys(Scroll::Bottom, cx)))
            .on_action(cx.listener(|_, _: &CopySelection, _, cx| copy_selection(cx)))
            // The composer's Ctrl+C, passed on when nothing is selected in it.
            .on_action(cx.listener(|_, _: &crate::ui::input::Copy, _, cx| copy_selection(cx)))
    }

    /// What each frame settles before drawing; returns the project's view.
    pub(super) fn sync_before_render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<View> {
        if let Active::Project(i) = self.saved.active
            && let Some(p) = self.saved.projects.get_mut(i).filter(|p| p.view == View::Settings)
        {
            p.view = View::Code;
        }
        let view = self.active_project().map(|p| p.view);
        // Create (or resume) the project's session as soon as its tab is shown,
        // so a restored transcript is visible before the first message.
        // Showing it counts as seeing it finish, while Kynex is in front.
        let active = window.is_window_active();
        if view != Some(View::Git)
            && let Some(s) = self.session_mut()
            && active
        {
            s.finished = false;
        }
        self.sync_attention(cx);
        self.sync_awake();
        let key = self.focused_key();
        self.sync_draft(key, cx);
        self.sync_prompt(key, window, cx);
        self.sync_toasts(window, cx);
        self.load_project_settings();
        // Shortcuts (Esc, Ctrl+B…) only reach the workspace through the focused
        // element, so never leave the window with nothing focused.
        if window.focused(cx).is_none() {
            self.focus.focus(window, cx);
        }
        view
    }
}
