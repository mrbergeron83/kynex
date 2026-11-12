//! The control socket's `workspace` commands: the shown project's checkouts, listed, shown,
//! made and removed with the same code the sidebar's buttons use.

use std::path::PathBuf;
use std::sync::mpsc::Sender;

use gpui::{AppContext, Context};
use kynex_core::git;
use kynex_core::workspace::{self, same_folder};
use serde_json::{Value, json};

use super::KynexApp;

type Answer = Result<Value, String>;

impl KynexApp {
    /// The shown project's workspaces, in the sidebar's order.
    pub(super) fn workspace_list(&mut self, cx: &mut Context<Self>) -> Result<Value, String> {
        let project = self.workspace_project().ok_or("no project is shown")?;
        let selected = self.saved_workspaces(&project).selected.clone();
        let list: Vec<Value> = self
            .workspaces_of(&project, cx)
            .into_iter()
            .map(|(w, tree)| {
                json!({
                    "path": w.path,
                    "label": w.label,
                    "branch": w.branch,
                    "main": w.main,
                    "folder": w.folder,
                    "changes": tree.and_then(|t| t.changes),
                    "selected": selected.as_deref().is_some_and(|s| same_folder(s, &w.path)),
                })
            })
            .collect();
        Ok(json!({ "project": project, "workspaces": list }))
    }

    /// The shown project, and the folder of its workspace that `path` names.
    pub(super) fn workspace_named(&mut self, path: &str, cx: &mut Context<Self>) -> Result<(PathBuf, PathBuf), String> {
        let project = self.workspace_project().ok_or("no project is shown")?;
        let wanted = PathBuf::from(path);
        let found = self.workspaces_of(&project, cx).into_iter().find(|(w, _)| w.at(&wanted));
        match found {
            Some((w, _)) => Ok((project, w.path)),
            None => Err(format!("{path} isn't a workspace of the shown project (see `knx workspace list`)")),
        }
    }

    /// Makes a workspace on `branch` for the shown project and shows it; `reply` gets its folder
    /// once git is done.
    pub(super) fn create_workspace_for(&mut self, branch: String, reply: Sender<Answer>, cx: &mut Context<Self>) {
        let Some(project) = self.workspace_project() else {
            let _ = reply.send(Err("no project is shown".into()));
            return;
        };
        cx.spawn(async move |this, cx| {
            let root = project.clone();
            let made = cx
                .background_spawn(async move {
                    let base = git::default_branch(&root);
                    workspace::create(&root, &branch, base.as_deref())
                })
                .await;
            this.update(cx, |this, cx| {
                match made {
                    Ok(path) => {
                        this.refresh_worktrees([&project], cx);
                        // The project may have been switched meanwhile; the workspace is its own project's.
                        if this.workspace_project().as_deref() == Some(project.as_path()) {
                            this.select_workspace(&path, cx);
                        }
                        let _ = reply.send(Ok(json!({ "created": path })));
                    }
                    Err(e) => {
                        let _ = reply.send(Err(e));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Removes the workspace at `path` (one `workspace_named` found); `reply` gets the outcome.
    /// The project's own working tree can't be removed.
    pub(super) fn remove_workspace_for(
        &mut self,
        path: PathBuf,
        force: bool,
        reply: Sender<Answer>,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let project = self.workspace_project().ok_or("no project is shown")?;
        let is_main = self.workspaces_of(&project, cx).iter().any(|(w, _)| w.at(&path) && w.main);
        if is_main || same_folder(&project, &path) {
            return Err("the project's own folder can't be removed".into());
        }
        self.confirm_remove_workspace(path, force, false, Some(reply), cx);
        Ok(())
    }
}
