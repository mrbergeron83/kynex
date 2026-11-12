//! The parts of the Git view the Git pane hosts: its state read from outside, staging and
//! discarding by path, the commit box and the notices (an error, conflicts). The pane keeps
//! one `GitView` per worktree and draws these pieces of it, so both act the same way.

use std::time::Instant;

use gpui::{AnyElement, Context, SharedString, Window, div, prelude::*};
use kynex_core::git::{self, FileStatus, Status};

use super::{GitEvent, GitView};
use crate::theme::theme;
use crate::ui::{Icon, icon, icon_button};

impl GitView {
    /// The operation in flight ("Committing", "Pushing"…), if any.
    pub fn busy(&self) -> Option<SharedString> {
        self.busy.clone()
    }

    /// Drawn somewhere: keeps the periodic re-read going.
    pub(crate) fn touch(&mut self) {
        self.drawn = Some(Instant::now());
    }

    /// Stages `paths` (pathspecs: files or folders), or unstages them.
    pub fn stage_paths(&mut self, paths: Vec<String>, unstage: bool, cx: &mut Context<Self>) {
        if paths.is_empty() {
            return;
        }
        let label = if unstage { "Unstaging" } else { "Staging" };
        self.run(
            label,
            cx,
            move |root| if unstage { git::unstage(root, &paths) } else { git::stage(root, &paths) },
            |_, _| {},
        );
    }

    /// Stages every change, or unstages everything.
    pub fn stage_everything(&mut self, unstage: bool, cx: &mut Context<Self>) {
        let label = if unstage { "Unstaging" } else { "Staging" };
        self.run(label, cx, move |root| if unstage { git::unstage_all(root) } else { git::stage_all(root) }, |_, _| {});
    }

    /// Throws away the unstaged changes of `files`; untracked ones are deleted.
    pub fn discard_files(&mut self, files: Vec<FileStatus>, cx: &mut Context<Self>) {
        if files.is_empty() {
            return;
        }
        self.run("Discarding", cx, move |root| files.iter().try_for_each(|f| git::discard(root, f)), |_, _| {});
    }

    /// The commit box: message, Amend, and Commit (or Commit all with nothing staged).
    pub fn render_commit_area(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        self.touch();
        let (staged, unstaged) = self.status.as_ref().map_or((0, 0), |s| (s.staged().count(), s.unstaged().count()));
        self.commit_box(staged, unstaged, window, cx).into_any_element()
    }

    /// What went wrong last (until dismissed), and a note while there are conflicts.
    pub fn render_notices(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let t = theme(cx).clone();
        let mut out = Vec::new();
        if let Some(e) = self.error.clone() {
            out.push(
                div()
                    .flex()
                    .flex_none()
                    .items_start()
                    .gap_2()
                    .px_3()
                    .py_1p5()
                    .border_b_1()
                    .border_color(t.border_variant)
                    .bg(t.error.opacity(0.08))
                    .child(icon(Icon::Dot, t.error).size_3().mt_1())
                    .child(div().flex_1().min_w_0().text_sm().text_color(t.text).child(e))
                    .child(icon_button("git-dismiss-error", Icon::Close, cx).on_click(cx.listener(|this, _, _, cx| {
                        this.error = None;
                        cx.notify();
                    })))
                    .into_any_element(),
            );
        }
        if self.status.as_ref().is_some_and(Status::has_conflicts) {
            out.push(
                div()
                    .flex_none()
                    .px_3()
                    .py_1p5()
                    .border_b_1()
                    .border_color(t.border_variant)
                    .bg(t.warning.opacity(0.08))
                    .text_sm()
                    .child("Conflicts: resolve the files marked ! in your editor, then stage them to continue.")
                    .into_any_element(),
            );
        }
        out
    }

    /// Tells whoever hosts the view that the repository changed under it.
    pub(super) fn changed(&mut self, cx: &mut Context<Self>) {
        cx.emit(GitEvent::Changed);
    }
}
