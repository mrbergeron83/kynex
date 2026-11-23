//! Project tabs and session opening/focusing in the pure-CLI workspace.

use std::path::PathBuf;

use gpui::{Context, Focusable, PathPromptOptions, Window};
use kynex_core::layout::{Axis, PaneId};
use kynex_core::tiling::{AgentCliPane, Content};
use kynex_core::workspace::same_folder;

use super::KynexApp;
use super::state::ProjectState;

impl KynexApp {
    pub(super) fn active_project(&self) -> Option<&ProjectState> {
        self.saved.active_project.and_then(|i| self.saved.projects.get(i))
    }

    /// The currently focused pane id in the active workspace.
    pub(super) fn focused_pane_id(&self) -> Option<PaneId> {
        let (_, cwd) = self.tiling_scope()?;
        self.tiles.get(&cwd)?.focus()
    }

    /// The live instance id currently shown in the focused/main agent pane, if any.
    pub(super) fn focused_agent_id(&self) -> Option<super::agent_cli::AgentId> {
        let (_, cwd) = self.tiling_scope()?;
        let t = self.tiles.get(&cwd)?;
        let pane = t.agent_focus()?;
        self.pane_agents.get(&pane).or_else(|| self.pane_claude.get(&pane)).copied()
    }

    /// The session id of the currently focused agent pane, if any.
    pub(super) fn focused_session_id(&self) -> Option<String> {
        let (_, cwd) = self.tiling_scope()?;
        let t = self.tiles.get(&cwd)?;
        let pane = t.agent_focus()?;
        self.pane_agent(pane)
            .and_then(|a| a.meta.session_id.clone())
            .or_else(|| self.pane_claude_chat(pane).and_then(|c| c.meta.session_id.clone()))
            .or_else(|| t.agent(pane)?.session_id.clone())
    }

    /// Replaces the current/main `agent_cli` pane with the inline provider picker so you can
    /// start a new session without cluttering the workspace with extra panes. Any running CLI
    /// in that pane stays alive in the background and can be switched back to from the Sessions list.
    pub(super) fn new_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, cwd)) = self.tiling_scope() else { return };
        let Some(target) = self.tiles.get(&cwd).and_then(|t| t.agent_focus()) else {
            return self.split_pane(Axis::Row, window, cx);
        };
        if let Some(t) = self.tiles.get_mut(&cwd) {
            t.focus_pane(target);
        }
        if self.tiles.get(&cwd).and_then(|t| t.agent(target)).is_some_and(AgentCliPane::is_pending) {
            self.picker_focus.focus(window, cx);
            cx.notify();
            return;
        }
        self.show_picker_in_pane(target, window, cx);
    }

    /// Resumes or switches to session `id` in workspace `cwd` inside the current/main
    /// `agent_cli` pane. If a live instance is already running for `id` (visible or parked in
    /// the background), focuses or swaps it in without restarting; otherwise launches a resume
    /// CLI while parking whatever was previously in the pane.
    pub(super) fn switch_session(&mut self, cwd: PathBuf, id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.edit_groups(&cwd, |g| {
            g.unarchive(&id);
        });
        if let Some(project) = self.workspace_project() {
            self.saved_workspaces(&project).remember(&cwd, Some(&id));
        }
        // Already running in this workspace (visible in a pane OR parked in background)?
        if let Some(agent_id) = self.live_agent_of_session(&cwd, &id) {
            self.go_to_agent(agent_id, window, cx);
            return;
        }
        let provider = self
            .sessions
            .get(&cwd)
            .and_then(|l| l.items.iter().find(|i| i.id == id))
            .map(|i| i.provider.clone())
            .unwrap_or_else(|| self.groups_for(&cwd).provider_of(&id));
        let model = self.groups_for(&cwd).model_of(&id).map(str::to_string);

        // Reuse the focused/main agent pane (or a pending pane if focused), parking whatever was there.
        let target = self.tiles.get(&cwd).and_then(|t| {
            t.agent_focus().or_else(|| t.agent_panes().into_iter().find(|(_, m)| m.is_pending()).map(|(p, _)| p))
        });
        let pane = match target {
            Some(p) => {
                if let Some(t) = self.tiles.get_mut(&cwd) {
                    t.focus_pane(p);
                }
                p
            }
            None => {
                let p = self.next_pane;
                let bounds = self.tile_bounds();
                let placed = self
                    .tiles
                    .get_mut(&cwd)
                    .is_some_and(|t| t.split_agent(Axis::Row, p, AgentCliPane::pending(), bounds));
                if !placed {
                    return;
                }
                self.next_pane += 1;
                p
            }
        };
        self.launch_agent(pane, cwd, provider, Some(id), model, window, cx);
    }

    /// Moves keyboard focus into the focused pane: `TerminalView` for running `agent_cli`
    /// panes, `ClaudeChatView` for Claude Chat panes, `picker_focus` for pending provider
    /// pickers, or the tool pane's focus handle.
    pub(super) fn focus_input(&self, window: &mut Window, cx: &mut Context<Self>) {
        if self.focus_tool_input(window, cx) {
            return;
        }
        let Some((_, ws)) = self.tiling_scope() else {
            self.focus.focus(window, cx);
            return;
        };
        let Some(t) = self.tiles.get(&ws) else { return };
        let Some(focus) = t.focus() else { return };
        match t.content(focus) {
            Some(Content::AgentCli) => {
                if let Some(view) = self.pane_agent(focus).and_then(|a| a.view.as_ref()) {
                    view.read(cx).focus_handle(cx).focus(window, cx);
                } else {
                    self.picker_focus.focus(window, cx);
                }
            }
            Some(Content::ClaudeChat) => {
                if let Some(chat) = self.pane_claude_chat(focus) {
                    chat.view.read(cx).focus_handle(cx).focus(window, cx);
                } else {
                    self.picker_focus.focus(window, cx);
                }
            }
            _ => {}
        }
    }

    pub(super) fn select_project(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.saved.projects.len() {
            return;
        }
        if self.saved.active_project != Some(index) {
            self.saved.active_project = Some(index);
            self.menu = None;
            self.save();
            self.sync_tiles(window, cx);
            self.focus_input(window, cx);
        }
        cx.notify();
    }

    pub(super) fn add_project(&mut self, path: PathBuf) {
        let idx = match self.saved.projects.iter().position(|p| same_folder(&p.path, &path)) {
            Some(i) => i,
            None => {
                self.saved.projects.push(ProjectState::new(path));
                self.saved.projects.len() - 1
            }
        };
        self.saved.active_project = Some(idx);
        self.save();
    }

    pub(super) fn close_project(&mut self, i: usize, cx: &mut Context<Self>) {
        if i >= self.saved.projects.len() {
            return;
        }
        let p = self.saved.projects.remove(i);
        let known: Vec<PathBuf> = self
            .worktrees
            .get(&p.path)
            .map(|w| w.read(cx).list.iter().map(|w| w.path.clone()).collect())
            .unwrap_or_default();
        self.drop_project_tiles(&p.path, cx);
        self.drop_gone_agents(cx);
        self.drop_gone_claude_chats(cx);
        self.drop_gone_terminals(cx);
        self.worktrees.remove(&p.path);
        self.drop_workspace_views(&p.path, &known);
        self.saved.active_project = match self.saved.active_project {
            Some(a) if a > i => Some(a - 1),
            Some(a) if a == i && i > 0 => Some(i - 1),
            Some(a) if a == i => (!self.saved.projects.is_empty()).then_some(0),
            other => other,
        };
        self.save();
    }

    pub(super) fn pick_project(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open project".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = paths.await
                && let Some(dir) = paths.into_iter().next()
            {
                this.update(cx, |this, cx| {
                    this.add_project(dir);
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }
}
