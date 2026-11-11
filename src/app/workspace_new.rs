//! Making and removing workspaces from the sessions column: the "New workspace" form (a
//! branch name, new or existing, off the default branch), the confirmation before a
//! worktree with changes or a running session goes, and folding worktrees that were
//! open as project tabs into their project.

use std::path::{Path, PathBuf};

use gpui::{AnyElement, Context, Entity, Focusable, Window, div, prelude::*};
use kynex_core::git;
use kynex_core::toasts::Toast;
use kynex_core::workspace;

use super::KynexApp;
use super::state::Active;
use crate::theme::theme;
use crate::ui::input::{InputEvent, TextInput};
use crate::ui::{Icon, Tooltip, icon_button, name_field, outline_button, primary_button, px, spinner};

/// What's open in the workspaces section besides its rows.
#[derive(Default)]
pub(super) struct WorkspaceUi {
    /// The "New workspace" branch field, while it's open.
    form: Option<Entity<TextInput>>,
    /// Local branches without a checkout, to pick from while naming one.
    branches: Vec<String>,
    /// A workspace is being made.
    busy: bool,
    /// Its branch name and the field it was typed in (kept to come back if it fails): the
    /// form closes at once and a "Creating…" row stands in while `git` makes the worktree.
    creating: Option<(String, Entity<TextInput>)>,
    /// A workspace waiting on a yes before it goes.
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
    /// Opens the "New workspace" field, with the branches that could be checked out.
    pub(super) fn open_new_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(project) = self.workspace_project() else { return };
        let input = cx.new(|cx| TextInput::new(cx).placeholder("Branch, new or existing"));
        cx.subscribe_in(&input, window, |this, _, ev: &InputEvent, window, cx| match ev {
            InputEvent::Submit => this.submit_new_workspace(window, cx),
            InputEvent::Changed => cx.notify(),
            InputEvent::PasteImages(_) => {}
        })
        .detach();
        input.read(cx).focus_handle(cx).focus(window, cx);
        self.workspace_ui = WorkspaceUi { form: Some(input), ..WorkspaceUi::default() };
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
                this.workspace_ui.branches = names;
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Asks for a folder and adds it as a workspace (scripts, notes: not a checkout), then shows it.
    pub(super) fn add_folder_workspace(&mut self, cx: &mut Context<Self>) {
        let Some(project) = self.workspace_project() else { return };
        let picked = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Add folder".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = picked.await
                && let Some(dir) = paths.into_iter().next()
            {
                this.update(cx, |this, cx| {
                    // A checkout of the project is already a workspace: just show it.
                    let checkout = this
                        .worktrees
                        .get(&project)
                        .is_some_and(|w| w.read(cx).list.iter().any(|w| workspace::same_folder(&w.path, &dir)));
                    if !checkout {
                        this.saved_workspaces(&project).add_folder(&project, &dir);
                    }
                    // (Selecting it saves the list.)
                    this.select_workspace(&dir, cx);
                })
                .ok();
            }
        })
        .detach();
    }

    /// Esc: closes the new-workspace field or the removal question. Returns whether one was open.
    pub(super) fn cancel_new_workspace(&mut self) -> bool {
        // One being made can't be taken back from here.
        if self.workspace_ui.creating.is_some() {
            return false;
        }
        let open = self.workspace_ui.form.is_some() || self.workspace_ui.removing.is_some();
        self.workspace_ui = WorkspaceUi::default();
        open
    }

    /// Enter in the field: makes the workspace and shows it, with a new session focused.
    fn submit_new_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = &self.workspace_ui.form else { return };
        let name = input.read(cx).text().trim().to_string();
        let Some(project) = self.workspace_project() else { return };
        if name.is_empty() || self.workspace_ui.busy {
            return;
        }
        self.workspace_ui.busy = true;
        // The form goes at once; the worktree takes a second or more to check out.
        self.workspace_ui.creating = self.workspace_ui.form.take().map(|input| (name.clone(), input));
        // Whichever session the new workspace opens with, the composer takes the keys.
        self.focus_input(window, cx);
        cx.spawn(async move |this, cx| {
            let root = project.clone();
            let made = cx
                .background_spawn(async move {
                    let _t = kynex_core::trace::span("new workspace: default branch + git worktree add (background)");
                    let base = git::default_branch(&root);
                    workspace::create(&root, &name, base.as_deref())
                })
                .await;
            this.update(cx, |this, cx| {
                let _t = kynex_core::trace::span("new workspace: showing it (UI thread)");
                this.workspace_ui.busy = false;
                let typed = this.workspace_ui.creating.take();
                match made {
                    Ok(path) => {
                        this.refresh_worktrees([&project], cx);
                        this.select_workspace(&path, cx);
                    }
                    Err(e) => {
                        // Back to the field, with the name still in it.
                        this.workspace_ui.form = typed.map(|(_, input)| input);
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

    /// Remove on a workspace's menu: asks first when it has changes or a running
    /// session, otherwise goes at once.
    pub(super) fn remove_workspace(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let Some(project) = self.workspace_project() else { return };
        // A folder added by hand leaves the list; a worktree is removed from disk.
        let is_folder = self.saved_workspaces(&project).is_folder(&path);
        let worktree = self.worktrees.get(&project).and_then(|w| w.read(cx).at(&path).cloned());
        let mut why = Vec::new();
        let label = if is_folder {
            path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
        } else {
            let Some(worktree) = worktree.filter(|w| !w.main) else { return };
            if let Some(n) = worktree.changes.filter(|n| *n > 0) {
                why.push(format!("{n} changed file{}", if n == 1 { "" } else { "s" }));
            }
            worktree.label().to_string()
        };
        let running = self.code.get(&project).is_some_and(|l| {
            l.sessions.iter().any(|s| workspace::same_folder(&s.cwd, &path) && (s.busy || s.waiting()))
        });
        if running {
            why.push("a session still working".to_string());
        }
        if why.is_empty() {
            return self.confirm_remove_workspace(path, false, None, cx);
        }
        self.workspace_ui = WorkspaceUi { removing: Some(Removal { path, label, why }), ..WorkspaceUi::default() };
        cx.notify();
    }

    /// Closes the workspace's sessions, then `git worktree remove`; the branch stays.
    pub(super) fn confirm_remove_workspace(
        &mut self,
        path: PathBuf,
        force: bool,
        done: Option<std::sync::mpsc::Sender<Result<serde_json::Value, String>>>,
        cx: &mut Context<Self>,
    ) {
        self.workspace_ui = WorkspaceUi::default();
        let Some(project) = self.workspace_project() else { return };
        // Sessions there go first: a process in the folder keeps Windows from removing it.
        let shown_there =
            self.code.get(&project).and_then(|l| l.current()).is_some_and(|s| workspace::same_folder(&s.cwd, &path));
        // If the removal fails, picking the workspace again resumes the session it had.
        let kept = self.code.get(&project).and_then(|l| {
            l.sessions
                .iter()
                .filter(|s| workspace::same_folder(&s.cwd, &path) && !s.is_new())
                .rev()
                .find_map(|s| s.session_id.clone())
        });
        let workspaces =
            self.workspaces.entry(project.clone()).or_insert_with(|| workspace::Workspaces::load(&project));
        workspaces.remember(&path, kept.as_deref());
        if workspaces.selected.as_deref().is_some_and(|s| workspace::same_folder(s, &path)) {
            workspaces.selected = None;
        }
        if let Some(live) = self.code.get_mut(&project) {
            let closed = live.close_where(|s| workspace::same_folder(&s.cwd, &path));
            self.forget(&closed);
        }
        // Its shells and Git view too: a shell standing in the folder blocks removing it.
        self.terminals.retain(|(f, _)| !workspace::same_folder(f, &path));
        self.git.retain(|f, _| !workspace::same_folder(f, &path));
        if shown_there {
            self.select_workspace(&project, cx);
        }
        // A folder added by hand only leaves the list; nothing on disk goes.
        if self.saved_workspaces(&project).is_folder(&path) {
            self.saved_workspaces(&project).remove_folder(&path);
            self.save_workspaces(&project, cx);
            if let Some(done) = done {
                let _ = done.send(Ok(serde_json::json!({ "removed": path })));
            }
            return cx.notify();
        }
        let root = project.clone();
        let gone = path.clone();
        cx.spawn(async move |this, cx| {
            let removed = cx.background_spawn(async move { workspace::remove(&root, &gone, force) }).await;
            this.update(cx, |this, cx| {
                match removed {
                    Ok(()) => {
                        this.drop_tiles_of(&path);
                        this.workspaces.entry(project.clone()).or_default().forget(&path);
                        this.save_workspaces(&project, cx);
                        if let Some(done) = &done {
                            let _ = done.send(Ok(serde_json::json!({ "removed": path })));
                        }
                    }
                    Err(e) => {
                        if let Some(done) = &done {
                            let _ = done.send(Err(e.clone()));
                        }
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

    /// The form and the question, under the workspace rows.
    pub(super) fn render_workspace_forms(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let t = theme(cx).clone();
        let mut out = Vec::new();
        if let Some((name, _)) = &self.workspace_ui.creating {
            out.push(
                div()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .h(px(28.))
                    .pl_2()
                    .text_sm()
                    .text_color(t.text_muted)
                    .child(spinner("creating-workspace-spin", t.accent, px(12.)))
                    .child(div().flex_1().min_w_0().truncate().child(format!("Creating {name}…")))
                    .into_any_element(),
            );
        }
        if let Some(input) = self.workspace_ui.form.clone() {
            let typed = input.read(cx).text().trim().to_lowercase();
            let picks: Vec<String> = self
                .workspace_ui
                .branches
                .iter()
                .filter(|b| typed.is_empty() || b.to_lowercase().contains(&typed))
                .take(5)
                .cloned()
                .collect();
            let busy = self.workspace_ui.busy;
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
                                primary_button("new-workspace-ok", "Create", cx)
                                    .when(busy, |b| b.opacity(0.5))
                                    .on_click(cx.listener(|this, _, window, cx| this.submit_new_workspace(window, cx))),
                            )
                            .child(
                                icon_button("new-workspace-close", Icon::Close, cx)
                                    .tooltip(Tooltip::with_meta("Cancel", "Esc"))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.cancel_new_workspace();
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
                            .id(("workspace-pick", n))
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
                                if let Some(input) = &this.workspace_ui.form {
                                    input.update(cx, |i, cx| i.set_text(&name, cx));
                                }
                                this.submit_new_workspace(window, cx);
                            }))
                    }))
                    .into_any_element(),
            );
        }
        if let Some(r) = &self.workspace_ui.removing {
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
                            .child(primary_button("remove-workspace-yes", "Remove anyway", cx).on_click(cx.listener(
                                move |this, _, _, cx| this.confirm_remove_workspace(path.clone(), true, None, cx),
                            )))
                            .child(outline_button("remove-workspace-no", "Keep", cx).on_click(cx.listener(
                                |this, _, _, cx| {
                                    this.cancel_new_workspace();
                                    cx.notify();
                                },
                            ))),
                    )
                    .into_any_element(),
            );
        }
        out
    }

    /// Shows `path` as a workspace of the open project it's a worktree of, if there
    /// is one. Returns whether it did.
    pub(super) fn open_as_workspace(&mut self, path: &Path, cx: &mut Context<Self>) -> bool {
        let owner = self.saved.projects.iter().position(|p| {
            !workspace::same_folder(&p.path, path)
                && self
                    .worktrees
                    .get(&p.path)
                    .is_some_and(|w| w.read(cx).list.iter().any(|w| !w.main && workspace::same_folder(&w.path, path)))
        });
        let Some(i) = owner else { return false };
        if self.saved.active != Active::Project(i) {
            self.saved.active = Active::Project(i);
            self.menu = None;
            self.save();
        }
        self.select_workspace(path, cx);
        true
    }

    /// A workspace's "Open in a new tab": the folder as a project tab of its own, which
    /// stays one (it isn't folded back into its project at startup).
    pub(super) fn open_workspace_tab(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.add_project(path.clone());
        if let Active::Project(i) = self.saved.active
            && let Some(p) = self.saved.projects.get_mut(i)
        {
            p.own_tab = true;
        }
        self.save();
        cx.notify();
    }

    /// Worktrees opened as project tabs by earlier versions fold into the project
    /// they belong to, keeping the session each had as that workspace's. Reads
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
                            let is_main = list.iter().any(|w| w.main && workspace::same_folder(&w.path, &p));
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
                        let Some(i) = this
                            .saved
                            .projects
                            .iter()
                            .position(|p| !p.own_tab && workspace::same_folder(&p.path, &tab))
                        else {
                            continue;
                        };
                        let session = this.saved.projects[i].session_id.clone();
                        let workspaces = this
                            .workspaces
                            .entry(project.clone())
                            .or_insert_with(|| workspace::Workspaces::load(&project));
                        let changed = workspaces.adopt_legacy_session(&tab, session.as_deref());
                        if changed && let Err(e) = workspaces.save(&project) {
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
