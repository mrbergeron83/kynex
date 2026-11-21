//! Saving each workspace's panes across restarts (`workspaces.json`) and bringing them back.

use std::path::{Path, PathBuf};

use gpui::Context;
use kynex_core::tiling::{SavedLayout, Tiling};
use kynex_core::workspace::same_folder;

use super::KynexApp;

impl KynexApp {
    pub(super) fn save_layouts(&mut self, cx: &mut Context<Self>) {
        for project in self.update_layouts() {
            self.save_workspaces(&project, cx);
        }
    }

    pub(super) fn save_layouts_now(&mut self) {
        for project in self.update_layouts() {
            if let Some(w) = self.workspaces.get(&project)
                && let Err(e) = w.save(&project)
            {
                eprintln!("{e}");
            }
        }
    }

    fn update_layouts(&mut self) -> Vec<PathBuf> {
        if self.tile_drag.is_some() {
            return Vec::new();
        }
        let mut layouts: Vec<(PathBuf, PathBuf, Option<SavedLayout>)> = Vec::new();
        for (cwd, tiling) in &self.tiles {
            let Some(project) = self.tile_projects.get(cwd).cloned() else { continue };
            if !self.saved.projects.iter().any(|p| same_folder(&p.path, &project)) {
                continue;
            }
            let saved = tiling.save();
            let worth = saved.agents.values().any(|a| a.provider.is_some())
                || saved.tools.as_ref().is_some_and(|t| !t.is_empty());
            layouts.push((project, cwd.clone(), worth.then_some(saved)));
        }
        let mut changed: Vec<PathBuf> = Vec::new();
        let now = super::layout_history::now_secs();
        for (project, cwd, saved) in layouts {
            let ws = self.saved_workspaces(&project);
            let kept = match &saved {
                Some(l) => ws.history(&cwd).record(now, l),
                None => {
                    ws.history(&cwd).pause();
                    false
                }
            };
            if (ws.set_layout(&cwd, saved) || kept) && !changed.contains(&project) {
                changed.push(project);
            }
        }
        changed
    }

    pub(super) fn keep_shape(&mut self, cwd: &Path) {
        let prev = self.tiled_last.replace(cwd.to_path_buf());
        let Some(prev) = prev.filter(|p| !same_folder(p, cwd)) else { return };
        let same = match (self.tiles.get(&prev), self.tiles.get(cwd)) {
            (Some(from), Some(to)) => to.same_shape(from),
            _ => return,
        };
        if same {
            return;
        }
        let (Some(from), Some(mut to)) = (self.tiles.remove(&prev), self.tiles.remove(cwd)) else { return };
        let mut next = self.next_pane;
        to.adopt_shape(&from, &mut next);
        self.next_pane = next;
        self.tiles.insert(prev, from);
        self.tiles.insert(cwd.to_path_buf(), to);
    }

    pub(super) fn drop_tiles_of(&mut self, cwd: &Path, cx: &Context<Self>) {
        self.tiles.retain(|f, _| !same_folder(f, cwd));
        self.tile_projects.retain(|f, _| !same_folder(f, cwd));
        self.drop_agents_in(cwd, cx);
    }

    pub(super) fn drop_project_tiles(&mut self, project: &Path, cx: &Context<Self>) {
        let gone: Vec<PathBuf> =
            self.tile_projects.iter().filter(|(_, p)| same_folder(p, project)).map(|(c, _)| c.clone()).collect();
        for cwd in gone {
            self.drop_tiles_of(&cwd, cx);
        }
    }

    pub(super) fn restore_tiling(&mut self, project: &Path, cwd: &Path) -> Option<Tiling> {
        let saved = self.saved_workspaces(project).layout(cwd).cloned()?;
        let groups = self.groups_for(cwd).clone();
        let mut next = self.next_pane;
        let tiling = Tiling::restore(&saved, &mut next, |id| groups.provider_of(id));
        self.next_pane = next;
        tiling
    }
}
