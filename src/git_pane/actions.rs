//! What the Git pane does to its files: a row's box stages it (a folder, all below it) or
//! unstages it, Stage all / Unstage all, and a row's right-click menu with Discard, which
//! asks once more on the row as the Git view does. Everything runs through the worktree's
//! [`GitView`], so one operation runs at a time and its errors show the same way.

use gpui::{AnyElement, Context, Entity, Pixels, Point, SharedString, anchored, deferred, div, prelude::*};
use kynex_core::git::{Staged, Target, TreeStatus, files_of, stage_paths};

use super::{GitPane, GitPaneEvent};
use crate::git::GitView;
use crate::theme::theme;
use crate::ui::{Tooltip, ghost_button, menu_item, popover, px};

/// A row, by path: the rows may be rebuilt under a menu or a question.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Owned {
    File(String),
    Folder(String),
}

/// A row's right-click menu, where it was opened.
pub(super) struct RowMenu {
    what: Owned,
    at: Point<Pixels>,
}

/// What a menu item does.
type MenuAction = Box<dyn Fn(&mut GitPane, &mut Context<GitPane>)>;

/// The row waiting for "Discard?" to be clicked.
pub(super) type Confirm = Owned;

impl GitPane {
    /// The files `what` covers, as indices into `files`.
    fn covered(&self, what: &Owned) -> Vec<usize> {
        match what {
            Owned::File(p) => self.files.iter().position(|f| f.path == *p).into_iter().collect(),
            Owned::Folder(p) => files_of(&self.files, Target::Folder(p)),
        }
    }

    /// How much of `what` is staged.
    pub(super) fn staged_of(&self, what: &Owned) -> Staged {
        let states: Vec<Staged> = self.covered(what).into_iter().map(|i| self.files[i].staged()).collect();
        if states.iter().all(|s| *s == Staged::All) && !states.is_empty() {
            Staged::All
        } else if states.iter().all(|s| *s == Staged::No) {
            Staged::No
        } else {
            Staged::Partly
        }
    }

    /// A row's box: stages what isn't staged yet, or unstages it all if it all is.
    pub(super) fn click_box(&mut self, what: &Owned, cx: &mut Context<Self>) {
        let unstage = self.staged_of(what).unstages();
        self.stage(what, unstage, cx);
    }

    fn stage(&mut self, what: &Owned, unstage: bool, cx: &mut Context<Self>) {
        let target = match what {
            Owned::File(p) => match self.files.iter().position(|f| f.path == *p) {
                Some(i) => Target::File(i),
                None => return,
            },
            Owned::Folder(p) => Target::Folder(p),
        };
        let paths = stage_paths(&self.files, target, unstage);
        self.confirm = None;
        let view = self.view(cx);
        view.update(cx, |v, cx| v.stage_paths(paths, unstage, cx));
    }

    /// Throws away the unstaged changes of `what` (untracked files are deleted).
    pub(super) fn discard(&mut self, what: &Owned, cx: &mut Context<Self>) {
        let files = self
            .covered(what)
            .into_iter()
            .filter(|&i| self.files[i].discardable())
            .map(|i| self.files[i].file_status());
        let files: Vec<_> = files.collect();
        self.confirm = None;
        let view = self.view(cx);
        view.update(cx, |v, cx| v.discard_files(files, cx));
        cx.notify();
    }

    /// What Discard says for `what`: deleting when all it would throw away is new files.
    pub(super) fn discard_words(&self, what: &Owned) -> Option<(&'static str, &'static str)> {
        let files: Vec<usize> = self.covered(what).into_iter().filter(|&i| self.files[i].discardable()).collect();
        if files.is_empty() {
            return None;
        }
        let all_new = files.iter().all(|&i| self.files[i].status == TreeStatus::Untracked);
        Some(match (what, all_new) {
            (Owned::File(_), true) => ("Delete?", "Delete this new file"),
            (Owned::File(_), false) => ("Discard?", "Discard these changes"),
            (Owned::Folder(_), true) => ("Delete?", "Delete the new files in this folder"),
            (Owned::Folder(_), false) => ("Discard?", "Discard the changes in this folder"),
        })
    }

    pub(super) fn open_menu(&mut self, what: Owned, at: Point<Pixels>, cx: &mut Context<Self>) {
        self.confirm = None;
        self.menu = Some(RowMenu { what, at });
        cx.notify();
    }

    /// The open row menu: stage, unstage, the diff, discard.
    pub(super) fn render_menu(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.menu.as_ref()?;
        let (what, at) = (menu.what.clone(), menu.at);
        let staged = self.staged_of(&what);
        let mut items: Vec<AnyElement> = Vec::new();
        let mut item = |id: &str, label: String, f: MenuAction, cx: &mut Context<Self>| {
            items.push(
                menu_item(SharedString::from(format!("git-pane-menu-{id}")), label, false, cx)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.menu = None;
                        f(this, cx);
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        };
        if staged != Staged::All {
            let w = what.clone();
            let label = if staged == Staged::Partly { "Stage the rest" } else { "Stage" };
            item("stage", label.into(), Box::new(move |this, cx| this.stage(&w, false, cx)), cx);
        }
        if staged != Staged::No {
            let w = what.clone();
            item("unstage", "Unstage".into(), Box::new(move |this, cx| this.stage(&w, true, cx)), cx);
        }
        if let Owned::File(path) = &what
            && let Some(f) = self.files.iter().find(|f| f.path == *path)
        {
            let (root, path, untracked, staged) =
                (self.selected.clone(), path.clone(), f.status == TreeStatus::Untracked, f.staged());
            item(
                "diff",
                "Open diff".into(),
                Box::new(move |_, cx| {
                    cx.emit(GitPaneEvent::OpenFile { root: root.clone(), path: path.clone(), untracked, staged })
                }),
                cx,
            );
        }
        if let Some((_, tip)) = self.discard_words(&what) {
            let w = what.clone();
            item("discard", format!("{tip}…"), Box::new(move |this, _| this.confirm = Some(w.clone())), cx);
        }
        Some(
            deferred(
                anchored().position(at).snap_to_window_with_margin(gpui::px(8.)).child(
                    popover(cx)
                        .id("git-pane-menu")
                        .w(px(220.))
                        .occlude()
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                            this.menu = None;
                            cx.notify();
                        }))
                        .children(items),
                ),
            )
            .into_any_element(),
        )
    }

    /// Above the tree: how many files changed, Stage all and Unstage all.
    pub(super) fn render_tree_bar(&mut self, view: &Entity<GitView>, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.loaded || self.files.is_empty() {
            return None;
        }
        let t = theme(cx).clone();
        let busy = view.read(cx).busy().is_some();
        let any_staged = self.files.iter().any(|f| f.staged() != Staged::No);
        let any_unstaged = self.files.iter().any(|f| f.staged() != Staged::All);
        let n = self.files.len();
        let button = |id: &'static str,
                      label: &'static str,
                      tip: &'static str,
                      unstage: bool,
                      on: bool,
                      cx: &mut Context<Self>| {
            let b = ghost_button(id, cx).text_xs().child(label);
            if !on || busy {
                return b.opacity(0.5).cursor_default();
            }
            let view = view.clone();
            b.tooltip(Tooltip::text(tip))
                .on_click(cx.listener(move |_, _, _, cx| view.update(cx, |v, cx| v.stage_everything(unstage, cx))))
        };
        Some(
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap_1()
                .h(px(30.))
                .pl_3()
                .pr_1()
                .text_xs()
                .text_color(t.text_muted)
                .child(div().flex_1().min_w_0().truncate().child(if n == 1 {
                    "1 changed file".to_string()
                } else {
                    format!("{n} changed files")
                }))
                .child(button("git-pane-stage-all", "Stage all", "Stage every change", false, any_unstaged, cx))
                .child(button(
                    "git-pane-unstage-all",
                    "Unstage all",
                    "Take everything out of the commit",
                    true,
                    any_staged,
                    cx,
                ))
                .into_any_element(),
        )
    }
}
