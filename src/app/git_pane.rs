//! The workspace's Git pane, from the app's side: making it, opening it where it was last
//! (the toolbar's button, its key, the palette, the branch chip), closing it, and what it
//! asks for (a file's diff in a modal). One per workspace; asking for another focuses it.
//! Chat has no panes: there the Git modal shows the session's folder instead.

use std::path::{Path, PathBuf};

use gpui::{AppContext, Context, Entity, Window};
use kynex_core::git::{self, Change, Staged};
use kynex_core::layout::PaneId;
use kynex_core::tiling::{Content, Tool};
use kynex_core::toasts::Toast;
use kynex_core::workspace::same_folder;

use super::KynexApp;
use super::keys::ToggleGit;
use super::state::Active;
use crate::git_pane::{GitPane, GitPaneEvent};

impl KynexApp {
    /// The toolbar's Git button, its key and the palette: the Git pane closes if it is in this
    /// tab, comes up if it is in another, else opens where it was last. In chat, the Git modal
    /// on the session's folder opens or closes.
    pub(super) fn toggle_git(&mut self, _: &ToggleGit, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, ws, _)) = self.tiling_scope() else {
            if self.git_modal.is_some() {
                return self.close_git_modal(cx);
            }
            if let Some(cwd) = self.focused_key().and_then(|k| self.session_by_key(k)).map(|s| s.cwd.clone()) {
                self.open_git_modal(cwd, cx);
            }
            return;
        };
        let Some(t) = self.tiles.get(&ws) else { return };
        let here = t.tab_panes_of(Content::Git).first().copied();
        match (here, t.panes_of(Content::Git).first().copied()) {
            (Some(p), _) => self.close_tool_pane(p, window, cx),
            (None, Some(p)) => {
                self.focus_pane(p, window, cx);
                self.focus_input(window, cx);
            }
            (None, None) => self.open_git_pane(window, cx),
        }
        cx.notify();
    }

    /// Whether the Git pane is in the tab shown.
    pub(super) fn git_pane_shown(&self) -> bool {
        self.tiling_scope()
            .and_then(|(_, ws, _)| self.tiles.get(&ws))
            .is_some_and(|t| !t.tab_panes_of(Content::Git).is_empty())
    }

    /// Opens the Git pane in the tab shown, where it was when it was closed last (see
    /// `Tiling::open_git`), else on the right of the focused session.
    fn open_git_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, ws, _)) = self.tiling_scope() else { return };
        self.git_pane(&ws, window, cx).update(cx, |g, cx| g.refresh(cx));
        let (id, bounds) = (self.next_pane, self.layout_bounds(cx));
        let Some(t) = self.tiles.get_mut(&ws) else { return };
        if !t.open_git(id, bounds) {
            self.toast(Toast::error("No room for the Git pane here: the panes would get too small.".to_string()));
            return cx.notify();
        }
        self.next_pane += 1;
        self.show_focused_pane(window, cx);
        self.focus_input(window, cx);
    }

    /// The workspace's changes on the right of pane `target`, sharing its room (a pane's
    /// "Changes beside"), or the Git pane focused if it is open.
    pub(super) fn show_git_pane(&mut self, target: PaneId, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((_, ws, _)) = self.tiling_scope() {
            let git = self.git_pane(&ws, window, cx);
            git.update(cx, |g, cx| g.refresh(cx));
        }
        self.open_tool(target, Tool::Git, 0.5, window, cx);
    }

    /// The Git pane, opened or focused, on worktree `path`'s changes (the branch chip).
    /// Outside a project's panes (chat) the Git modal shows them instead.
    pub(super) fn show_git_worktree(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, ws, _)) = self.tiling_scope() else { return self.open_git_modal(path, cx) };
        match self.tool_pane(Content::Git) {
            Some(p) => {
                self.focus_pane(p, window, cx);
                self.focus_input(window, cx);
            }
            None => self.open_git_pane(window, cx),
        }
        self.git_pane(&ws, window, cx).update(cx, |g, cx| g.select(path, cx));
    }

    /// Closes the workspace's Git pane, wherever it is.
    fn close_git_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(p) = self.tool_pane(Content::Git) {
            self.close_tool_pane(p, window, cx);
        }
    }

    /// The Git pane for workspace folder `ws`, made the first time it's needed.
    pub(super) fn git_pane(&mut self, ws: &Path, window: &mut Window, cx: &mut Context<Self>) -> Entity<GitPane> {
        if let Some(p) = self.git_panes.get(ws) {
            return p.clone();
        }
        let project = self.workspace_project().unwrap_or_else(|| ws.to_path_buf());
        let worktrees = self.worktrees(&project, cx);
        let tab = self.tiles.get(ws).map(|t| t.git_tab()).unwrap_or_default();
        let pane = cx.new(|cx| {
            let mut pane = GitPane::new(ws.to_path_buf(), worktrees, cx);
            pane.set_tab(tab, cx);
            pane
        });
        cx.subscribe_in(&pane, window, |this, _, ev: &GitPaneEvent, window, cx| match ev {
            GitPaneEvent::OpenFile { root, path, untracked, staged } => {
                this.open_git_file_diff(root.clone(), path.clone(), *untracked, *staged, cx)
            }
            GitPaneEvent::OpenDetail(view) => {
                let title = view.read(cx).detail_title();
                this.open_git_detail(view.clone(), title, cx)
            }
            GitPaneEvent::OpenFolder(folder) => this.open_folder_with_git(folder.clone(), cx),
            GitPaneEvent::Close => this.close_git_pane(window, cx),
            GitPaneEvent::Tab(tab) => {
                if let Some((_, ws, _)) = this.tiling_scope()
                    && let Some(t) = this.tiles.get_mut(&ws)
                {
                    t.set_git_tab(*tab);
                }
            }
            // A Git view of that folder (chat's modal) reads it again.
            GitPaneEvent::Changed(root) => {
                for (folder, view) in &this.git {
                    if same_folder(folder, root) {
                        view.update(cx, |v, cx| v.refresh(cx));
                    }
                }
            }
        })
        .detach();
        self.git_panes.insert(ws.to_path_buf(), pane.clone());
        pane
    }

    /// Opens `folder` (a worktree) as one of its project's workspaces, or as a project tab of
    /// its own, with the Git pane open there.
    pub(super) fn open_folder_with_git(&mut self, folder: PathBuf, cx: &mut Context<Self>) {
        self.git_modal = None;
        // A worktree of an open project is one of its workspaces, not a tab.
        if !self.open_as_workspace(&folder, cx) {
            self.add_project(folder.clone());
            // A worktree is another view of a project you have: no setup checklist for it.
            self.setup_for = None;
        }
        self.want_git_pane = Some((folder, std::time::Instant::now()));
        self.save();
        cx.notify();
    }

    /// Each frame, in a project's panes: the Git pane opens where it was asked for (a folder
    /// opened from Git, a project left on the old full-window Git view) once its panes exist.
    pub(super) fn sync_git_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, ws, _)) = self.tiling_scope() else { return };
        if !self.tiles.contains_key(&ws) {
            return;
        }
        // Asked for a folder that didn't come up (soon): it isn't wanted any more.
        let wanted = self.want_git_pane.as_ref().map(|(w, at)| (same_folder(w, &ws), at.elapsed().as_secs() > 10));
        let mut wanted = match wanted {
            Some((here, stale)) if here || stale => {
                self.want_git_pane = None;
                here && !stale
            }
            _ => false,
        };
        if let Active::Project(i) = self.saved.active
            && let Some(p) = self.saved.projects.get_mut(i)
            && p.was_on_git()
        {
            p.left_git();
            self.save();
            wanted = true;
        }
        if wanted && self.tool_pane(Content::Git).is_none() {
            self.open_git_pane(window, cx);
        }
    }

    /// A file link in the transcript (or the scratch files): its diff. In a project's panes,
    /// the file's whole diff in a modal; in chat, the Git modal on it.
    pub(super) fn open_file_changes(&mut self, cwd: PathBuf, file: PathBuf, cx: &mut Context<Self>) {
        if self.tiling_scope().is_none() {
            return self.open_git_file(cwd, file, cx);
        }
        cx.spawn(async move |this, cx| {
            let at = cwd.clone();
            let status = cx.background_spawn(async move { git::status(&at) }).await;
            this.update(cx, |this, cx| {
                let found = status.as_ref().ok().and_then(|s| git::find_file(s, &file)).cloned();
                match found {
                    Some(f) => {
                        let untracked = f.unstaged == Some(Change::Untracked);
                        let staged = match (f.staged, f.unstaged) {
                            (Some(_), Some(_)) => Staged::Partly,
                            (Some(_), None) => Staged::All,
                            _ => Staged::No,
                        };
                        this.open_git_file_diff(cwd.clone(), f.path, untracked, staged, cx);
                    }
                    None => {
                        let name =
                            file.file_name().map_or_else(|| file.display().to_string(), |n| n.to_string_lossy().into());
                        this.toast(Toast::info(format!("{name} has no changes")));
                        cx.notify();
                    }
                }
            })
            .ok();
        })
        .detach();
    }
}
