//! The control socket's session commands: open a new `agent_cli` pane (launching a chosen
//! provider or showing the provider picker), resume a past session by CLI id, or rename /
//! archive / pin a session in its workspace's groups.

use std::path::{Path, PathBuf};

use gpui::{Context, Window};
use kynex_core::control::protocol::Place;
use kynex_core::layout::{Direction, PaneId};
use kynex_core::tiling::AgentCliPane;
use kynex_core::workspace::same_folder;
use serde_json::{Value, json};

use super::KynexApp;

type Answer = Result<Value, String>;

/// What `session new` asks for.
pub(super) struct NewSession {
    pub workspace: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub place: Place,
}

impl KynexApp {
    /// The project a request is about: the calling pane's project, else the shown one.
    pub(super) fn request_project(&self, caller: Option<PaneId>) -> Result<PathBuf, String> {
        caller
            .and_then(|p| self.agent_pane_location(p).and_then(|(ws, _)| self.tile_projects.get(&ws).cloned()))
            .or_else(|| self.workspace_project())
            .ok_or_else(|| "no project is shown".to_string())
    }

    /// `session new`: opens a new `agent_cli` pane in the target workspace. If `provider` is
    /// given, launches that provider CLI immediately; otherwise opens the inline provider picker.
    pub(super) fn control_session_new(
        &mut self,
        new: NewSession,
        caller: Option<PaneId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Answer {
        let project = self.request_project(caller)?;
        let folder = match &new.workspace {
            Some(w) => {
                let wanted = PathBuf::from(w);
                let found = self.workspaces_of(&project, cx).into_iter().find(|(e, _)| e.at(&wanted));
                found.map(|(e, _)| e.path).ok_or_else(|| format!("{w} isn't a workspace of {}", project.display()))?
            }
            None => caller
                .and_then(|p| self.agent_pane_location(p).map(|(ws, _)| ws))
                .or_else(|| self.tiling_scope().filter(|(p, _)| same_folder(p, &project)).map(|(_, ws)| ws))
                .unwrap_or_else(|| project.clone()),
        };
        let pane = self.place_agent_pane(&project, &folder, AgentCliPane::pending(), new.place, caller)?;
        if let Some(provider) = new.provider {
            self.launch_agent_cli(pane, folder.clone(), provider.clone(), None, new.model, window, cx);
            Ok(json!({ "pane": pane, "workspace": folder, "provider": provider }))
        } else {
            self.agent_pickers.insert(pane, Default::default());
            self.save_layouts(cx);
            cx.notify();
            Ok(json!({ "pane": pane, "workspace": folder, "provider": null }))
        }
    }

    /// Places a new `agent_cli` pane in `folder` beside `caller` (or the focused pane).
    fn place_agent_pane(
        &mut self,
        project: &Path,
        folder: &Path,
        meta: AgentCliPane,
        place: Place,
        caller: Option<PaneId>,
    ) -> Result<PaneId, String> {
        let pane = self.next_pane;
        let bounds = self.tile_bounds();
        // Ensure a tiling layout exists for `folder`.
        if !self.tiles.keys().any(|ws| same_folder(ws, folder)) {
            let p = self.next_pane;
            self.next_pane += 1;
            let t = kynex_core::tiling::Tiling::new(p, meta);
            self.tiles.insert(folder.to_path_buf(), t);
            self.tile_projects.insert(folder.to_path_buf(), project.to_path_buf());
            return Ok(p);
        }
        let t = self
            .tiles
            .iter_mut()
            .find(|(ws, _)| same_folder(ws, folder))
            .map(|(_, t)| t)
            .ok_or_else(|| format!("{} isn't open", folder.display()))?;
        let before = t.focus();
        let target = caller.filter(|p| t.has(*p)).or(before).ok_or("no pane to split from")?;
        let placed = match place {
            Place::Right => t.split_agent_at(target, Direction::Right, pane, meta, bounds),
            Place::Down => t.split_agent_at(target, Direction::Down, pane, meta, bounds),
        };
        if !placed {
            return Err("no room for another pane in that workspace".into());
        }
        if let Some(b) = before {
            t.focus_pane(b);
        }
        self.next_pane += 1;
        Ok(pane)
    }

    /// `session open`: resumes a past session by its CLI id (or swaps/focuses if already live).
    pub(super) fn control_session_open(
        &mut self,
        id: String,
        place: Place,
        caller: Option<PaneId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Answer {
        if let Some(inst) = self.agent_instances.values().find(|a| a.meta.session_id.as_deref() == Some(id.as_str())) {
            let agent_id = inst.id;
            let ws = inst.cwd.clone();
            if let Some(pane) = self.pane_showing_agent(agent_id) {
                return Ok(
                    json!({ "pane": pane, "instance": agent_id, "workspace": ws, "session_id": id, "already_open": true }),
                );
            }
            let project = self.tile_projects.get(&ws).cloned().unwrap_or_else(|| ws.clone());
            let pane = self.place_agent_pane(&project, &ws, AgentCliPane::pending(), place, caller)?;
            self.mount_agent_in_pane(pane, &ws, agent_id, window, cx);
            return Ok(
                json!({ "pane": pane, "instance": agent_id, "workspace": ws, "session_id": id, "already_open": true }),
            );
        }
        let project = self.request_project(caller)?;
        let cwd = self
            .folder_of_listed(&id)
            .filter(|f| self.workspaces_of(&project, cx).iter().any(|(w, _)| w.at(f)))
            .ok_or_else(|| format!("no past session {id} in {}'s workspaces", project.display()))?;
        self.edit_groups(&cwd, |g| {
            g.unarchive(&id);
        });
        let provider = self
            .sessions
            .get(&cwd)
            .and_then(|l| l.items.iter().find(|i| i.id == id))
            .map(|i| i.provider.clone())
            .unwrap_or_else(|| self.groups_for(&cwd).provider_of(&id));
        let model = self.groups_for(&cwd).model_of(&id).map(str::to_string);
        let pane = self.place_agent_pane(&project, &cwd, AgentCliPane::pending(), place, caller)?;
        self.launch_agent_cli(pane, cwd.clone(), provider.clone(), Some(id.clone()), model, window, cx);
        Ok(json!({ "pane": pane, "workspace": cwd, "session_id": id, "provider": provider }))
    }

    fn session_folder(&self, id: &str) -> Result<PathBuf, String> {
        for (ws, t) in &self.tiles {
            if t.pane_of_session(id).is_some() {
                return Ok(ws.clone());
            }
        }
        self.folder_of_listed(id).ok_or_else(|| format!("no session {id} found"))
    }

    pub(super) fn control_session_rename(&mut self, id: &str, title: &str) -> Answer {
        let cwd = self.session_folder(id)?;
        self.edit_groups(&cwd, |g| g.rename(id, title));
        Ok(json!({ "renamed": id, "title": title.trim() }))
    }

    pub(super) fn control_session_archive(&mut self, id: &str) -> Answer {
        let cwd = self.session_folder(id)?;
        self.edit_groups(&cwd, |g| {
            g.archive(id);
        });
        Ok(json!({ "archived": id }))
    }

    pub(super) fn control_session_pin(&mut self, id: &str, off: bool) -> Answer {
        let cwd = self.session_folder(id)?;
        self.edit_groups(&cwd, |g| {
            if g.is_pinned(id) == off {
                g.toggle_pin(id);
            }
        });
        Ok(json!({ "session": id, "pinned": !off }))
    }
}
