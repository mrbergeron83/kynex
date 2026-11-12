//! A changed file's whole diff in a modal over the window, opened from the Git pane's
//! file tree. Esc, the close button or a click outside closes it; the pane stays as it was.

use std::path::PathBuf;

use gpui::{AnyElement, Context, MouseButton, Window, div, prelude::*};

use super::KynexApp;
use crate::git_pane::file_modal::{FileDiffEvent, FileDiffView};
use crate::theme::theme;
use crate::ui::motion::{self, fade, pop};

impl KynexApp {
    pub(super) fn open_git_file_diff(&mut self, root: PathBuf, path: String, untracked: bool, cx: &mut Context<Self>) {
        let view = cx.new(|cx| FileDiffView::new(root, path, untracked, cx));
        cx.subscribe(&view, |this, _, ev: &FileDiffEvent, cx| match ev {
            FileDiffEvent::Close => this.close_git_file_modal(cx),
        })
        .detach();
        self.git_file = Some(view);
        cx.notify();
    }

    pub(super) fn close_git_file_modal(&mut self, cx: &mut Context<Self>) {
        self.git_file = None;
        cx.notify();
    }

    pub(super) fn render_git_file_modal(
        &mut self,
        leaving: Option<f32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let view = self.git_file.clone()?;
        let t = theme(cx).clone();
        let vp = window.viewport_size();
        let card = div()
            .id("git-file-modal")
            .occlude()
            .w(gpui::px(1200.).min(vp.width - gpui::px(80.)))
            .h(vp.height - gpui::px(100.))
            .rounded_lg()
            .overflow_hidden()
            .border_1()
            .border_color(t.border)
            .bg(t.editor)
            .shadow_lg()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(view);
        Some(fade(
            "git-file-modal-fade",
            div()
                .id("git-file-modal-backdrop")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(t.scrim())
                .occlude()
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| this.close_git_file_modal(cx)))
                .child(pop("git-file-modal-in", card, gpui::px(8.), leaving)),
            motion::QUICK,
            leaving,
        ))
    }
}
