//! The workspaces section at the top of the sessions column: one row per checkout of
//! the project (its main tree and linked git worktrees) with a state lamp rolled up
//! from the sessions running there, its branch and its changes. Picking one shows
//! its session, which changes the sessions list below to that checkout's.

use std::path::{Path, PathBuf};

use gpui::{Context, prelude::*};
use kynex_core::git::Worktree;
use kynex_core::session::{Kind, Session};
use kynex_core::toasts::Toast;
use kynex_core::workspace::{self, Workspaces};

use super::KynexApp;
use super::state::{Active, View};

/// A checkout with the worktree it came from (for its changes).
pub(super) type Entry = (workspace::Workspace, Option<Worktree>);

impl KynexApp {
    /// The project's saved workspaces, read on first use.
    pub(super) fn saved_workspaces(&mut self, project: &Path) -> &mut Workspaces {
        self.workspaces.entry(project.to_path_buf()).or_insert_with(|| Workspaces::load(project))
    }

    /// The project's workspaces in their saved order, from the worktree list kept
    /// up to date in the background.
    pub(super) fn workspaces_of(&mut self, project: &Path, cx: &mut Context<Self>) -> Vec<Entry> {
        let _t = kynex_core::trace::span("workspaces_of");
        let list = self.worktrees(project, cx).read(cx).list.clone();
        let saved = self.saved_workspaces(project);
        let ordered = saved.ordered(saved.with_folders(workspace::from_worktrees(project, &list)));
        ordered
            .into_iter()
            // A worktree whose folder is gone is no workspace to pick.
            // (A worktree that's gone is `prunable` and already dropped; an added folder
            // that's gone stays, marked, so it can be removed.)
            .map(|c| {
                let worktree = list.iter().find(|w| c.at(&w.path)).cloned();
                (c, worktree)
            })
            .collect()
    }

    /// The code project in the active tab, if it is one.
    pub(super) fn workspace_project(&self) -> Option<PathBuf> {
        self.active_project().map(|p| p.path.clone())
    }

    /// Whose Kynex settings (rules, mode, effort, models) apply in `cwd`: the project
    /// whose tab holds a session there, or that added it as a workspace folder; otherwise
    /// `cwd` itself. A workspace (a worktree, a folder) is part of its project, so it runs
    /// under the project's settings, not under settings of its own that nobody set.
    pub(super) fn settings_dir(&self, cwd: &Path) -> PathBuf {
        let here = |p: &Path| workspace::same_folder(p, cwd);
        if let Some(p) = self.saved.projects.iter().find(|p| here(&p.path)) {
            return p.path.clone();
        }
        for p in &self.saved.projects {
            let open_there = self.code.get(&p.path).is_some_and(|l| l.sessions.iter().any(|s| here(&s.cwd)));
            if open_there || self.workspaces.get(&p.path).is_some_and(|c| c.is_folder(cwd)) {
                return p.path.clone();
            }
        }
        cwd.to_path_buf()
    }

    /// A session about to be shown in the active project's tab runs under that project's
    /// settings when it's in another folder of it (a workspace): its mode too, which a new
    /// session read from its own folder's.
    pub(super) fn adopt_project_settings(&mut self, s: &mut Session) {
        if s.kind != Kind::Code || self.in_chief() {
            return;
        }
        let Some(project) = self.active_project().map(|p| p.path.clone()) else { return };
        if workspace::same_folder(&project, &s.cwd) {
            return;
        }
        s.mode = self.claude_settings(&project).mode;
        s.settings_dir = Some(project);
    }

    /// Writes the project's workspaces file off the UI thread; a failure shows as a notice.
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

    /// The folder you're in: the shown session's, which is the selected workspace's.
    /// The terminal, the Git view and the toolbar's path follow it.
    pub(super) fn workspace_cwd(&self) -> Option<PathBuf> {
        let p = self.active_project()?;
        Some(self.code.get(&p.path).and_then(|l| l.current()).map_or_else(|| p.path.clone(), |s| s.cwd.clone()))
    }

    /// A closed project's terminals and Git views, in each of its workspaces' folders.
    /// `cwds` are the folders its sessions ran in.
    pub(super) fn drop_workspace_views(&mut self, project: &Path, cwds: &[PathBuf]) {
        let made_here = workspace::worktrees_dir(project);
        let ours = |f: &Path| {
            workspace::same_folder(f, project)
                || f.starts_with(&made_here)
                || cwds.iter().any(|c| workspace::same_folder(c, f))
        };
        self.git.retain(|f, _| !ours(f));
        // Dropping a panel ends its shells.
        self.terminals.retain(|f, _| !ours(f));
    }

    /// Shows a workspace: its open session, else the one it last had, else a new one.
    pub(super) fn select_workspace(&mut self, target: &Path, cx: &mut Context<Self>) {
        let _t = kynex_core::trace::span("select_workspace");
        let Some(project) = self.workspace_project() else { return };
        if !target.is_dir() {
            self.toast(Toast::error(format!("{} isn't there any more", target.display())));
            return;
        }
        let leaving = self
            .code
            .get(&project)
            .and_then(|l| l.current())
            .map(|s| (s.cwd.clone(), s.session_id.clone().filter(|_| !s.is_new())));
        let workspaces = self.saved_workspaces(&project);
        if let Some((cwd, Some(id))) = &leaving {
            workspaces.remember(cwd, Some(id));
        }
        workspaces.select(target);
        let remembered = workspaces.session(target).map(str::to_string);
        self.save_workspaces(&project, cx);
        // Sessions are reached through the Code view; the view you were in comes back after.
        let was_git = self.active_project().is_some_and(|p| p.view == View::Git);
        self.show_code_view();
        let there = |s: &Session| s.kind == Kind::Code && workspace::same_folder(&s.cwd, target);
        // The workspace's remembered session if it's open, else any session there.
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

    /// The next (`1`) or previous (`-1`) workspace, wrapping round.
    pub(super) fn step_workspace(&mut self, by: isize, cx: &mut Context<Self>) {
        let Some(project) = self.workspace_project() else { return };
        let Some(cwd) = self.code.get(&project).and_then(|l| l.current()).map(|s| s.cwd.clone()) else { return };
        let list = self.workspaces_of(&project, cx);
        if list.len() < 2 {
            return;
        }
        let at = list.iter().position(|(c, _)| c.at(&cwd)).unwrap_or(0);
        let next = (at as isize + by).rem_euclid(list.len() as isize) as usize;
        self.select_workspace(&list[next].0.path, cx);
    }
}

/// The workspaces of `list`, without their worktrees.
pub(super) fn plain(list: &[Entry]) -> Vec<workspace::Workspace> {
    list.iter().map(|(c, _)| c.clone()).collect()
}
