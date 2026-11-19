//! The control socket's `workspace` commands: the shown project's checkouts, listed, shown,
//! made and removed with the same code the sidebar's buttons use.

use std::path::PathBuf;
use std::sync::mpsc::Sender;

use gpui::{AppContext, Context};
use kynex_core::control::approval::Made;
use kynex_core::git;
use kynex_core::live::Key;
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
    /// once git is done. With `session` (`Some(prompt)`), a session in it too, sent the prompt.
    pub(super) fn create_workspace_for(
        &mut self,
        branch: String,
        session: Option<(Option<String>, Option<String>)>,
        caller: Option<Key>,
        reply: Sender<Answer>,
        cx: &mut Context<Self>,
    ) {
        // The asking session's project, which may not be the one shown.
        let project = match self.request_project(caller) {
            Ok(p) => p,
            Err(e) => {
                let _ = reply.send(Err(e));
                return;
            }
        };
        cx.spawn(async move |this, cx| {
            let root = project.clone();
            let name = branch.clone();
            let made = cx
                .background_spawn(async move {
                    let base = git::default_branch(&root);
                    workspace::create(&root, &branch, base.as_deref())
                })
                .await;
            this.update(cx, |this, cx| {
                match made {
                    Ok(path) => {
                        this.bind_made(&project, &path, &name, cx);
                        this.refresh_worktrees([&project], cx);
                        this.approvals.record_made(Made::Workspace(path.clone()), caller);
                        // The project may have been switched meanwhile; the workspace is its own project's.
                        let shown = this.workspace_project().as_deref() == Some(project.as_path());
                        if shown {
                            this.select_workspace(&path, cx);
                        }
                        let answer = match session {
                            None => Ok(json!({ "created": path })),
                            Some((prompt, why)) => this
                                .session_in_new_workspace(&project, path.clone(), prompt, None, why, shown, caller)
                                .map(|s| json!({ "created": path, "session": s["session"] }))
                                .map_err(|e| format!("made {}, but no session in it: {e}", path.display())),
                        };
                        let _ = reply.send(answer);
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
