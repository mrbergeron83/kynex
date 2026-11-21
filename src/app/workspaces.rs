//! Workspaces (a project's main checkout, linked git worktrees, and added folders).

use std::path::{Path, PathBuf};

use gpui::{AppContext, Context};
use kynex_core::git::Worktree;
use kynex_core::toasts::Toast;
use kynex_core::workspace::{self, Workspaces};

use super::KynexApp;

pub(super) type Entry = (workspace::Workspace, Option<Worktree>);

impl KynexApp {
    pub(super) fn saved_workspaces(&mut self, project: &Path) -> &mut Workspaces {
        self.workspaces.entry(project.to_path_buf()).or_insert_with(|| Workspaces::load(project))
    }

    pub(super) fn workspaces_of(&mut self, project: &Path, cx: &mut Context<Self>) -> Vec<Entry> {
        let _t = kynex_core::trace::span("workspaces_of");
        let list = self.worktrees(project, cx).read(cx).list.clone();
        let saved = self.saved_workspaces(project);
        let ordered = saved.ordered(saved.with_folders(workspace::from_worktrees(project, &list)));
        ordered
            .into_iter()
            .map(|c| {
                let worktree = list.iter().find(|w| c.at(&w.path)).cloned();
                (c, worktree)
            })
            .collect()
    }

    pub(super) fn workspace_project(&self) -> Option<PathBuf> {
        self.active_project().map(|p| p.path.clone())
    }

    pub(super) fn save_workspaces(&mut self, project: &Path, cx: &mut Context<Self>) {
        let Some(workspaces) = self.workspaces.get(project).cloned() else { return };
        let project = project.to_path_buf();
        cx.spawn(async move |this, cx| {
            if let Err(e) = cx.background_spawn(async move { workspaces.save(&project) }).await {
                this.update(cx, |this, cx| {
                    this.toast(Toast::error(e));
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    /// The selected workspace's folder for the active project (defaults to the project root).
    pub(super) fn workspace_cwd(&self) -> Option<PathBuf> {
        let p = self.active_project()?;
        let selected =
            self.workspaces.get(&p.path).and_then(|w| w.selected.as_ref()).filter(|dir| dir.is_dir()).cloned();
        Some(selected.unwrap_or_else(|| p.path.clone()))
    }

    pub(super) fn drop_workspace_views(&mut self, project: &Path, known: &[PathBuf]) {
        let own_root = kynex_core::project::WorktreeSettings::load(project).root.is_some();
        let made_here = (!own_root).then(|| workspace::worktrees_dir(project));
        let ours = |f: &Path| {
            workspace::same_folder(f, project)
                || made_here.as_ref().is_some_and(|m| f.starts_with(m))
                || known.iter().any(|k| workspace::same_folder(k, f))
        };
        self.git_panes.retain(|f, _| !ours(f));
        self.terminals.retain(|(f, _)| !ours(f));
    }

    pub(super) fn select_workspace(&mut self, target: &Path, cx: &mut Context<Self>) {
        let _t = kynex_core::trace::span("select_workspace");
        let Some(project) = self.workspace_project() else { return };
        if !target.is_dir() {
            self.toast(Toast::error(format!("{} isn't there any more", target.display())));
            return;
        }
        let workspaces = self.saved_workspaces(&project);
        workspaces.select(target);
        self.save_workspaces(&project, cx);
        cx.notify();
    }

    pub(super) fn step_workspace(&mut self, by: isize, cx: &mut Context<Self>) {
        let Some(project) = self.workspace_project() else { return };
        let Some(cwd) = self.workspace_cwd() else { return };
        let list = self.workspaces_of(&project, cx);
        if list.len() < 2 {
            return;
        }
        let at = list.iter().position(|(c, _)| c.at(&cwd)).unwrap_or(0);
        let next = (at as isize + by).rem_euclid(list.len() as isize) as usize;
        self.select_workspace(&list[next].0.path, cx);
    }
}
