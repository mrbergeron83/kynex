//! Live interactive `agent_cli` instances and pane hosting:
//!
//! Each running CLI is a [`LiveAgent`] with its own stable [`AgentId`]. An `agent_cli` pane
//! displays either a [`LiveAgent`] (mapped via `pane_agents`) or the inline New Session
//! provider picker (`agent_picker.rs`). Switching sessions or clicking New Session replaces the
//! focused `agent_cli` pane while keeping previously running instances alive in the background
//! so you can cycle through them from the Sessions list without cluttering the layout.

use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui::{AnyElement, Context, Entity, Focusable, MouseButton, MouseDownEvent, Window, div, prelude::*};
use kynex_core::agent_cli::{self, LaunchRequest};
use kynex_core::agent_panes::{self, AgentState, Tracker};
use kynex_core::hooks;
use kynex_core::layout::{Axis, PaneId};
use kynex_core::tiling::AgentCliPane;
use kynex_core::workspace::same_folder;

use super::KynexApp;
use super::agent_picker::AgentPickerState;
use super::menus::Menu;
use crate::terminal::{TerminalEvent, TerminalView};
use crate::theme::{mono, theme};
use crate::ui::pane_drag::pane_handle;
use crate::ui::{Icon, Lamp, Tooltip, ghost_button, icon_button, lamp, outline_button, primary_button, px, spinner};

/// How often active `agent_cli` screens are scanned as a fallback / resume-id check.
const LOOK_EVERY: Duration = Duration::from_millis(500);

/// Stable identifier for one live CLI instance (allocated from `next_pane` so it never
/// collides with any `PaneId`).
pub(super) type AgentId = u64;

/// One live interactive CLI instance (either mounted in a pane or parked in the background).
pub(super) struct LiveAgent {
    pub id: AgentId,
    pub cwd: PathBuf,
    pub meta: AgentCliPane,
    pub view: Option<Entity<TerminalView>>,
    pub starting: bool,
    pub error: Option<String>,
    pub tracker: Tracker,
}

impl KynexApp {
    /// Allocates a fresh [`AgentId`] disjoint from all [`PaneId`]s.
    pub(super) fn alloc_agent_id(&mut self) -> AgentId {
        let id = self.next_pane;
        self.next_pane += 1;
        id
    }

    /// Reads built-in + user agent screen rules off the UI thread, then refreshes the provider catalog.
    pub(super) fn refresh_agent_rules(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let (rules, errors) = cx.background_spawn(async { agent_panes::rules(&agent_panes::user_dir()) }).await;
            this.update(cx, |this, cx| {
                for e in errors {
                    eprintln!("kynex: {e}");
                }
                if this.agent_rules != rules {
                    this.agent_rules = rules;
                    this.refresh_provider_catalog(cx);
                    this.rebuild_palette_if_open(cx);
                    cx.notify();
                } else if this.provider_catalog.is_empty() {
                    this.refresh_provider_catalog(cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Friendly display name for a provider id.
    pub(super) fn provider_display_name(&self, id: &str) -> String {
        agent_cli::label(id, &self.agent_rules)
    }

    /// Short badge label for a provider id.
    pub(super) fn provider_badge(&self, id: &str) -> String {
        agent_cli::badge(id, &self.agent_rules)
    }

    /// The [`LiveAgent`] currently mounted in `pane`, if any.
    pub(super) fn pane_agent(&self, pane: PaneId) -> Option<&LiveAgent> {
        let id = self.pane_agents.get(&pane)?;
        self.agent_instances.get(id)
    }

    /// The pane currently displaying `agent_id`, if it is mounted in any workspace.
    pub(super) fn pane_showing_agent(&self, agent_id: AgentId) -> Option<PaneId> {
        self.pane_agents.iter().chain(self.pane_claude.iter()).find_map(|(&pane, &id)| (id == agent_id).then_some(pane))
    }

    /// The live instance in `ws` currently running session `session_id`, if any (mounted or parked).
    pub(super) fn live_agent_of_session(&self, ws: &Path, session_id: &str) -> Option<AgentId> {
        self.agent_instances
            .values()
            .find(|a| same_folder(&a.cwd, ws) && a.meta.session_id.as_deref() == Some(session_id))
            .map(|a| a.id)
            .or_else(|| {
                self.claude_chats
                    .values()
                    .find(|c| same_folder(&c.cwd, ws) && c.meta.session_id.as_deref() == Some(session_id))
                    .map(|c| c.id)
            })
    }

    /// Current status lamp of live instance `id`.
    pub(super) fn instance_lamp(&self, id: AgentId) -> Lamp {
        let state = self
            .agent_instances
            .get(&id)
            .map(|a| a.tracker.state())
            .or_else(|| self.claude_chats.get(&id).map(|c| c.state))
            .unwrap_or_default();
        match state {
            AgentState::Idle => Lamp::Idle,
            AgentState::Working => Lamp::Working,
            AgentState::NeedsYou => Lamp::NeedsYou,
            AgentState::Done => Lamp::Done,
        }
    }

    /// Current status lamp of the instance mounted in `pane` (or `Idle` if pending).
    pub(super) fn agent_lamp(&self, pane: PaneId) -> Lamp {
        self.pane_agents
            .get(&pane)
            .or_else(|| self.pane_claude.get(&pane))
            .map_or(Lamp::Idle, |&id| self.instance_lamp(id))
    }

    /// Whether any live agent instance (CLI or Claude Chat, visible or parked) is currently working.
    pub(super) fn any_agent_working(&self) -> bool {
        self.agent_instances.values().any(|a| a.tracker.state() == AgentState::Working)
            || self.claude_chats.values().any(|c| c.state == AgentState::Working)
    }

    /// Whether any live instance inside `project` (or its workspaces) is working.
    pub(super) fn project_has_working_agent(&self, project: &Path) -> bool {
        let belongs = |cwd: &Path| {
            same_folder(cwd, project) || self.tile_projects.get(cwd).is_some_and(|p| same_folder(p, project))
        };
        self.agent_instances.values().any(|a| belongs(&a.cwd) && a.tracker.state() == AgentState::Working)
            || self.claude_chats.values().any(|c| belongs(&c.cwd) && c.state == AgentState::Working)
    }

    /// Rolled-up lamp for all live instances in `project` (across its workspaces, visible or parked).
    pub(super) fn project_lamp(&self, project: &Path) -> Lamp {
        let belongs = |cwd: &Path| {
            same_folder(cwd, project) || self.tile_projects.get(cwd).is_some_and(|p| same_folder(p, project))
        };
        let cli_lamps = self.agent_instances.values().filter(|a| belongs(&a.cwd)).map(|a| self.instance_lamp(a.id));
        let claude_lamps = self.claude_chats.values().filter(|c| belongs(&c.cwd)).map(|c| self.instance_lamp(c.id));
        Lamp::most_urgent(cli_lamps.chain(claude_lamps))
    }

    /// Live agent instances (CLI and Claude Chat) in workspace `ws`: `(id, session_id, label, lamp)`.
    pub(super) fn live_agents_in(&self, ws: &Path) -> Vec<(AgentId, Option<String>, String, Lamp)> {
        let mut list: Vec<(AgentId, Option<String>, String, Lamp)> = self
            .agent_instances
            .values()
            .filter(|a| same_folder(&a.cwd, ws) && a.meta.provider.is_some())
            .map(|a| (a.id, a.meta.session_id.clone(), self.agent_title(&a.cwd, &a.meta), self.instance_lamp(a.id)))
            .chain(self.claude_chats.values().filter(|c| same_folder(&c.cwd, ws) && c.meta.provider.is_some()).map(
                |c| (c.id, c.meta.session_id.clone(), self.agent_title(&c.cwd, &c.meta), self.instance_lamp(c.id)),
            ))
            .collect();
        list.sort_by_key(|(id, _, _, _)| *id);
        list
    }

    /// Human-readable title for an `agent_cli` instance / pane.
    pub(super) fn agent_title(&self, ws: &Path, meta: &AgentCliPane) -> String {
        if let Some(id) = meta.session_id.as_deref() {
            if let Some(name) = self.organize.name_of(ws, id) {
                return name;
            }
            if let Some(title) = self.listed_title(ws, id) {
                return title;
            }
        }
        if let Some(title) = meta.title.as_deref().filter(|t| !t.trim().is_empty()) {
            return title.to_string();
        }
        match meta.provider.as_deref() {
            Some(id) => self.provider_display_name(id),
            None => "New session".into(),
        }
    }

    /// Detaches whatever live instance is currently mounted in `pane`.
    /// Running instances stay alive in `agent_instances` (parked in the background);
    /// exited or errored instances are dropped cleanly.
    pub(super) fn detach_pane_agent(&mut self, pane: PaneId, cx: &Context<Self>) {
        let Some(old_id) = self.pane_agents.remove(&pane) else { return };
        let drop_dead = self
            .agent_instances
            .get(&old_id)
            .is_some_and(|a| a.error.is_some() || (!a.starting && a.view.as_ref().is_none_or(|v| v.read(cx).exited())));
        if drop_dead {
            self.agent_instances.remove(&old_id);
            self.clear_agent_alert(old_id, cx);
        }
    }

    /// Mounts live instance `agent_id` inside `pane` of workspace `ws`, parking whatever
    /// live instance was previously displayed in `pane`.
    pub(super) fn mount_agent_in_pane(
        &mut self,
        pane: PaneId,
        ws: &Path,
        agent_id: AgentId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.claude_chats.contains_key(&agent_id) {
            return self.mount_claude_chat_in_pane(pane, ws, agent_id, window, cx);
        }
        if self.pane_agents.get(&pane) == Some(&agent_id) {
            if let Some(t) = self.tiles.get_mut(ws) {
                t.focus_pane(pane);
            }
            self.mark_agent_seen(agent_id, cx);
            self.focus_input(window, cx);
            return;
        }
        // If `agent_id` was somehow mounted in another pane, vacate that slot first.
        if let Some(other) = self.pane_showing_agent(agent_id)
            && other != pane
        {
            self.pane_agents.remove(&other);
            if let Some(t) = self.tiles.get_mut(ws)
                && t.is_agent(other)
            {
                t.set_agent(other, AgentCliPane::pending());
                self.agent_pickers.insert(other, AgentPickerState::default());
            }
        }
        self.detach_pane_agent(pane, cx);
        self.detach_pane_claude_chat(pane);
        let Some(meta) = self.agent_instances.get(&agent_id).map(|a| a.meta.clone()) else { return };
        self.pane_agents.insert(pane, agent_id);
        self.agent_pickers.remove(&pane);
        if let Some(t) = self.tiles.get_mut(ws) {
            t.set_agent(pane, meta);
            t.focus_pane(pane);
        }
        self.mark_agent_seen(agent_id, cx);
        if let Some(view) = self.agent_instances.get(&agent_id).and_then(|a| a.view.clone()) {
            view.read(cx).focus_handle(cx).focus(window, cx);
        }
        self.save_layouts(cx);
        cx.notify();
    }

    /// Clears `Done` → `Idle` on `agent_id` and dismisses any stale OS notification.
    pub(super) fn mark_agent_seen(&mut self, agent_id: AgentId, cx: &Context<Self>) {
        if let Some(inst) = self.agent_instances.get_mut(&agent_id) {
            if inst.tracker.mark_seen() {
                self.clear_agent_alert(agent_id, cx);
            }
            return;
        }
        if let Some(chat) = self.claude_chats.get_mut(&agent_id)
            && chat.state == AgentState::Done
        {
            chat.state = AgentState::Idle;
            self.clear_agent_alert(agent_id, cx);
        }
    }

    /// Stops and removes live instance `agent_id` (and resets its pane to the picker if mounted).
    pub(super) fn stop_agent(&mut self, agent_id: AgentId, window: &mut Window, cx: &mut Context<Self>) {
        if self.claude_chats.contains_key(&agent_id) {
            return self.stop_claude_chat(agent_id, window, cx);
        }
        let mounted = self.pane_showing_agent(agent_id);
        let cwd = self.agent_instances.remove(&agent_id).map(|a| a.cwd);
        self.clear_agent_alert(agent_id, cx);
        if let Some(pane) = mounted {
            self.pane_agents.remove(&pane);
            if let Some(ws) = cwd
                && let Some(t) = self.tiles.get_mut(&ws)
                && t.is_agent(pane)
            {
                t.set_agent(pane, AgentCliPane::pending());
            }
            self.agent_pickers.insert(pane, AgentPickerState::default());
            if self.focused_pane_id() == Some(pane) {
                self.picker_focus.focus(window, cx);
            }
            self.save_layouts(cx);
        }
        self.sync_awake();
        cx.notify();
    }

    /// Brings live instance `agent_id` into view: switches to its project & workspace,
    /// focuses its pane if already visible, or swaps it into the focused/main agent pane
    /// (parking whatever was there).
    pub(super) fn go_to_agent(&mut self, agent_id: AgentId, window: &mut Window, cx: &mut Context<Self>) {
        let cwd = self
            .agent_instances
            .get(&agent_id)
            .map(|a| a.cwd.clone())
            .or_else(|| self.claude_chats.get(&agent_id).map(|c| c.cwd.clone()));
        let Some(cwd) = cwd else {
            // Backwards fallback if a caller passed a PaneId directly.
            if let Some((ws, _)) = self.agent_pane_location(agent_id) {
                let project = self.tile_projects.get(&ws).cloned().unwrap_or_else(|| ws.clone());
                if let Some(i) = self.saved.projects.iter().position(|p| same_folder(&p.path, &project)) {
                    self.select_project(i, window, cx);
                }
                self.select_workspace(&ws, cx);
                self.sync_tiles(window, cx);
                self.focus_pane(agent_id, window, cx);
                cx.notify();
            }
            return;
        };
        let project = self.tile_projects.get(&cwd).cloned().unwrap_or_else(|| cwd.clone());
        if let Some(i) = self.saved.projects.iter().position(|p| same_folder(&p.path, &project)) {
            self.select_project(i, window, cx);
        }
        self.select_workspace(&cwd, cx);
        self.sync_tiles(window, cx);
        if let Some(pane) = self.pane_showing_agent(agent_id)
            && self.tiles.get(&cwd).is_some_and(|t| t.has(pane))
        {
            self.focus_pane(pane, window, cx);
            cx.notify();
            return;
        }
        let target = self.tiles.get(&cwd).and_then(|t| t.agent_focus());
        let pane = match target {
            Some(p) => p,
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
        self.mount_agent_in_pane(pane, &cwd, agent_id, window, cx);
    }

    /// Launches either a native Claude Chat instance or a terminal CLI instance depending on `provider_id`.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn launch_agent(
        &mut self,
        pane: PaneId,
        ws: PathBuf,
        provider_id: String,
        resume_session: Option<String>,
        model: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let provider_id = agent_cli::normalize_provider_id(&provider_id);
        if agent_cli::is_claude_chat(&provider_id) {
            self.launch_claude_chat(pane, ws, provider_id, resume_session, None, model, window, cx);
        } else {
            self.launch_agent_cli(pane, ws, provider_id, resume_session, model, window, cx);
        }
    }

    /// Launches a fresh native CLI [`LiveAgent`] and mounts it in `pane` of workspace `ws`
    /// (parking whatever live instance was previously in `pane`).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn launch_agent_cli(
        &mut self,
        pane: PaneId,
        ws: PathBuf,
        provider_id: String,
        resume_session: Option<String>,
        model: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let provider_id = agent_cli::normalize_provider_id(&provider_id);
        let default_model = match provider_id.as_str() {
            agent_cli::CLAUDE_OPENROUTER => model
                .filter(|m| !m.trim().is_empty())
                .or_else(|| Some(self.saved.openrouter.default_model().to_string())),
            agent_cli::AGY_OPENROUTER => model
                .filter(|m| !m.trim().is_empty())
                .or_else(|| Some(kynex_core::provider::DEFAULT_ANTIGRAVITY_MODEL.to_string())),
            _ => model.filter(|m| !m.trim().is_empty()),
        };

        // Detach/park any previous instance occupying `pane`.
        self.detach_pane_agent(pane, cx);
        self.detach_pane_claude_chat(pane);

        let agent_id = self.alloc_agent_id();
        let meta = AgentCliPane {
            provider: Some(provider_id.clone()),
            session_id: resume_session.clone(),
            model: default_model.clone(),
            title: None,
        };
        let mut tracker = Tracker::default();
        if let Some(sid) = &resume_session {
            tracker.set_primary_session(sid.clone());
        }
        self.agent_instances.insert(
            agent_id,
            LiveAgent {
                id: agent_id,
                cwd: ws.clone(),
                meta: meta.clone(),
                view: None,
                starting: true,
                error: None,
                tracker,
            },
        );
        self.pane_agents.insert(pane, agent_id);
        self.agent_pickers.remove(&pane);
        if let Some(t) = self.tiles.get_mut(&ws) {
            t.set_agent(pane, meta);
        }
        if let Some(sid) = &resume_session {
            self.edit_groups(&ws, |g| g.set_provider(sid, &provider_id));
        }

        let rules = self.agent_rules.clone();
        let ws_bg = ws.clone();
        let prov_bg = provider_id.clone();
        let openrouter_key = self.openrouter_key.clone();
        let openrouter_cap = self.saved.openrouter.max_output_tokens;
        let control_addr = self.control.as_ref().map(|c| c.address().to_string());
        let task = cx.background_spawn(async move {
            let req = LaunchRequest {
                pane_id: pane,
                instance_id: agent_id,
                provider: &prov_bg,
                cwd: &ws_bg,
                resume_id: resume_session.as_deref(),
                model: default_model.as_deref(),
                openrouter_key: openrouter_key.as_deref(),
                openrouter_cap,
                control_addr: control_addr.as_deref(),
                rules: &rules,
            };
            agent_cli::build_spec(&req)
        });
        cx.spawn_in(window, async move |this, cx| {
            let spec = task.await;
            this.update_in(cx, |this, window, cx| {
                if !this.agent_instances.contains_key(&agent_id) {
                    return;
                }
                match spec {
                    Ok(spec) => {
                        let view = cx.new(|cx| TerminalView::new(spec, ws.clone(), cx));
                        cx.subscribe(&view, |_, _, _: &TerminalEvent, cx| cx.notify()).detach();
                        let focused = this
                            .pane_showing_agent(agent_id)
                            .is_some_and(|p| this.tiles.get(&ws).is_some_and(|t| t.focus() == Some(p)));
                        if focused {
                            view.read(cx).focus_handle(cx).focus(window, cx);
                        }
                        if let Some(inst) = this.agent_instances.get_mut(&agent_id) {
                            inst.starting = false;
                            inst.error = None;
                            inst.view = Some(view);
                        }
                        this.watch_agents(cx);
                        this.save_layouts(cx);
                    }
                    Err(e) => {
                        if let Some(inst) = this.agent_instances.get_mut(&agent_id) {
                            inst.starting = false;
                            inst.error = Some(e);
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Resets `pane` to the inline provider picker while keeping its live instance parked
    /// in the background (if still running). Used by New Session (`+` / `Ctrl+N`).
    pub(super) fn show_picker_in_pane(&mut self, pane: PaneId, window: &mut Window, cx: &mut Context<Self>) {
        let Some((ws, _)) = self.agent_pane_location(pane) else { return };
        self.detach_pane_agent(pane, cx);
        self.detach_pane_claude_chat(pane);
        if let Some(t) = self.tiles.get_mut(&ws) {
            t.set_agent(pane, AgentCliPane::pending());
            t.focus_pane(pane);
        }
        self.agent_pickers.insert(pane, AgentPickerState::default());
        self.picker_focus.focus(window, cx);
        self.save_layouts(cx);
        cx.notify();
    }

    /// Resets `pane` back to the inline provider picker and stops the instance that was in it.
    pub(super) fn reset_agent_pane_to_picker(&mut self, pane: PaneId, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.pane_agents.get(&pane).or_else(|| self.pane_claude.get(&pane)).copied() {
            self.stop_agent(id, window, cx);
            return;
        }
        self.show_picker_in_pane(pane, window, cx);
    }

    /// Restarts an agent pane (stopping its current CLI or Claude Chat instance and resuming its `session_id`).
    pub(super) fn restart_agent_cli(&mut self, pane: PaneId, window: &mut Window, cx: &mut Context<Self>) {
        let Some((ws, meta)) = self.agent_pane_location(pane) else { return };
        let Some(provider) = meta.provider else {
            return self.reset_agent_pane_to_picker(pane, window, cx);
        };
        if let Some(old_id) = self.pane_agents.remove(&pane) {
            self.agent_instances.remove(&old_id);
            self.clear_agent_alert(old_id, cx);
        }
        self.stop_pane_claude_chat(pane, cx);
        self.launch_agent(pane, ws, provider, meta.session_id, meta.model, window, cx);
    }

    /// Ensures a restored `agent_cli` pane that already has a provider has a mounted [`LiveAgent`].
    pub(super) fn ensure_agent_started(
        &mut self,
        pane: PaneId,
        ws: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.pane_agents.contains_key(&pane) {
            return;
        }
        let Some(meta) = self.tiles.get(ws).and_then(|t| t.agent(pane)).cloned() else { return };
        let Some(provider) = meta.provider else { return };
        if agent_cli::is_claude_chat(&provider) {
            return self.ensure_claude_chat_started(pane, ws, window, cx);
        }
        // If a parked live instance already exists for this session (e.g. after layout undo/redo),
        // re-mount it instead of spawning a duplicate CLI process.
        if let Some(sid) = meta.session_id.as_deref()
            && let Some(existing) = self.live_agent_of_session(ws, sid)
            && self.pane_showing_agent(existing).is_none()
        {
            self.pane_agents.insert(pane, existing);
            self.agent_pickers.remove(&pane);
            return;
        }
        // Wait for rules if this is a non-primary agent rule.
        let primary = matches!(
            provider.as_str(),
            agent_cli::CLAUDE
                | agent_cli::CLAUDE_OPENROUTER
                | agent_cli::AGY
                | agent_cli::AGY_OPENROUTER
                | agent_cli::GROK
        );
        if !primary && self.agent_rules.is_empty() {
            return;
        }
        self.launch_agent_cli(pane, ws.to_path_buf(), provider, meta.session_id, meta.model, window, cx);
    }

    /// Finds `(workspace_cwd, AgentCliPane)` for `pane` (or `agent_id`) across open workspaces.
    pub(super) fn agent_pane_location(&self, id: u64) -> Option<(PathBuf, AgentCliPane)> {
        if let Some(inst) = self.agent_instances.get(&id) {
            return Some((inst.cwd.clone(), inst.meta.clone()));
        }
        if let Some(chat) = self.claude_chats.get(&id) {
            return Some((chat.cwd.clone(), chat.meta.clone()));
        }
        for (ws, t) in &self.tiles {
            if let Some(meta) = t.agent(id) {
                return Some((ws.clone(), meta.clone()));
            }
        }
        None
    }

    /// Drops `pane_agents` / `agent_pickers` for closed panes (parked `agent_instances` survive
    /// until their workspace/project is dropped or the CLI exits).
    pub(super) fn drop_gone_agents(&mut self, cx: &Context<Self>) {
        let live_panes: Vec<PaneId> =
            self.tiles.values().flat_map(|t| t.agent_panes().into_iter().map(|(p, _)| p)).collect();
        let gone_panes: Vec<PaneId> = self.pane_agents.keys().copied().filter(|p| !live_panes.contains(p)).collect();
        for p in gone_panes {
            self.detach_pane_agent(p, cx);
        }
        self.agent_pickers.retain(|p, _| live_panes.contains(p));
    }

    /// Drops all live instances belonging to workspace `cwd` (when closing a project or removing a worktree).
    pub(super) fn drop_agents_in(&mut self, cwd: &Path, cx: &Context<Self>) {
        let gone: Vec<AgentId> =
            self.agent_instances.values().filter(|a| same_folder(&a.cwd, cwd)).map(|a| a.id).collect();
        for id in gone {
            self.agent_instances.remove(&id);
            self.clear_agent_alert(id, cx);
            self.pane_agents.retain(|_, v| *v != id);
        }
        self.drop_claude_chats_in(cwd, cx);
    }

    /// Whether the user is actively looking at live instance `agent_id` right now.
    pub(super) fn is_looking_at(&self, agent_id: AgentId) -> bool {
        if !self.window_active {
            return false;
        }
        let Some(focus) = self.tiling_scope().and_then(|(_, w)| self.tiles.get(&w)).and_then(|t| t.focus()) else {
            return false;
        };
        self.pane_agents.get(&focus) == Some(&agent_id) || self.pane_claude.get(&focus) == Some(&agent_id)
    }

    /// Handles a native CLI hook callback arriving over the control socket (`knx hook`).
    /// `target` is either a stable [`AgentId`] (from `KYNEX_SESSION`) or a [`PaneId`] fallback.
    pub(super) fn handle_hook_event(
        &mut self,
        target: u64,
        provider: &str,
        report: hooks::HookReport,
        cx: &mut Context<Self>,
    ) {
        let agent_id = if self.agent_instances.contains_key(&target) {
            target
        } else if let Some(&id) = self.pane_agents.get(&target) {
            id
        } else {
            return;
        };

        let looking = self.is_looking_at(agent_id);
        let mut layout_dirty = false;
        let (ws, prov_id, was, now, meta_snapshot, session_changed) = {
            let Some(inst) = self.agent_instances.get_mut(&agent_id) else { return };
            let prov_id = inst.meta.provider.clone().unwrap_or_else(|| agent_cli::normalize_provider_id(provider));
            let was = inst.tracker.state();
            let outcome = inst.tracker.on_hook_for(&prov_id, &report, looking);
            let mut session_changed = None;
            if let Some(sid) = outcome.session_id
                && inst.meta.session_id.as_deref() != Some(sid.as_str())
            {
                inst.meta.session_id = Some(sid.clone());
                session_changed = Some((sid, prov_id.clone()));
            }
            (inst.cwd.clone(), prov_id, was, outcome.state, inst.meta.clone(), session_changed)
        };

        if let Some((sid, prov)) = session_changed {
            if let Some(pane) = self.pane_showing_agent(agent_id)
                && let Some(t) = self.tiles.get_mut(&ws)
            {
                t.set_agent(pane, meta_snapshot.clone());
                layout_dirty = true;
            }
            self.edit_groups(&ws, |g| g.set_provider(&sid, &prov));
            self.reread_sessions();
        }

        if now != was {
            let event_pane = self.pane_showing_agent(agent_id).unwrap_or(agent_id);
            self.agent_event(event_pane, &prov_id, was, now);
            let name = self.agent_title(&ws, &meta_snapshot);
            self.sync_agent_alert(agent_id, &name, now, cx);
            if matches!(now, AgentState::Done | AgentState::Idle) && was == AgentState::Working {
                if let Some(project) = self.tile_projects.get(&ws).cloned() {
                    self.refresh_worktrees([&project], cx);
                }
                if agent_cli::uses_openrouter(&prov_id) {
                    self.refresh_openrouter_credit(cx);
                }
                self.reread_sessions();
            }
        }
        if layout_dirty {
            self.save_layouts(cx);
        }
        self.finish_control_waits(cx);
        self.sync_awake();
        cx.notify();
    }

    /// Starts the 500ms background screen watcher while any live `agent_cli` instances exist.
    pub(super) fn watch_agents(&mut self, cx: &mut Context<Self>) {
        if self.agent_watch {
            return;
        }
        self.agent_watch = true;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(LOOK_EVERY).await;
                let more = this.update(cx, |this, cx| this.look_at_agents(cx)).unwrap_or(false);
                if !more {
                    break;
                }
            }
        })
        .detach();
    }

    /// Scans all live `agent_cli` terminal screens (visible and parked) for status, resume IDs,
    /// and cleans up parked instances whose CLI process has exited.
    fn look_at_agents(&mut self, cx: &mut Context<Self>) -> bool {
        if self.agent_instances.is_empty() {
            self.agent_watch = false;
            return false;
        }
        let ids: Vec<AgentId> = self.agent_instances.keys().copied().collect();
        let mut changed = false;
        let mut layout_dirty = false;
        let mut exited_parked = Vec::new();

        for agent_id in ids {
            let mounted = self.pane_showing_agent(agent_id);
            let looking = self.is_looking_at(agent_id);
            let Some((ws, provider_id, view, starting, has_session)) = self.agent_instances.get(&agent_id).map(|a| {
                (a.cwd.clone(), a.meta.provider.clone(), a.view.clone(), a.starting, a.meta.session_id.is_some())
            }) else {
                continue;
            };
            let Some(provider_id) = provider_id else { continue };
            let Some(view) = view else { continue };

            // Auto-prune parked instances whose CLI process exited cleanly.
            if mounted.is_none() && !starting && view.read(cx).exited() {
                exited_parked.push(agent_id);
                continue;
            }

            let Some(screen) = view.read(cx).screen_text() else { continue };
            let base = agent_cli::rule_id(&provider_id);
            let rule = self.agent_rules.iter().find(|r| r.id == base || r.id == provider_id).cloned();

            if let Some(rule) = &rule {
                let detected = agent_panes::detect(rule, &screen);
                let resume = (!has_session).then(|| agent_panes::resume_id(rule, &screen)).flatten();
                let (was, now, meta_snapshot) = {
                    let Some(inst) = self.agent_instances.get_mut(&agent_id) else { continue };
                    let was = inst.tracker.state();
                    let now = inst.tracker.update(detected, looking);
                    if let Some(r) = &resume {
                        inst.meta.session_id = Some(r.clone());
                        inst.tracker.set_primary_session(r.clone());
                    }
                    (was, now, inst.meta.clone())
                };
                if now != was {
                    changed = true;
                    let event_pane = mounted.unwrap_or(agent_id);
                    self.agent_event(event_pane, &provider_id, was, now);
                    let name = self.agent_title(&ws, &meta_snapshot);
                    self.sync_agent_alert(agent_id, &name, now, cx);
                    if matches!(now, AgentState::Done | AgentState::Idle) && was == AgentState::Working {
                        if let Some(project) = self.tile_projects.get(&ws).cloned() {
                            self.refresh_worktrees([&project], cx);
                        }
                        self.reread_sessions();
                    }
                }
                if let Some(resume) = resume {
                    if let Some(pane) = mounted
                        && let Some(t) = self.tiles.get_mut(&ws)
                    {
                        t.set_agent(pane, meta_snapshot);
                        layout_dirty = true;
                    }
                    self.edit_groups(&ws, |g| g.set_provider(&resume, &provider_id));
                    self.reread_sessions();
                    changed = true;
                }
            } else if looking
                && let Some(inst) = self.agent_instances.get_mut(&agent_id)
                && inst.tracker.mark_seen()
            {
                self.clear_agent_alert(agent_id, cx);
                changed = true;
            }
        }

        if !exited_parked.is_empty() {
            for id in exited_parked {
                self.agent_instances.remove(&id);
                self.clear_agent_alert(id, cx);
            }
            self.reread_sessions();
            changed = true;
        }
        if layout_dirty {
            self.save_layouts(cx);
        }
        if changed {
            self.sync_awake();
            cx.notify();
        }
        !self.agent_instances.is_empty()
    }

    /// Renders an `agent_cli` pane: 26px draggable header + inline provider picker or GPU terminal.
    pub(super) fn render_agent_cli_pane(
        &mut self,
        pane: PaneId,
        ws: &Path,
        several: bool,
        focused: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let meta = self.tiles.get(ws).and_then(|t| t.agent(pane)).cloned().unwrap_or_default();
        if meta.provider.is_some() {
            self.ensure_agent_started(pane, ws, window, cx);
        }
        let live_meta = self.pane_agent(pane).map(|a| a.meta.clone()).unwrap_or(meta);
        let t = theme(cx).clone();
        let lamp_state = self.agent_lamp(pane);
        let doing = self.pane_agent(pane).map(|a| a.tracker.detail().to_string()).unwrap_or_default();
        let title = self.agent_title(ws, &live_meta);
        let badge = live_meta.provider.as_deref().map(|id| self.provider_badge(id));
        let zoomed = self.tiles.get(ws).is_some_and(|tl| tl.zoomed().is_some());
        let exited = self.pane_agent(pane).and_then(|a| a.view.as_ref()).is_some_and(|v| v.read(cx).exited());
        let n = pane as usize;

        let header_content = div()
            .id(("agent-header", n))
            .flex()
            .flex_none()
            .items_center()
            .gap_1p5()
            .h(px(26.))
            .pl_2()
            .pr_1()
            .border_b_1()
            .border_color(t.border_variant)
            .text_xs()
            .child(lamp(("agent-lamp", n), lamp_state, cx))
            .when_some(badge, |d, b| {
                d.child(
                    div()
                        .flex_none()
                        .px_1()
                        .rounded_sm()
                        .border_1()
                        .border_color(t.border_variant)
                        .font_family(mono())
                        .text_size(px(10.))
                        .text_color(t.text_muted)
                        .child(b),
                )
            })
            .child(div().flex_1().min_w_0().truncate().text_color(t.text).child(title.clone()))
            .when(!doing.is_empty() && lamp_state == Lamp::Working, |d| {
                d.child(div().flex_none().max_w(px(160.)).truncate().text_color(t.text_placeholder).child(doing))
            })
            .when(live_meta.provider.is_some(), |d| {
                d.child(
                    icon_button(("agent-restart", n), Icon::Refresh, cx)
                        .size(px(20.))
                        .tooltip(Tooltip::text("Restart CLI"))
                        .on_click(cx.listener(move |this, _, window, cx| this.restart_agent_cli(pane, window, cx))),
                )
            })
            .when(several, |d| {
                d.child(
                    icon_button(("pane-zoom", n), if zoomed { Icon::Restore } else { Icon::Maximize }, cx)
                        .size(px(20.))
                        .tooltip(Tooltip::text(if zoomed {
                            "Show the other panes again"
                        } else {
                            "Zoom: fill the tab"
                        }))
                        .on_click(cx.listener(|this, _, window, cx| this.toggle_pane_zoom(window, cx))),
                )
                .child(
                    icon_button(("pane-close", n), Icon::Close, cx)
                        .size(px(20.))
                        .tooltip(Tooltip::text("Close pane"))
                        .on_click(cx.listener(move |this, _, window, cx| this.close_pane_id(pane, window, cx))),
                )
            })
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.open_menu(Menu::Pane(pane), ev.position, gpui::Anchor::TopLeft, cx)
                }),
            );

        let header = pane_handle(header_content, Some(pane), &title);

        let body: AnyElement = if live_meta.provider.is_none() {
            self.render_provider_picker(pane, ws, focused, cx)
        } else if let Some(err) = self.pane_agent(pane).and_then(|a| a.error.clone()) {
            div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_3()
                .p_6()
                .child(div().text_sm().text_color(t.error).child(err))
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(
                            primary_button(("agent-retry", n), "Retry", cx).on_click(
                                cx.listener(move |this, _, window, cx| this.restart_agent_cli(pane, window, cx)),
                            ),
                        )
                        .child(outline_button(("agent-pick-other", n), "Choose another provider", cx).on_click(
                            cx.listener(move |this, _, window, cx| this.reset_agent_pane_to_picker(pane, window, cx)),
                        )),
                )
                .into_any_element()
        } else if let Some(view) = self.pane_agent(pane).and_then(|a| a.view.clone()) {
            let banner = exited.then(|| {
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_1p5()
                    .bg(t.panel)
                    .border_b_1()
                    .border_color(t.border_variant)
                    .text_xs()
                    .text_color(t.text_muted)
                    .child("CLI process exited.")
                    .child(div().flex_1())
                    .child(
                        outline_button(("agent-exited-restart", n), "Restart", cx)
                            .on_click(cx.listener(move |this, _, window, cx| this.restart_agent_cli(pane, window, cx))),
                    )
                    .child(ghost_button(("agent-exited-switch", n), cx).text_xs().child("Change provider").on_click(
                        cx.listener(move |this, _, window, cx| this.reset_agent_pane_to_picker(pane, window, cx)),
                    ))
            });
            div()
                .size_full()
                .flex()
                .flex_col()
                .children(banner)
                .child(div().flex_1().min_h_0().child(view))
                .into_any_element()
        } else {
            let label = live_meta.provider.as_deref().map(|id| self.provider_display_name(id)).unwrap_or_default();
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .gap_2()
                .text_sm()
                .text_color(t.text_muted)
                .child(spinner(("agent-starting", n), t.accent, px(14.)))
                .child(format!("Starting {label}…"))
                .into_any_element()
        };

        div()
            .key_context("Pane")
            .size_full()
            .flex()
            .flex_col()
            .child(header)
            .child(div().flex_1().min_h_0().child(body))
            .into_any_element()
    }
}
