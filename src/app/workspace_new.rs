//! Making and removing workspaces from the Sessions tree, and folding worktrees that
//! were open as separate project tabs into their project.

use std::path::{Path, PathBuf};

use gpui::{AnyElement, Context, Entity, Focusable, Window, div, prelude::*};
use kynex_core::git;
use kynex_core::removed;
use kynex_core::sessions;
use kynex_core::toasts::Toast;
use kynex_core::workspace;

use super::KynexApp;
use crate::theme::theme;
use crate::ui::input::{InputEvent, TextInput};
use crate::ui::{Icon, Lamp, Tooltip, icon_button, menu_item, name_field, outline_button, primary_button, px, spinner};

#[derive(Default)]
pub(super) struct WorkspaceUi {
    form: Option<Entity<TextInput>>,
    branches: Vec<String>,
    busy: bool,
    creating: Option<(String, Entity<TextInput>)>,
    removing: Option<Removal>,
}

struct Removal {
    path: PathBuf,
    label: String,
    why: Vec<String>,
    sessions: usize,
    delete_sessions: bool,
}

impl KynexApp {
    pub(super) fn add_workspace_menu_items(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(project) = self.workspace_project() else { return Vec::new() };
        let repo = !self.worktrees(&project, cx).read(cx).list.is_empty();
        let mut items = Vec::new();
        if repo {
            items.push(
                menu_item("add-workspace-branch", "New worktree on a branch…", false, cx)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.menu = None;
                        this.open_new_workspace(window, cx);
                    }))
                    .into_any_element(),
            );
        }
        items.push(
            menu_item("add-workspace-folder", "Add a folder…", false, cx)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.menu = None;
                    this.add_folder_workspace(cx);
                }))
                .into_any_element(),
        );
        items
    }

    pub(super) fn open_new_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(project) = self.workspace_project() else { return };
        let input = cx.new(|cx| TextInput::new(cx).placeholder("Branch, new or existing"));
        cx.subscribe_in(&input, window, |this, _, ev: &InputEvent, window, cx| match ev {
            InputEvent::Submit => this.submit_new_workspace(window, cx),
            InputEvent::Changed => cx.notify(),
            _ => {}
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
                    let checkout = this
                        .worktrees
                        .get(&project)
                        .is_some_and(|w| w.read(cx).list.iter().any(|w| workspace::same_folder(&w.path, &dir)));
                    if !checkout {
                        this.saved_workspaces(&project).add_folder(&project, &dir);
                    }
                    this.select_workspace(&dir, cx);
                })
                .ok();
            }
        })
        .detach();
    }

    pub(super) fn cancel_new_workspace(&mut self) -> bool {
        if self.workspace_ui.creating.is_some() {
            return false;
        }
        let open = self.workspace_ui.form.is_some() || self.workspace_ui.removing.is_some();
        self.workspace_ui = WorkspaceUi::default();
        open
    }

    fn submit_new_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = &self.workspace_ui.form else { return };
        let name = input.read(cx).text().trim().to_string();
        let Some(project) = self.workspace_project() else { return };
        if name.is_empty() || self.workspace_ui.busy {
            return;
        }
        self.workspace_ui.busy = true;
        self.workspace_ui.creating = self.workspace_ui.form.take().map(|input| (name.clone(), input));
        self.focus_input(window, cx);
        cx.spawn(async move |this, cx| {
            let root = project.clone();
            let branch = name.clone();
            let made = cx
                .background_spawn(async move {
                    let base = git::default_branch(&root);
                    workspace::create(&root, &name, base.as_deref())
                })
                .await;
            this.update(cx, |this, cx| {
                this.workspace_ui.busy = false;
                let typed = this.workspace_ui.creating.take();
                match made {
                    Ok(path) => {
                        this.bind_made(&project, &path, &branch, cx);
                        this.refresh_worktrees([&project], cx);
                        this.select_workspace(&path, cx);
                    }
                    Err(e) => {
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

    pub(super) fn remove_workspace(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let Some(project) = self.workspace_project() else { return };
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
        let running = self.live_agents_in(&path).iter().any(|(_, _, _, l)| matches!(l, Lamp::Working | Lamp::NeedsYou));
        if running {
            why.push("an agent still working".to_string());
        }
        let counted = path.clone();
        cx.spawn(async move |this, cx| {
            let count = cx.background_spawn(async move { sessions::list_sessions(&counted).len() }).await;
            this.update(cx, |this, cx| {
                if why.is_empty() && count == 0 {
                    return this.confirm_remove_workspace(path, false, false, None, cx);
                }
                this.workspace_ui = WorkspaceUi {
                    removing: Some(Removal { path, label, why, sessions: count, delete_sessions: false }),
                    ..WorkspaceUi::default()
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn note_removed(
        &mut self,
        project: PathBuf,
        path: PathBuf,
        label: String,
        branch: Option<String>,
        delete: bool,
        cx: &mut Context<Self>,
    ) {
        let there = path.clone();
        cx.spawn(async move |this, cx| {
            let (kept, purged) = cx
                .background_spawn(async move {
                    if delete {
                        let purged = removed::delete_sessions(&there);
                        (!purged.errors.is_empty(), purged)
                    } else {
                        (!sessions::list_sessions(&there).is_empty(), removed::Purged::default())
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                this.edit_groups(&path, |g| purged.deleted.iter().for_each(|id| g.forget(id)));
                if let Some(e) = purged.errors.first() {
                    this.toast(Toast::error(format!("Couldn't delete {} session(s): {e}", purged.errors.len())));
                }
                if kept && this.saved_workspaces(&project).remember_removed(&path, &label, branch.as_deref()) {
                    this.save_workspaces(&project, cx);
                }
                this.reread_sessions();
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn confirm_remove_workspace(
        &mut self,
        path: PathBuf,
        force: bool,
        delete_sessions: bool,
        done: Option<std::sync::mpsc::Sender<Result<serde_json::Value, String>>>,
        cx: &mut Context<Self>,
    ) {
        self.workspace_ui = WorkspaceUi::default();
        let Some(project) = self.workspace_project() else { return };
        let worktree = self.worktrees.get(&project).and_then(|w| w.read(cx).at(&path).cloned());
        let label = worktree.as_ref().map_or_else(
            || path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned()),
            |w| w.label().to_string(),
        );
        let branch = worktree.and_then(|w| w.branch);
        let shown_there = self.workspace_cwd().is_some_and(|c| workspace::same_folder(&c, &path));

        // Close all agent CLIs and plain terminals standing in `path` so Windows unlocks the folder.
        self.drop_tiles_of(&path, cx);
        self.drop_gone_agents(cx);
        self.drop_gone_claude_chats(cx);
        self.drop_gone_terminals(cx);
        self.terminals.retain(|(f, _)| !workspace::same_folder(f, &path));
        self.git_panes.retain(|f, _| !workspace::same_folder(f, &path));

        let workspaces =
            self.workspaces.entry(project.clone()).or_insert_with(|| workspace::Workspaces::load(&project));
        if workspaces.selected.as_deref().is_some_and(|s| workspace::same_folder(s, &path)) {
            workspaces.selected = None;
        }
        if shown_there {
            self.select_workspace(&project, cx);
        }
        if self.saved_workspaces(&project).is_folder(&path) {
            self.saved_workspaces(&project).remove_folder(&path);
            self.save_workspaces(&project, cx);
            if let Some(done) = done {
                let _ = done.send(Ok(serde_json::json!({ "removed": path })));
            }
            self.note_removed(project, path, label, branch, delete_sessions, cx);
            return cx.notify();
        }
        let root = project.clone();
        let gone = path.clone();
        cx.spawn(async move |this, cx| {
            let removed = cx.background_spawn(async move { workspace::remove(&root, &gone, force) }).await;
            this.update(cx, |this, cx| {
                match removed {
                    Ok(()) => {
                        this.workspaces.entry(project.clone()).or_default().forget(&path);
                        this.save_workspaces(&project, cx);
                        if let Some(done) = &done {
                            let _ = done.send(Ok(serde_json::json!({ "removed": path })));
                        }
                        this.note_removed(project.clone(), path.clone(), label, branch, delete_sessions, cx);
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
            let (sessions, delete, force) = (r.sessions, r.delete_sessions, !r.why.is_empty());
            let question = if force {
                format!("Remove {}? It has {}.", r.label, r.why.join(" and "))
            } else {
                format!("Remove {}?", r.label)
            };
            let tick = self.pick_box(delete, cx);
            out.push(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .px_2()
                    .pt_1()
                    .child(div().text_xs().text_color(if force { t.warning } else { t.text }).child(question))
                    .when(sessions > 0, |d| {
                        d.child(
                            div()
                                .id("remove-workspace-sessions")
                                .flex()
                                .items_center()
                                .gap_1p5()
                                .cursor_pointer()
                                .text_xs()
                                .text_color(t.text_muted)
                                .child(tick)
                                .child(format!(
                                    "Also delete its {} (they stay under Removed workspaces otherwise)",
                                    super::storage::count(sessions, "session")
                                ))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if let Some(r) = &mut this.workspace_ui.removing {
                                        r.delete_sessions = !r.delete_sessions;
                                    }
                                    cx.notify();
                                })),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .gap_1()
                            .child(
                                primary_button(
                                    "remove-workspace-yes",
                                    if force { "Remove anyway" } else { "Remove" },
                                    cx,
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.confirm_remove_workspace(path.clone(), force, delete, None, cx)
                                })),
                            )
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

    pub(super) fn open_as_workspace(&mut self, path: &Path, cx: &mut Context<Self>) -> bool {
        let owner = self.saved.projects.iter().position(|p| {
            !workspace::same_folder(&p.path, path)
                && self
                    .worktrees
                    .get(&p.path)
                    .is_some_and(|w| w.read(cx).list.iter().any(|w| !w.main && workspace::same_folder(&w.path, path)))
        });
        let Some(i) = owner else { return false };
        if self.saved.active_project != Some(i) {
            self.saved.active_project = Some(i);
            self.menu = None;
            self.save();
        }
        self.select_workspace(path, cx);
        true
    }

    pub(super) fn open_workspace_tab(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.add_project(path);
        if let Some(i) = self.saved.active_project
            && let Some(p) = self.saved.projects.get_mut(i)
        {
            p.own_tab = true;
        }
        self.save();
        cx.notify();
    }

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
                for (_project, others) in linked {
                    for tab in others {
                        let Some(i) = this
                            .saved
                            .projects
                            .iter()
                            .position(|p| !p.own_tab && workspace::same_folder(&p.path, &tab))
                        else {
                            continue;
                        };
                        this.close_project(i, cx);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}
