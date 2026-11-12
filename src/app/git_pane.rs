//! The workspace's Git pane, from the app's side: making it, opening it where it was last
//! (the toolbar's button, its key, the palette, the branch chip), closing it, and what it
//! asks for (a file's diff in a modal). One per workspace; asking for another focuses it.
//! Chat has no panes: there the Git modal shows the session's folder instead.

use std::path::{Path, PathBuf};

use gpui::{AppContext, Context, Entity, Window};
use kynex_core::layout::PaneId;
use kynex_core::tiling::{Content, Tool};
use kynex_core::toasts::Toast;

use super::KynexApp;
use super::keys::ToggleGit;
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
        let pane = cx.new(|cx| GitPane::new(ws.to_path_buf(), worktrees, cx));
        cx.subscribe_in(&pane, window, |this, _, ev: &GitPaneEvent, window, cx| match ev {
            GitPaneEvent::OpenFile { root, path, untracked } => {
                this.open_git_file_diff(root.clone(), path.clone(), *untracked, cx)
            }
            GitPaneEvent::OpenFull(path) => this.open_git_modal(path.clone(), cx),
            GitPaneEvent::Close => this.close_git_pane(window, cx),
        })
        .detach();
        self.git_panes.insert(ws.to_path_buf(), pane.clone());
        pane
    }
}
