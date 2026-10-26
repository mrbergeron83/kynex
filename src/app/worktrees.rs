//! The project's git worktrees, at the bottom of the sessions panel: one row
//! per worktree with its branch, changed files, distance from main, and
//! whether a session is working in it. Clicking one opens it as a project tab.

use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui::{Context, Entity, prelude::*};
use kynex_core::git::{self, Worktree};
use kynex_core::session::{Item, Kind};

use super::KynexApp;
use super::state::{Active, View};

/// Worktrees change behind our back (Claude makes and removes them), so re-read often.
const REFRESH_EVERY: Duration = Duration::from_secs(10);

/// A project's worktrees, re-read in the background while the project is open.
pub struct Worktrees {
    pub list: Vec<Worktree>,
}

impl Worktrees {
    fn new(root: PathBuf, cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this, cx| {
            loop {
                let root = root.clone();
                let list = cx.background_spawn(async move { git::list_worktrees(&root).unwrap_or_default() }).await;
                let alive = this.update(cx, |this, cx| {
                    if this.list != list {
                        this.list = list;
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
                cx.background_executor().timer(REFRESH_EVERY).await;
            }
        })
        .detach();
        Self { list: Vec::new() }
    }

    /// The worktree whose folder is `path`.
    pub fn at(&self, path: &Path) -> Option<&Worktree> {
        self.list.iter().find(|w| w.path == path)
    }
}

impl KynexApp {
    /// The worktree list for a project folder, started on first use.
    pub(super) fn worktrees(&mut self, path: &Path, cx: &mut Context<Self>) -> Entity<Worktrees> {
        if let Some(w) = self.worktrees.get(path) {
            return w.clone();
        }
        let root = path.to_path_buf();
        let entity = cx.new(|cx| Worktrees::new(root, cx));
        cx.observe(&entity, |_, _, cx| cx.notify()).detach();
        self.worktrees.insert(path.to_path_buf(), entity.clone());
        entity
    }

    /// The linked worktree the shown session is working in, judging by its most
    /// recent tool call that points into one (Claude often works in a worktree
    /// by path while the session itself runs in the main folder).
    pub(super) fn working_worktree(&mut self, cx: &mut Context<Self>) -> Option<Worktree> {
        let s = self.active_session().filter(|s| s.kind == Kind::Code)?;
        let cwd = s.cwd.clone();
        let inputs: Vec<String> = s
            .items
            .iter()
            .rev()
            .filter_map(|i| match i {
                Item::ToolUse { input, .. } => Some(input.clone()),
                _ => None,
            })
            .take(300)
            .collect();
        let list = self.worktrees(&cwd, cx).read(cx).list.clone();
        inputs.iter().find_map(|input| git::worktree_in(input, &list)).filter(|w| w.path != cwd).cloned()
    }

    /// Opens a worktree as a tab, on its Git view, to look at its changes.
    pub(super) fn open_worktree_changes(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.add_project(path);
        if let Active::Project(i) = self.saved.active
            && let Some(p) = self.saved.projects.get_mut(i)
        {
            p.view = View::Git;
        }
        self.save();
        cx.notify();
    }
}
