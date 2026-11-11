//! Go To: showing a session wherever it is (its project tab, workspace and pane), for the
//! palette's sessions and for the key that jumps to the next session that needs you.

use std::path::{Path, PathBuf};

use gpui::{Context, Window};
use kynex_core::attention::next_waiting;
use kynex_core::live::Key;
use kynex_core::palette::workspace_places;
use kynex_core::session::Kind;
use kynex_core::toasts::Toast;
use kynex_core::workspace;

use super::KynexApp;
use super::keys;
use super::state::Active;

/// How to tell which session of a project is meant.
#[derive(Clone, Copy)]
pub(super) enum Which<'a> {
    /// By the CLI's session id: open already, or to be resumed.
    Id(&'a str),
    /// An open session, by key.
    Open(Key),
}

impl KynexApp {
    /// Shows session `which` of `project`, which runs in `workspace`: selects the project's tab
    /// and the workspace, then focuses the pane showing it, or shows it in the focused pane
    /// (resuming it if it isn't open).
    pub(super) fn reveal_session(
        &mut self,
        project: &Path,
        workspace: &Path,
        which: Which<'_>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(i) = self.saved.projects.iter().position(|p| p.path == project) else { return };
        self.select(Active::Project(i), window, cx);
        self.show_code_view();
        let here = self.workspace_cwd().is_some_and(|c| workspace::same_folder(&c, workspace));
        if !here {
            self.select_workspace(workspace, cx);
        }
        let open = self.code.get(project).and_then(|l| {
            l.sessions
                .iter()
                .find(|s| match which {
                    Which::Id(id) => s.session_id.as_deref() == Some(id),
                    Which::Open(key) => s.uid == key,
                })
                .map(|s| s.uid)
        });
        match (open, which) {
            (Some(key), _) => {
                // Shown first: the workspace's tiling is the one of the shown session's folder.
                self.show_open(|s| s.uid == key);
                if let Some((_, cwd, _)) = self.tiling_scope()
                    && let Some(tiles) = self.tiles.get_mut(&cwd)
                    && let Some(pane) = tiles.pane_of(key)
                {
                    // Its pane may be in another pane tab: this brings that tab up.
                    tiles.focus_pane(pane);
                    self.focus_pane(pane, window, cx);
                }
            }
            (None, Which::Id(id)) => self.switch_session(Kind::Code, workspace.to_path_buf(), id.to_string()),
            (None, Which::Open(_)) => {}
        }
        self.focus_input(window, cx);
        cx.notify();
    }

    /// Every open session in a stable order (project, workspace, then as opened): its project,
    /// folder, key, and whether it needs you (a prompt waits).
    fn sessions_in_order(&mut self, cx: &mut Context<Self>) -> Vec<(PathBuf, PathBuf, Key, bool)> {
        let projects: Vec<PathBuf> = self.saved.projects.iter().map(|p| p.path.clone()).collect();
        let mut rows = Vec::new();
        for (pi, project) in projects.iter().enumerate() {
            if !self.code.contains_key(project) {
                continue;
            }
            let list = self.workspaces_of(project, cx);
            let places = workspace_places(&list.iter().map(|(c, _)| (c.main, c.label.as_str())).collect::<Vec<_>>());
            let Some(live) = self.code.get(project) else { continue };
            for (pos, s) in live.sessions.iter().enumerate() {
                let place = list.iter().position(|(c, _)| c.at(&s.cwd)).map_or(usize::MAX, |i| places[i]);
                rows.push(((pi, place, pos), (project.clone(), s.cwd.clone(), s.uid, s.waiting())));
            }
        }
        rows.sort_by_key(|(at, _)| *at);
        rows.into_iter().map(|(_, r)| r).collect()
    }

    /// Shows the next session, in any project, that needs you (after the focused one, wrapping).
    pub(super) fn next_needs_you(&mut self, _: &keys::NextNeedsYou, window: &mut Window, cx: &mut Context<Self>) {
        self.palette = None;
        let rows = self.sessions_in_order(cx);
        let order: Vec<(Key, bool)> = rows.iter().map(|r| (r.2, r.3)).collect();
        let current = self.focused_key().filter(|_| matches!(self.saved.active, Active::Project(_)));
        let Some(key) = next_waiting(&order, current) else {
            self.toast(Toast::info("Nothing needs you."));
            return cx.notify();
        };
        if let Some((project, cwd, _, _)) = rows.into_iter().find(|r| r.2 == key) {
            self.reveal_session(&project, &cwd, Which::Open(key), window, cx);
        }
    }
}
