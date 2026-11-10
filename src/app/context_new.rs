//! Making and removing contexts from the sessions column: the "New context" form (a
//! branch name, new or existing, off the default branch), the confirmation before a
//! worktree with changes or a running session goes, and folding worktrees that were
//! open as project tabs into their project.

use std::path::{Path, PathBuf};

use gpui::{AnyElement, Context, Entity, Focusable, Window, div, prelude::*};
use kynex_core::context;
use kynex_core::git;
use kynex_core::toasts::Toast;

use super::KynexApp;
use super::state::Active;
use crate::theme::theme;
use crate::ui::input::{InputEvent, TextInput};
use crate::ui::{Icon, Tooltip, icon_button, name_field, outline_button, primary_button, px};

/// What's open in the contexts section besides its rows.
#[derive(Default)]
pub(super) struct ContextUi {
    /// The "New context" branch field, while it's open.
    form: Option<Entity<TextInput>>,
    /// Local branches without a checkout, to pick from while naming one.
    branches: Vec<String>,
    /// A context is being made.
    busy: bool,
    /// A context waiting on a yes before it goes.
    removing: Option<Removal>,
}

/// A removal that waits for confirmation.
struct Removal {
    path: PathBuf,
    label: String,
    /// Why it asks: what would be lost.
    why: Vec<String>,
}

impl KynexApp {
    /// Opens the "New context" field, with the branches that could be checked out.
    pub(super) fn open_new_context(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(project) = self.context_project() else { return };
        let input = cx.new(|cx| TextInput::new(cx).placeholder("Branch, new or existing"));
        cx.subscribe_in(&input, window, |this, _, ev: &InputEvent, window, cx| match ev {
            InputEvent::Submit => this.submit_new_context(window, cx),
            InputEvent::Changed => cx.notify(),
            InputEvent::PasteImages(_) => {}
        })
        .detach();
        input.read(cx).focus_handle(cx).focus(window, cx);
        self.context_ui = ContextUi { form: Some(input), ..ContextUi::default() };
        cx.spawn(async move |this, cx| {
            let names = cx
                .background_spawn(async move {
                    git::branches(&project)
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|b| !b.remote && !b.current && b.worktree.is_none())
                        .map(|b| b.name)
                        .collect::<Vec<_>>()
                })
                .await;
            this.update(cx, |this, cx| {
                this.context_ui.branches = names;
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Esc: closes the new-context field or the removal question. Returns whether one was open.
    pub(super) fn cancel_new_context(&mut self) -> bool {
        let open = self.context_ui.form.is_some() || self.context_ui.removing.is_some();
        self.context_ui = ContextUi::default();
        open
    }

    /// Enter in the field: makes the context and shows it, with a new session focused.
    fn submit_new_context(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = &self.context_ui.form else { return };
        let name = input.read(cx).text().trim().to_string();
        let Some(project) = self.context_project() else { return };
        if name.is_empty() || self.context_ui.busy {
            return;
        }
        self.context_ui.busy = true;
        // Whichever session the new context opens with, the composer takes the keys.
        self.focus_input(window, cx);
        cx.spawn(async move |this, cx| {
            let root = project.clone();
            let made = cx
                .background_spawn(async move {
                    let base = git::default_branch(&root);
                    context::create(&root, &name, base.as_deref())
                })
                .await;
            this.update(cx, |this, cx| {
                this.context_ui.busy = false;
                match made {
                    Ok(path) => {
                        this.context_ui.form = None;
                        this.refresh_worktrees([&project], cx);
                        this.select_context(&path, cx);
                    }
                    Err(e) => {
                        this.toast(Toast::error(e));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Remove on a context's menu: asks first when it has changes or a running
    /// session, otherwise goes at once.
    pub(super) fn remove_context(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let Some(project) = self.context_project() else { return };
        let worktree = self.worktrees.get(&project).and_then(|w| w.read(cx).at(&path).cloned());
        let Some(worktree) = worktree.filter(|w| !w.main) else { return };
        let mut why = Vec::new();
        if let Some(n) = worktree.changes.filter(|n| *n > 0) {
            why.push(format!("{n} changed file{}", if n == 1 { "" } else { "s" }));
        }
        let running = self
            .code
            .get(&project)
            .is_some_and(|l| l.sessions.iter().any(|s| context::same_folder(&s.cwd, &path) && (s.busy || s.waiting())));
        if running {
            why.push("a session still working".to_string());
        }
        if why.is_empty() {
            return self.confirm_remove_context(path, false, cx);
        }
        let label = worktree.label().to_string();
        self.context_ui = ContextUi { removing: Some(Removal { path, label, why }), ..ContextUi::default() };
        cx.notify();
    }

    /// Closes the context's sessions, then `git worktree remove`; the branch stays.
    fn confirm_remove_context(&mut self, path: PathBuf, force: bool, cx: &mut Context<Self>) {
        self.context_ui = ContextUi::default();
        let Some(project) = self.context_project() else { return };
        // Sessions there go first: a process in the folder keeps Windows from removing it.
        let shown_there =
            self.code.get(&project).and_then(|l| l.current()).is_some_and(|s| context::same_folder(&s.cwd, &path));
        // If the removal fails, picking the context again resumes the session it had.
        let kept = self.code.get(&project).and_then(|l| {
            l.sessions
                .iter()
                .filter(|s| context::same_folder(&s.cwd, &path) && !s.is_new())
                .rev()
                .find_map(|s| s.session_id.clone())
        });
        let contexts = self.contexts.entry(project.clone()).or_insert_with(|| context::Contexts::load(&project));
        contexts.remember(&path, kept.as_deref());
        if contexts.selected.as_deref().is_some_and(|s| context::same_folder(s, &path)) {
            contexts.selected = None;
        }
        if let Some(live) = self.code.get_mut(&project) {
            let closed = live.close_where(|s| context::same_folder(&s.cwd, &path));
            self.forget(&closed);
        }
        if shown_there {
            self.select_context(&project, cx);
        }
        let root = project.clone();
        let gone = path.clone();
        cx.spawn(async move |this, cx| {
            let removed = cx.background_spawn(async move { context::remove(&root, &gone, force) }).await;
            this.update(cx, |this, cx| {
                match removed {
                    Ok(()) => {
                        let contexts = this.contexts.entry(project.clone()).or_default();
                        contexts.forget(&path);
                        if let Err(e) = contexts.save(&project) {
                            this.toast(Toast::error(e));
                        }
                    }
                    Err(e) => {
                        this.toast(Toast::error(e));
                    }
                }
                this.refresh_worktrees([&project], cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// The form and the question, under the context rows.
    pub(super) fn render_context_forms(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let t = theme(cx).clone();
        let mut out = Vec::new();
        if let Some(input) = self.context_ui.form.clone() {
            let typed = input.read(cx).text().trim().to_lowercase();
            let picks: Vec<String> = self
                .context_ui
                .branches
                .iter()
                .filter(|b| typed.is_empty() || b.to_lowercase().contains(&typed))
                .take(5)
                .cloned()
                .collect();
            let busy = self.context_ui.busy;
            out.push(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .px_1()
                    .pt_1()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(name_field(input.clone(), &t).flex_1())
                            .child(
                                primary_button("new-context-ok", "Create", cx)
                                    .when(busy, |b| b.opacity(0.5))
                                    .on_click(cx.listener(|this, _, window, cx| this.submit_new_context(window, cx))),
                            )
                            .child(
                                icon_button("new-context-close", Icon::Close, cx)
                                    .tooltip(Tooltip::with_meta("Cancel", "Esc"))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.cancel_new_context();
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(t.text_placeholder)
                            .child("A new branch starts from the default branch; an existing one is checked out."),
                    )
                    .children(picks.into_iter().enumerate().map(|(n, b)| {
                        let name = b.clone();
                        div()
                            .id(("context-pick", n))
                            .px_2()
                            .h(px(24.))
                            .flex()
                            .items_center()
                            .rounded_md()
                            .cursor_pointer()
                            .text_xs()
                            .text_color(t.text_muted)
                            .hover(|s| s.bg(t.hover))
                            .child(b)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if let Some(input) = &this.context_ui.form {
                                    input.update(cx, |i, cx| i.set_text(&name, cx));
                                }
                                this.submit_new_context(window, cx);
                            }))
                    }))
                    .into_any_element(),
            );
        }
        if let Some(r) = &self.context_ui.removing {
            let path = r.path.clone();
            out.push(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .px_2()
                    .pt_1()
                    .child(div().text_xs().text_color(t.warning).child(format!(
                        "Remove {}? It has {}.",
                        r.label,
                        r.why.join(" and ")
                    )))
                    .child(
                        div()
                            .flex()
                            .gap_1()
                            .child(primary_button("remove-context-yes", "Remove anyway", cx).on_click(
                                cx.listener(move |this, _, _, cx| this.confirm_remove_context(path.clone(), true, cx)),
                            ))
                            .child(outline_button("remove-context-no", "Keep", cx).on_click(cx.listener(
                                |this, _, _, cx| {
                                    this.cancel_new_context();
                                    cx.notify();
                                },
                            ))),
                    )
                    .into_any_element(),
            );
        }
        out
    }

    /// Shows `path` as a context of the open project it's a worktree of, if there
    /// is one. Returns whether it did.
    pub(super) fn open_as_context(&mut self, path: &Path, cx: &mut Context<Self>) -> bool {
        let owner = self.saved.projects.iter().position(|p| {
            !context::same_folder(&p.path, path)
                && self
                    .worktrees
                    .get(&p.path)
                    .is_some_and(|w| w.read(cx).list.iter().any(|w| !w.main && context::same_folder(&w.path, path)))
        });
        let Some(i) = owner else { return false };
        if self.saved.active != Active::Project(i) {
            self.saved.active = Active::Project(i);
            self.menu = None;
            self.save();
        }
        self.select_context(path, cx);
        true
    }

    /// Worktrees opened as project tabs by earlier versions fold into the project
    /// they belong to, keeping the session each had as that context's. Reads
    /// every project's worktrees off the UI thread.
    pub(super) fn fold_worktree_tabs(&mut self, cx: &mut Context<Self>) {
        let projects: Vec<PathBuf> = self.saved.projects.iter().map(|p| p.path.clone()).collect();
        if projects.len() < 2 {
            return;
        }
        cx.spawn(async move |this, cx| {
            let linked = cx
                .background_spawn(async move {
                    projects
                        .into_iter()
                        .map(|p| {
                            // Only a project that is its repository's own tree owns a fold: from
                            // inside a linked worktree the list names that worktree as well.
                            let list = git::list_worktrees(&p).unwrap_or_default();
                            let is_main = list.iter().any(|w| w.main && context::same_folder(&w.path, &p));
                            let others: Vec<PathBuf> =
                                list.into_iter().filter(|w| is_main && !w.main).map(|w| w.path).collect();
                            (p, others)
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            this.update(cx, |this, cx| {
                for (project, others) in linked {
                    for tab in others {
                        let Some(i) = this.saved.projects.iter().position(|p| context::same_folder(&p.path, &tab))
                        else {
                            continue;
                        };
                        let session = this.saved.projects[i].session_id.clone();
                        let contexts =
                            this.contexts.entry(project.clone()).or_insert_with(|| context::Contexts::load(&project));
                        let changed = contexts.adopt_legacy_session(&tab, session.as_deref());
                        if changed && let Err(e) = contexts.save(&project) {
                            this.toast(Toast::error(e));
                        }
                        this.close_project(i);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}
