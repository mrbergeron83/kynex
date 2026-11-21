//! Go To: jumping to a session or `agent_cli` pane across open projects and workspaces,
//! for the command palette and the "Next agent that needs you" shortcut.

use std::path::Path;

use gpui::{Context, Window};
use kynex_core::agent_panes::AgentState;
use kynex_core::attention::next_waiting;
use kynex_core::palette::workspace_places;
use kynex_core::toasts::Toast;
use kynex_core::workspace::same_folder;

use super::KynexApp;
use super::keys;

impl KynexApp {
    /// Reveals a session by CLI `session_id` in `project` / `workspace`: switches to the
    /// project tab and workspace, then focuses or resumes the session in an `agent_cli` pane.
    pub(super) fn reveal_session(
        &mut self,
        project: &Path,
        workspace: &Path,
        session_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(i) = self.saved.projects.iter().position(|p| same_folder(&p.path, project)) else { return };
        self.select_project(i, window, cx);
        let here = self.workspace_cwd().is_some_and(|c| same_folder(&c, workspace));
        if !here {
            self.select_workspace(workspace, cx);
        }
        self.switch_session(workspace.to_path_buf(), session_id.to_string(), window, cx);
        self.focus_input(window, cx);
        cx.notify();
    }

    /// Collects all live `agent_cli` instances (visible and parked) across open projects
    /// in a stable order: `(agent_id, needs_you)`.
    fn agents_in_order(&mut self, cx: &mut Context<Self>) -> Vec<(super::agent_cli::AgentId, bool)> {
        let projects: Vec<_> = self.saved.projects.iter().map(|p| p.path.clone()).collect();
        let mut rows = Vec::new();
        for (pi, project) in projects.iter().enumerate() {
            let list = self.workspaces_of(project, cx);
            let places = workspace_places(&list.iter().map(|(c, _)| (c.main, c.label.as_str())).collect::<Vec<_>>());
            for inst in self.agent_instances.values() {
                let belongs = same_folder(&inst.cwd, project)
                    || self.tile_projects.get(&inst.cwd).is_some_and(|p| same_folder(p, project));
                if !belongs {
                    continue;
                }
                let place = list.iter().position(|(c, _)| c.at(&inst.cwd)).map_or(usize::MAX, |i| places[i]);
                let needs = inst.tracker.state() == AgentState::NeedsYou;
                rows.push(((pi, place, inst.id), (inst.id, needs)));
            }
        }
        rows.sort_by_key(|(at, _)| *at);
        rows.into_iter().map(|(_, r)| r).collect()
    }

    /// Jumps to the next live `agent_cli` instance (visible or parked), in any project,
    /// that needs your input.
    pub(super) fn next_needs_you(&mut self, _: &keys::NextNeedsYou, window: &mut Window, cx: &mut Context<Self>) {
        self.palette = None;
        let order = self.agents_in_order(cx);
        let current = self.focused_agent_id();
        let Some(agent_id) = next_waiting(&order, current) else {
            self.toast(Toast::info("Nothing needs you."));
            return cx.notify();
        };
        self.go_to_agent(agent_id, window, cx);
    }
}
