//! Keeping each workspace's panes across restarts: the layout and the session id in every
//! pane are saved with the project's workspaces (`workspaces.json`), and the first time a
//! workspace is shown in a run its panes come back. The sessions are only reopened (their
//! transcripts read); a `claude` process starts when its pane is focused or used.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use gpui::Context;
use kynex_core::live::Key;
use kynex_core::session::{Kind, Session};
use kynex_core::tiling::{SavedLayout, Tiling};
use kynex_core::workspace::same_folder;

use super::KynexApp;

impl KynexApp {
    /// Saves every workspace's panes that changed, to its project's `workspaces.json`.
    /// Cheap enough for every poll; it writes only on a change, and waits for a divider
    /// drag to end.
    pub(super) fn save_layouts(&mut self, cx: &mut Context<Self>) {
        for project in self.update_layouts() {
            self.save_workspaces(&project, cx);
        }
    }

    /// [`Self::save_layouts`] written at once, for quitting. A failure is only logged.
    pub(super) fn save_layouts_now(&mut self) {
        for project in self.update_layouts() {
            if let Some(w) = self.workspaces.get(&project)
                && let Err(e) = w.save(&project)
            {
                eprintln!("{e}");
            }
        }
    }

    /// Puts the current layouts into the projects' workspaces; the projects that changed.
    fn update_layouts(&mut self) -> Vec<PathBuf> {
        if self.tile_drag.is_some() {
            return Vec::new();
        }
        let mut layouts: Vec<(PathBuf, PathBuf, Option<SavedLayout>)> = Vec::new();
        for (cwd, tiling) in &self.tiles {
            let Some(project) = self.tile_projects.get(cwd).cloned() else { continue };
            // A project that's closed, or whose sessions aren't open, has nothing to say:
            // what was saved stays.
            if !self.saved.projects.iter().any(|p| same_folder(&p.path, &project)) {
                continue;
            }
            let Some(live) = self.code.get(&project) else { continue };
            if tiling.sessions().iter().any(|k| !live.sessions.iter().any(|s| s.uid == *k)) {
                continue;
            }
            let ids: HashMap<Key, String> = live
                .sessions
                .iter()
                .filter(|s| !s.is_new())
                .filter_map(|s| Some((s.uid, s.session_id.clone()?)))
                .collect();
            let saved = tiling.save(|key| ids.get(&key).cloned());
            // Panes of sessions nobody has talked to say nothing worth bringing back; terminals do.
            let worth = !saved.sessions.is_empty() || saved.tools.as_ref().is_some_and(|t| !t.is_empty());
            layouts.push((project, cwd.clone(), worth.then_some(saved)));
        }
        let mut changed: Vec<PathBuf> = Vec::new();
        for (project, cwd, saved) in layouts {
            if self.saved_workspaces(&project).set_layout(&cwd, saved) && !changed.contains(&project) {
                changed.push(project);
            }
        }
        changed
    }

    /// Stops tracking the panes of workspace `cwd` (removed), so its layout isn't saved again.
    pub(super) fn drop_tiles_of(&mut self, cwd: &Path) {
        self.tiles.retain(|f, _| !same_folder(f, cwd));
        self.tile_projects.retain(|f, _| !same_folder(f, cwd));
    }

    /// Stops tracking the panes of every workspace of a closed `project`; what was saved stays.
    pub(super) fn drop_project_tiles(&mut self, project: &Path) {
        let gone: Vec<PathBuf> =
            self.tile_projects.iter().filter(|(_, p)| same_folder(p, project)).map(|(c, _)| c.clone()).collect();
        for cwd in gone {
            self.drop_tiles_of(&cwd);
        }
    }

    /// After a restore, the window talks to the session in the restored focused pane, unless
    /// the session it had is in a pane too. A new session left in no pane is closed.
    pub(super) fn keep_restored_focus(&mut self, project: &Path, cwd: &Path, was: Key) {
        let Some(t) = self.tiles.get(cwd) else { return };
        if t.pane_of(was).is_some() {
            return;
        }
        let Some(key) = t.focus().and_then(|p| t.key_of(p)) else { return };
        if let Some(live) = self.code.get_mut(project) {
            live.focus(key);
        }
        if self.session_by_key(was).is_some_and(Session::is_new) {
            self.close_open(was);
        }
    }

    /// The panes `cwd` had last time, if saved: reopened sessions (the ones already open are
    /// reused) in a layout with fresh pane ids. `current` is the session the window talks to.
    /// The focused pane takes the id `keep`, so the pane state already up carries over.
    pub(super) fn restore_tiling(
        &mut self,
        project: &Path,
        cwd: &Path,
        current: Key,
        keep: Option<kynex_core::layout::PaneId>,
    ) -> Option<Tiling> {
        let saved = self.saved_workspaces(project).layout(cwd).cloned()?;
        // A new session already open (the one a project starts on) can fill a pane that was
        // a new session too, rather than leaving a second one behind.
        let mut spare = self
            .code
            .get(project)
            .and_then(|l| l.sessions.iter().find(|s| s.uid == current))
            .filter(|s| s.is_new())
            .map(|s| s.uid);
        let mut next = self.next_pane;
        let tiling = Tiling::restore(&saved, &mut next, keep, |id| match id {
            Some(id) => self.saved_session(project, cwd, id),
            None => spare.take().or_else(|| self.fresh_session(project, cwd)),
        });
        self.next_pane = next;
        tiling
    }

    /// The open session `id`, else it reopened if its log is still there.
    fn saved_session(&mut self, project: &Path, cwd: &Path, id: &str) -> Option<Key> {
        let open = self
            .code
            .get(project)
            .and_then(|l| l.sessions.iter().find(|s| s.session_id.as_deref() == Some(id) && same_folder(&s.cwd, cwd)));
        if let Some(s) = open {
            return Some(s.uid);
        }
        if !kynex_core::session::session_exists(cwd, id) {
            return None;
        }
        let mut s = Session::resume(Kind::Code, cwd.to_path_buf(), id.to_string(), self.wake.clone());
        self.adopt_project_settings(&mut s);
        self.start_model(&mut s);
        s.wake = self.wake.clone();
        Some(self.keep_open(project, s))
    }

    fn fresh_session(&mut self, project: &Path, cwd: &Path) -> Option<Key> {
        let s = self.pane_session(Kind::Code, cwd.to_path_buf());
        Some(self.keep_open(project, s))
    }

    /// Opens `s` in the project, off screen until a pane shows it.
    fn keep_open(&mut self, project: &Path, s: Session) -> Key {
        let key = s.uid;
        self.code.entry(project.to_path_buf()).or_default().sessions.push(s);
        key
    }
}
