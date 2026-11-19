//! A workspace stays bound to its branch (`kynex_core::binding`): worktrees Kynex makes
//! are bound to their branch, ones first seen are bound to what they're on, and when a
//! session later switches the branch, one warning says so and offers to keep the new one.
//! The check runs when the worktree list changes, never while drawing.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use gpui::Context;
use kynex_core::binding::{Drift, drift};
use kynex_core::toasts::{Action, Toast};
use kynex_core::workspace::{self, Workspace};

use super::KynexApp;

/// What was warned about, and what a "Keep here" button stands for.
#[derive(Default)]
pub(super) struct Binding {
    /// (workspace folder, branch it was on) pairs already warned about.
    warned: Vec<(PathBuf, String)>,
    /// Per toast button: the project and the folder it's about. One entry per folder.
    rebinds: HashMap<u64, (PathBuf, PathBuf)>,
    seq: u64,
}

impl KynexApp {
    /// A worktree was made on `branch`: the workspace is bound to it.
    pub(super) fn bind_made(&mut self, project: &Path, path: &Path, branch: &str, cx: &mut Context<Self>) {
        self.saved_workspaces(project).bind(path, branch.trim());
        self.save_workspaces(project, cx);
    }

    /// How `w` has drifted from its branch, for its row.
    pub(super) fn drift_of(&self, project: &Path, w: &Workspace) -> Option<Drift> {
        if w.folder {
            return None;
        }
        drift(self.workspaces.get(project)?.bound(&w.path), w.branch.as_deref())
    }

    /// The project's worktree list changed: binds what's new, and warns once about each
    /// workspace that has drifted.
    pub(super) fn worktrees_changed(&mut self, project: &Path, cx: &mut Context<Self>) {
        let Some(entity) = self.worktrees.get(project) else { return };
        let read = &entity.read(cx).list;
        let known = !read.is_empty();
        let list = workspace::from_worktrees(project, read);
        let saved = self.saved_workspaces(project);
        // Pruning only from a list that was read: an empty one is a failed read.
        let pruned = known && saved.prune_bindings(&list);
        if saved.bind_unbound(&list) || pruned {
            self.save_workspaces(project, cx);
        }
        for w in list.iter().filter(|w| !w.folder) {
            let Some(d) = self.drift_of(project, w) else {
                self.binding.warned.retain(|(f, _)| !w.at(f));
                self.binding.rebinds.retain(|_, (_, f)| !w.at(f));
                continue;
            };
            if self.binding.warned.iter().any(|(f, b)| w.at(f) && *b == d.live) {
                continue;
            }
            self.binding.warned.push((w.path.clone(), d.live.clone()));
            self.binding.seq += 1;
            let id = self.binding.seq;
            self.binding.rebinds.retain(|_, (_, f)| !w.at(f));
            self.binding.rebinds.insert(id, (project.to_path_buf(), w.path.clone()));
            self.toast(
                Toast::warning(format!("Workspace \u{201c}{}\u{201d} is on {}", w.label, d.live))
                    .detail(format!(
                        "It belongs to {}. A session switched its branch; new branch work should go to a worktree of its own.",
                        d.bound
                    ))
                    .action(Action::RebindWorkspace(id)),
            );
            cx.notify();
        }
    }

    /// "Keep here" on a drift warning: the workspace belongs to the branch it is on now
    /// (read at click time); nothing if it's back on its own or detached.
    pub(super) fn rebind_workspace(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some((project, folder)) = self.binding.rebinds.remove(&id) else { return };
        let live = self
            .worktrees
            .get(&project)
            .and_then(|w| w.read(cx).list.iter().find(|w| workspace::same_folder(&w.path, &folder)).cloned())
            .and_then(|w| w.branch);
        let saved = self.saved_workspaces(&project);
        let Some(d) = drift(saved.bound(&folder), live.as_deref()) else { return };
        saved.bind(&folder, &d.live);
        self.save_workspaces(&project, cx);
        self.binding.warned.retain(|(f, _)| !workspace::same_folder(f, &folder));
        cx.notify();
    }
}
