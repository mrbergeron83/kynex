//! The terminal under the conversation: one [`TerminalPanel`] per project,
//! shown below the composer with a draggable top edge, toggled with Ctrl+`
//! and fed by the play buttons on Bash / PowerShell tool calls.

use std::path::{Path, PathBuf};

use gpui::{Context, Entity, Focusable, MouseButton, Window, div, prelude::*, px};
use kynex_core::terminal::ShellKind;

use super::KynexApp;
use super::keys::ToggleTerminal;
use super::state::{Active, View};
use crate::terminal::{PanelEvent, TerminalPanel};
use crate::theme::theme;

/// The panel never shrinks below this, nor squeezes the conversation below it.
const MIN_HEIGHT: f32 = 120.;
const MIN_CONVERSATION: f32 = 220.;

impl KynexApp {
    /// The project folder the terminal follows; Chat has none.
    fn terminal_cwd(&self) -> Option<PathBuf> {
        self.active_project().map(|p| p.path.clone())
    }

    fn terminal_panel(&mut self, cwd: &Path, cx: &mut Context<Self>) -> Entity<TerminalPanel> {
        if let Some(p) = self.terminals.get(cwd) {
            return p.clone();
        }
        let panel = cx.new(|cx| TerminalPanel::new(cwd.to_path_buf(), cx));
        cx.subscribe(&panel, |this, _, ev: &PanelEvent, cx| match ev {
            PanelEvent::Hide => {
                this.saved.terminal_open = false;
                this.save();
                cx.notify();
            }
        })
        .detach();
        self.terminals.insert(cwd.to_path_buf(), panel.clone());
        panel
    }

    pub(super) fn toggle_terminal(&mut self, _: &ToggleTerminal, window: &mut Window, cx: &mut Context<Self>) {
        let Some(cwd) = self.terminal_cwd() else { return };
        self.saved.terminal_open = !self.saved.terminal_open;
        self.save();
        if self.saved.terminal_open {
            let panel = self.terminal_panel(&cwd, cx);
            panel.update(cx, |p, cx| {
                p.ensure_open(window, cx);
                p.focus_active(window, cx);
            });
        } else {
            self.composer.read(cx).focus_handle(cx).focus(window, cx);
        }
        cx.notify();
    }

    /// The toolbar's terminal button: from the Git view it switches to Code and
    /// shows the terminal; otherwise it toggles it like Ctrl+`.
    pub(super) fn terminal_button(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Active::Project(i) = self.saved.active
            && let Some(p) = self.saved.projects.get_mut(i)
            && p.view != View::Code
        {
            p.view = View::Code;
            self.saved.terminal_open = false;
        }
        self.toggle_terminal(&ToggleTerminal, window, cx);
    }

    /// Runs a command from a tool call in the project's terminal, opening it if needed.
    pub(super) fn run_in_terminal(
        &mut self,
        kind: ShellKind,
        command: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(cwd) = self.terminal_cwd() else { return };
        self.saved.terminal_open = true;
        self.save();
        let panel = self.terminal_panel(&cwd, cx);
        panel.update(cx, |p, cx| p.run(kind, &command, window, cx));
        cx.notify();
    }

    /// The panel and its resize handle, when it's open in a project.
    pub(super) fn render_terminal_dock(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        if !self.saved.terminal_open {
            return None;
        }
        let cwd = self.terminal_cwd()?;
        let panel = self.terminal_panel(&cwd, cx);
        // Restored as open from last time: start a shell on first show.
        if panel.read(cx).is_empty() {
            panel.update(cx, |p, cx| p.ensure_open(window, cx));
        }
        let t = theme(cx).clone();
        let max = (f32::from(window.viewport_size().height) - MIN_CONVERSATION).max(MIN_HEIGHT);
        let height = self.saved.terminal_height.clamp(MIN_HEIGHT, max);
        Some(
            div().relative().flex_none().h(px(height)).border_t_1().border_color(t.border).child(panel).child(
                // Drag the top edge to resize.
                div()
                    .id("terminal-resize")
                    .absolute()
                    .top(px(-3.))
                    .left_0()
                    .right_0()
                    .h(px(6.))
                    .cursor_row_resize()
                    .hover(|s| s.bg(t.border_focused.opacity(0.6)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.resizing_terminal = true;
                            cx.stop_propagation();
                        }),
                    ),
            ),
        )
    }

    /// Follows the mouse while the top edge is dragged.
    pub(super) fn drag_terminal_edge(&mut self, mouse_y: f32, window: &Window) {
        let viewport = f32::from(window.viewport_size().height);
        let max = (viewport - MIN_CONVERSATION).max(MIN_HEIGHT);
        self.saved.terminal_height = (viewport - mouse_y).clamp(MIN_HEIGHT, max);
    }
}
