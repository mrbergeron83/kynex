//! A worktree's Git view in a modal over the conversation, opened from the
//! branch chips in the session header: its changes and diffs, history and
//! commit box, without leaving the session. Esc or a click outside closes it.

use std::path::{Path, PathBuf};

use gpui::{AnyElement, Context, FontWeight, MouseButton, Window, div, prelude::*, px};
use kynex_core::git::Worktree;

use super::KynexApp;
use crate::git::GitView;
use crate::theme::{mono, theme};
use crate::ui::motion::{self, fade_in, pop_in};
use crate::ui::{Icon, Tooltip, icon, icon_button, outline_button};

impl KynexApp {
    pub(super) fn open_git_modal(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        // Opened again: read it fresh rather than show what it was last time.
        if let Some(git) = self.git.get(&path) {
            git.update(cx, |g, cx| g.refresh(cx));
        }
        self.git_modal = Some(path);
        cx.notify();
    }

    pub(super) fn close_git_modal(&mut self, cx: &mut Context<Self>) {
        // A Git view only the modal was showing stops polling with it.
        if let Some(path) = self.git_modal.take()
            && !self.saved.projects.iter().any(|p| p.path == path)
        {
            self.git.remove(&path);
        }
        cx.notify();
    }

    /// The worktree at `path`, from whichever project is watching it.
    fn known_worktree(&self, path: &Path, cx: &Context<Self>) -> Option<Worktree> {
        self.worktrees.values().find_map(|w| w.read(cx).at(path).cloned())
    }

    pub(super) fn render_git_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let path = self.git_modal.clone()?;
        let t = theme(cx).clone();
        let vp = window.viewport_size();
        let git = self.git.entry(path.clone()).or_insert_with(|| cx.new(|cx| GitView::new(path.clone(), cx))).clone();
        let worktree = self.known_worktree(&path, cx);
        let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
        let branch = worktree.as_ref().map(|w| w.label().to_string());
        let linked = worktree.as_ref().is_some_and(|w| !w.main);

        let open_path = path.clone();
        let header = div()
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .h(px(40.))
            .px_3()
            .border_b_1()
            .border_color(t.border_variant)
            .child(icon(Icon::GitBranch, t.text_muted))
            .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(name))
            .when_some(branch, |d, b| d.child(div().text_sm().text_color(t.text_muted).child(b)))
            .when(linked, |d| d.child(div().text_xs().text_color(t.info).child("worktree")))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_xs()
                    .font_family(mono())
                    .text_color(t.text_placeholder)
                    .child(path.display().to_string()),
            )
            .child(
                outline_button("git-modal-tab", "Open as tab", cx)
                    .tooltip(Tooltip::text("Open this folder as a project tab, on its Git view"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.git_modal = None;
                        this.open_worktree_changes(open_path.clone(), cx);
                    })),
            )
            .child(
                icon_button("git-modal-close", Icon::Close, cx)
                    .tooltip(Tooltip::with_meta("Close", "Esc"))
                    .on_click(cx.listener(|this, _, _, cx| this.close_git_modal(cx))),
            );

        let card = div()
            .id("git-modal")
            .occlude()
            .flex()
            .flex_col()
            .w(px(1280.).min(vp.width - px(80.)))
            .h(vp.height - px(100.))
            .rounded_lg()
            .overflow_hidden()
            .border_1()
            .border_color(t.border)
            .bg(t.editor)
            .shadow_lg()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(header)
            .child(div().flex_1().min_h_0().child(git));

        Some(
            fade_in(
                "git-modal-fade",
                div()
                    .id("git-modal-backdrop")
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(gpui::black().opacity(0.45))
                    .occlude()
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| this.close_git_modal(cx)))
                    .child(pop_in("git-modal-in", card, px(8.))),
                motion::QUICK,
            )
            .into_any_element(),
        )
    }
}
