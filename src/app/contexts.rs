//! The contexts section at the top of the sessions column: one row per checkout of
//! the project (its main tree and linked git worktrees) with a state lamp rolled up
//! from the sessions running there, its branch and its changes. Picking one shows
//! its session, which changes the sessions list below to that checkout's.

use std::path::{Path, PathBuf};

use gpui::{Anchor, AnyElement, Context, MouseButton, MouseDownEvent, div, prelude::*};
use kynex_core::context::{self, Contexts};
use kynex_core::git::Worktree;
use kynex_core::session::{Kind, Session};
use kynex_core::toasts::Toast;

use super::KynexApp;
use super::menus::Menu;
use super::session_header::git_stats;
use super::state::{Active, View};
use crate::theme::{mono, theme};
use crate::ui::wash::row_wash;
use crate::ui::{Icon, Lamp, Tooltip, icon_button, lamp, px, section_label};

/// A checkout with the worktree it came from (for its changes).
type Entry = (context::Context, Option<Worktree>);

impl KynexApp {
    /// The project's saved contexts, read on first use.
    pub(super) fn saved_contexts(&mut self, project: &Path) -> &mut Contexts {
        self.contexts.entry(project.to_path_buf()).or_insert_with(|| Contexts::load(project))
    }

    /// The project's contexts in their saved order, from the worktree list kept
    /// up to date in the background.
    pub(super) fn contexts_of(&mut self, project: &Path, cx: &mut Context<Self>) -> Vec<Entry> {
        let list = self.worktrees(project, cx).read(cx).list.clone();
        let saved = self.saved_contexts(project);
        let ordered = saved.ordered(saved.with_folders(context::from_worktrees(project, &list)));
        ordered
            .into_iter()
            // A worktree whose folder is gone is no context to pick.
            // (A worktree that's gone is `prunable` and already dropped; an added folder
            // that's gone stays, marked, so it can be removed.)
            .map(|c| {
                let worktree = list.iter().find(|w| c.at(&w.path)).cloned();
                (c, worktree)
            })
            .collect()
    }

    /// The code project in the active tab, if it is one.
    pub(super) fn context_project(&self) -> Option<PathBuf> {
        self.active_project().map(|p| p.path.clone())
    }

    /// Writes the project's contexts file off the UI thread; a failure shows as a notice.
    pub(super) fn save_contexts(&mut self, project: &Path, cx: &mut Context<Self>) {
        let Some(contexts) = self.contexts.get(project).cloned() else { return };
        let project = project.to_path_buf();
        cx.spawn(async move |this, cx| {
            if let Err(e) = cx.background_spawn(async move { contexts.save(&project) }).await {
                this.update(cx, |this, cx| {
                    this.toast(Toast::error(e));
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    /// The folder you're in: the shown session's, which is the selected context's.
    /// The terminal, the Git view and the toolbar's path follow it.
    pub(super) fn context_cwd(&self) -> Option<PathBuf> {
        let p = self.active_project()?;
        Some(self.code.get(&p.path).and_then(|l| l.current()).map_or_else(|| p.path.clone(), |s| s.cwd.clone()))
    }

    /// A closed project's terminals and Git views, in each of its contexts' folders.
    /// `cwds` are the folders its sessions ran in.
    pub(super) fn drop_context_views(&mut self, project: &Path, cwds: &[PathBuf]) {
        let made_here = context::worktrees_dir(project);
        let ours = |f: &Path| {
            context::same_folder(f, project)
                || f.starts_with(&made_here)
                || cwds.iter().any(|c| context::same_folder(c, f))
        };
        self.git.retain(|f, _| !ours(f));
        // Dropping a panel ends its shells.
        self.terminals.retain(|f, _| !ours(f));
    }

    /// Shows a context: its open session, else the one it last had, else a new one.
    pub(super) fn select_context(&mut self, target: &Path, cx: &mut Context<Self>) {
        let Some(project) = self.context_project() else { return };
        if !target.is_dir() {
            self.toast(Toast::error(format!("{} isn't there any more", target.display())));
            return;
        }
        let leaving = self
            .code
            .get(&project)
            .and_then(|l| l.current())
            .map(|s| (s.cwd.clone(), s.session_id.clone().filter(|_| !s.is_new())));
        let contexts = self.saved_contexts(&project);
        if let Some((cwd, Some(id))) = &leaving {
            contexts.remember(cwd, Some(id));
        }
        contexts.select(target);
        let remembered = contexts.session(target).map(str::to_string);
        self.save_contexts(&project, cx);
        // Sessions are reached through the Code view; the view you were in comes back after.
        let was_git = self.active_project().is_some_and(|p| p.view == View::Git);
        self.show_code_view();
        let there = |s: &Session| s.kind == Kind::Code && context::same_folder(&s.cwd, target);
        // The context's remembered session if it's open, else any session there.
        let remembered_open = |s: &Session| there(s) && remembered.is_some() && s.session_id == remembered;
        if !self.show_open(remembered_open) && !self.show_open(there) {
            let cwd = target.to_path_buf();
            self.open_session(match remembered {
                Some(id) => Session::resume(Kind::Code, cwd, id, self.wake.clone()),
                None => Session::new(Kind::Code, cwd),
            });
        }
        if was_git
            && let Active::Project(i) = self.saved.active
            && let Some(p) = self.saved.projects.get_mut(i)
        {
            p.view = View::Git;
            self.save();
        }
        cx.notify();
    }

    /// The next (`1`) or previous (`-1`) context, wrapping round.
    pub(super) fn step_context(&mut self, by: isize, cx: &mut Context<Self>) {
        let Some(project) = self.context_project() else { return };
        let Some(cwd) = self.code.get(&project).and_then(|l| l.current()).map(|s| s.cwd.clone()) else { return };
        let list = self.contexts_of(&project, cx);
        if list.len() < 2 {
            return;
        }
        let at = list.iter().position(|(c, _)| c.at(&cwd)).unwrap_or(0);
        let next = (at as isize + by).rem_euclid(list.len() as isize) as usize;
        self.select_context(&list[next].0.path, cx);
    }

    /// The section: a header with "New context" (git repositories) and "Add folder",
    /// and a row per context.
    pub(super) fn render_contexts(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let project = self.context_project()?;
        // A git repository, where a worktree can be made: git read it once the worktree
        // list isn't empty. Any project can add plain folders.
        let repo = !self.worktrees(&project, cx).read(cx).list.is_empty();
        let list = self.contexts_of(&project, cx);
        let t = theme(cx).clone();
        let cwd = self.code.get(&project)?.current()?.cwd.clone();
        let colour = self.project_colour(&project, &t).unwrap_or(t.accent);
        // The shown session's context, else the saved selection, else the main tree.
        let saved = self.saved_contexts(&project).selected_in(&plain(&list)).path.clone();
        let selected =
            list.iter().position(|(c, _)| c.at(&cwd)).or_else(|| list.iter().position(|(c, _)| c.at(&saved)));
        let selected = selected.unwrap_or(0);
        let live = self.code.get(&project)?;
        let rows: Vec<AnyElement> = list
            .iter()
            .enumerate()
            .map(|(n, (c, worktree))| {
                let lamp_state = Lamp::most_urgent(
                    live.sessions
                        .iter()
                        .filter(|s| context::same_folder(&s.cwd, &c.path))
                        .map(|s| Lamp::of(s.busy, s.waiting(), s.finished)),
                );
                let is_selected = n == selected;
                let path = c.path.clone();
                let menu_path = c.path.clone();
                let stats = worktree.as_ref().map(|w| git_stats(w, &t)).unwrap_or_default();
                // The branch, when the folder's name doesn't already say it; a KNX Ops worker's says so.
                let branch = if c.worker() {
                    Some("worker".to_string())
                } else if c.folder {
                    Some(if c.path.is_dir() { "folder" } else { "missing" }.to_string())
                } else {
                    c.branch.clone().filter(|b| *b != c.label)
                };
                div()
                    .id(("context", n))
                    .flex()
                    .flex_col()
                    .justify_center()
                    .min_h(px(28.))
                    .pl_2()
                    .pr_1()
                    .py_0p5()
                    .rounded_md()
                    .cursor_pointer()
                    .text_sm()
                    .relative()
                    .child(row_wash(is_selected, t.hover, t.selection))
                    .when(is_selected, |d| {
                        d.child(div().absolute().left_0().top_1().bottom_1().w(px(2.)).rounded_full().bg(colour))
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .child(lamp(("context-lamp", n), lamp_state, cx))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(if is_selected { t.text } else { t.text_muted })
                                    .child(c.label.clone()),
                            )
                            .children(branch.map(|b| {
                                div()
                                    .flex_none()
                                    .max_w(px(100.))
                                    .truncate()
                                    .font_family(mono())
                                    .text_xs()
                                    .text_color(t.text_placeholder)
                                    .child(b)
                            })),
                    )
                    .when(!stats.is_empty(), |d| d.child(div().flex().gap_1p5().pl_4().text_xs().children(stats)))
                    .tooltip(Tooltip::text(path.display().to_string()))
                    .on_click(cx.listener(move |this, _, _, cx| this.select_context(&path, cx)))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                            this.open_menu(Menu::Context(menu_path.clone()), ev.position, Anchor::TopLeft, cx)
                        }),
                    )
                    .into_any_element()
            })
            .collect();
        let forms = self.render_context_forms(cx);
        Some(
            div()
                .flex()
                .flex_col()
                .flex_none()
                .px_1p5()
                .pb_1p5()
                .border_b_1()
                .border_color(t.border_variant)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .child(section_label("Contexts", cx).flex_1().px_2().pt_1p5().pb_1())
                        .child(
                            icon_button("add-context-folder", Icon::FolderPlus, cx)
                                .tooltip(Tooltip::text("Add a folder as a context: scripts, notes"))
                                .on_click(cx.listener(|this, _, _, cx| this.add_folder_context(cx))),
                        )
                        .when(repo, |d| {
                            d.child(
                                icon_button("new-context", Icon::Plus, cx)
                                    .tooltip(Tooltip::text("New context: a worktree on a branch"))
                                    .on_click(cx.listener(|this, _, window, cx| this.open_new_context(window, cx))),
                            )
                        }),
                )
                .children(rows)
                .children(forms)
                .into_any_element(),
        )
    }
}

/// The contexts of `list`, without their worktrees.
fn plain(list: &[Entry]) -> Vec<context::Context> {
    list.iter().map(|(c, _)| c.clone()).collect()
}
