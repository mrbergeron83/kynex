//! Live native Claude Code chat instances (`ClaudeChatView`) and pane hosting.
//!
//! Mirrors `src/app/agent_cli.rs` for `claude` and `claude-openrouter`: each running session
//! lives in `claude_chats` under a stable [`AgentId`], and mounted panes reference it via
//! `pane_claude`. Switching sessions or starting a new session detaches the pane while keeping
//! the live instance parked in the background so active turns keep streaming and updating
//! status lamps, the Sessions tree, the Agents pane, OS alerts, and Keep Awake.

use std::path::{Path, PathBuf};

use gpui::{AnyElement, App, Context, Entity, Focusable, MouseButton, MouseDownEvent, Window, div, prelude::*};
use kynex_core::agent_cli;
use kynex_core::agent_panes::AgentState;
use kynex_core::control::agent as control_agent;
use kynex_core::layout::{Axis, Direction, PaneId};
use kynex_core::provider::{Launch, OpenRouterSettings};
use kynex_core::tiling::AgentCliPane;
use kynex_core::toasts::Toast;
use kynex_core::workspace::same_folder;

use super::KynexApp;
use super::agent_cli::AgentId;
use super::agent_picker::AgentPickerState;
use super::menus::Menu;
use crate::claude_chat::{ClaudeChatConfig, ClaudeChatEvent, ClaudeChatView, SettingsPage};
use crate::claude_pane::{self, ClaudePane};
use crate::theme::{mono, theme};
use crate::ui::pane_drag::pane_handle;
use crate::ui::{Icon, Lamp, Tooltip, icon_button, lamp, px};

/// One live native Claude Code chat instance (mounted in a pane or parked in the background).
pub(super) struct LiveClaudeChat {
    pub id: AgentId,
    pub cwd: PathBuf,
    pub meta: AgentCliPane,
    pub view: ChatView,
    pub state: AgentState,
    pub detail: String,
}

/// The view a [`LiveClaudeChat`] draws in: today's `ClaudeChatView`, or the old chat
/// (`ClaudePane`) when `KYNEX_OLD_CHAT=1` while it's ported back (#545).
#[derive(Clone)]
pub(super) enum ChatView {
    New(Entity<ClaudeChatView>),
    Old(Entity<ClaudePane>),
}

impl ChatView {
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        let handle = match self {
            ChatView::New(v) => v.read(cx).focus_handle(cx),
            ChatView::Old(v) => v.read(cx).focus_handle(cx),
        };
        handle.focus(window, cx);
    }

    pub fn stop(&self, cx: &mut App) {
        match self {
            ChatView::New(v) => v.update(cx, |v, cx| v.stop(cx)),
            ChatView::Old(v) => v.update(cx, |v, cx| v.stop(cx)),
        }
    }

    pub fn cancel_turn(&self, cx: &mut App) {
        match self {
            ChatView::New(v) => v.update(cx, |v, cx| v.cancel_turn(cx)),
            ChatView::Old(v) => v.update(cx, |v, cx| v.cancel_turn(cx)),
        }
    }

    pub fn send_external_prompt(&self, text: String, cx: &mut App) {
        match self {
            ChatView::New(v) => v.update(cx, |v, cx| v.send_external_prompt(text, cx)),
            ChatView::Old(v) => v.update(cx, |v, cx| v.send_external_prompt(text, cx)),
        }
    }

    fn element(&self) -> AnyElement {
        match self {
            ChatView::New(v) => v.clone().into_any_element(),
            ChatView::Old(v) => v.clone().into_any_element(),
        }
    }
}

impl KynexApp {
    /// The [`LiveClaudeChat`] currently mounted in `pane`, if any.
    pub(super) fn pane_claude_chat(&self, pane: PaneId) -> Option<&LiveClaudeChat> {
        let id = self.pane_claude.get(&pane)?;
        self.claude_chats.get(id)
    }

    /// Detaches whatever [`LiveClaudeChat`] is currently mounted in `pane`, keeping it parked
    /// alive in `claude_chats` so active turns continue running in the background.
    pub(super) fn detach_pane_claude_chat(&mut self, pane: PaneId) {
        self.pane_claude.remove(&pane);
    }

    /// Stops and removes any [`LiveClaudeChat`] mounted in `pane` (used when restarting a pane).
    pub(super) fn stop_pane_claude_chat(&mut self, pane: PaneId, cx: &mut Context<Self>) {
        if let Some(old_id) = self.pane_claude.remove(&pane) {
            if let Some(chat) = self.claude_chats.remove(&old_id) {
                chat.view.stop(cx);
            }
            self.clear_agent_alert(old_id, cx);
        }
    }

    /// Mounts live Claude Chat `agent_id` inside `pane` of workspace `ws`, parking whatever
    /// live instance was previously displayed in `pane`.
    pub(super) fn mount_claude_chat_in_pane(
        &mut self,
        pane: PaneId,
        ws: &Path,
        agent_id: AgentId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.pane_claude.get(&pane) == Some(&agent_id) {
            if let Some(t) = self.tiles.get_mut(ws) {
                t.focus_pane(pane);
            }
            self.mark_agent_seen(agent_id, cx);
            self.focus_input(window, cx);
            return;
        }
        if let Some(other) = self.pane_showing_agent(agent_id)
            && other != pane
        {
            self.pane_claude.remove(&other);
            if let Some(t) = self.tiles.get_mut(ws)
                && t.is_agent(other)
            {
                t.set_agent(other, AgentCliPane::pending());
                self.agent_pickers.insert(other, AgentPickerState::default());
            }
        }
        self.detach_pane_agent(pane, cx);
        self.detach_pane_claude_chat(pane);
        let Some((meta, view)) = self.claude_chats.get(&agent_id).map(|c| (c.meta.clone(), c.view.clone())) else {
            return;
        };
        self.pane_claude.insert(pane, agent_id);
        self.agent_pickers.remove(&pane);
        if let Some(t) = self.tiles.get_mut(ws) {
            t.set_agent(pane, meta);
            t.focus_pane(pane);
        }
        self.mark_agent_seen(agent_id, cx);
        view.focus(window, cx);
        self.save_layouts(cx);
        cx.notify();
    }

    /// Launches a fresh native [`LiveClaudeChat`] and mounts it in `pane` of workspace `ws`
    /// (parking whatever live instance was previously in `pane`).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn launch_claude_chat(
        &mut self,
        pane: PaneId,
        ws: PathBuf,
        provider_id: String,
        resume_session: Option<String>,
        fork_from: Option<String>,
        model: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let provider_id = agent_cli::normalize_provider_id(&provider_id);
        let default_model = if provider_id == agent_cli::CLAUDE_OPENROUTER {
            model.filter(|m| !m.trim().is_empty()).or_else(|| Some(self.saved.openrouter.default_model().to_string()))
        } else {
            model.filter(|m| !m.trim().is_empty())
        };

        let launch = if provider_id == agent_cli::CLAUDE_OPENROUTER {
            let Some(key) = self.openrouter_key.clone() else {
                self.toast(Toast::error("No OpenRouter key is saved. Add one in Settings → Providers.".to_string()));
                cx.notify();
                return;
            };
            Launch::OpenRouter {
                key,
                settings: OpenRouterSettings {
                    model: default_model.clone().unwrap_or_else(|| self.saved.openrouter.default_model().to_string()),
                    max_output_tokens: self.saved.openrouter.max_output_tokens,
                },
            }
        } else {
            Launch::Anthropic
        };

        // Detach/park any previous instance occupying `pane`.
        self.detach_pane_agent(pane, cx);
        self.detach_pane_claude_chat(pane);

        let agent_id = self.alloc_agent_id();
        let mut extra_env = vec![
            (control_agent::PANE_VAR.to_string(), pane.to_string()),
            (control_agent::SESSION_VAR.to_string(), agent_id.to_string()),
            (control_agent::WORKSPACE_VAR.to_string(), ws.to_string_lossy().into_owned()),
            ("KYNEX_PROVIDER".to_string(), provider_id.clone()),
        ];
        if let Some(addr) = self.control.as_ref().map(|c| c.address().to_string()) {
            extra_env.push((control_agent::CONTROL_VAR.to_string(), addr));
        }

        let config = ClaudeChatConfig {
            workspace: ws.clone(),
            provider: provider_id.clone(),
            model: default_model.clone(),
            resume_session_id: resume_session.clone(),
            fork_from,
            launch,
            openrouter_models: self.saved.openrouter.models.clone(),
            openrouter: self.saved.openrouter.clone(),
            extra_env,
        };
        let view = if claude_pane::enabled() {
            let settings_dir = self.tile_projects.get(&ws).cloned().filter(|p| !same_folder(p, &ws));
            let column = self.saved.column;
            let view = cx.new(|cx| {
                let mut pane = ClaudePane::new(config, window, cx);
                pane.column = column;
                if let Some(dir) = settings_dir {
                    pane.set_settings_dir(dir);
                }
                pane
            });
            cx.subscribe_in(&view, window, move |this, _, ev: &ClaudeChatEvent, window, cx| {
                this.on_claude_chat_event(agent_id, ev, window, cx);
            })
            .detach();
            ChatView::Old(view)
        } else {
            let view = cx.new(|cx| ClaudeChatView::new(config, window, cx));
            cx.subscribe_in(&view, window, move |this, _, ev: &ClaudeChatEvent, window, cx| {
                this.on_claude_chat_event(agent_id, ev, window, cx);
            })
            .detach();
            ChatView::New(view)
        };

        let meta = AgentCliPane {
            provider: Some(provider_id.clone()),
            session_id: resume_session.clone(),
            model: default_model,
            title: None,
        };
        self.claude_chats.insert(
            agent_id,
            LiveClaudeChat {
                id: agent_id,
                cwd: ws.clone(),
                meta: meta.clone(),
                view: view.clone(),
                state: AgentState::Idle,
                detail: String::new(),
            },
        );
        self.pane_claude.insert(pane, agent_id);
        self.agent_pickers.remove(&pane);
        if let Some(t) = self.tiles.get_mut(&ws) {
            t.set_agent(pane, meta);
        }
        if let Some(sid) = &resume_session {
            self.edit_groups(&ws, |g| g.set_provider(sid, &provider_id));
        }
        let focused = self.tiles.get(&ws).is_some_and(|t| t.focus() == Some(pane));
        if focused {
            view.focus(window, cx);
        }
        self.save_layouts(cx);
        self.sync_awake();
        cx.notify();
    }

    /// Forks a running Claude session into a new `ClaudeChat` pane split beside the active pane.
    pub(super) fn fork_claude_chat(
        &mut self,
        from_agent_id: AgentId,
        from_session_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((cwd, provider, model)) = self.claude_chats.get(&from_agent_id).map(|c| {
            (
                c.cwd.clone(),
                c.meta.provider.clone().unwrap_or_else(|| agent_cli::CLAUDE.to_string()),
                c.meta.model.clone(),
            )
        }) else {
            return;
        };
        let target = self.pane_showing_agent(from_agent_id).or_else(|| self.tiles.get(&cwd).and_then(|t| t.focus()));
        let pane = self.next_pane;
        let bounds = self.tile_bounds();
        let placed = {
            let Some(t) = self.tiles.get_mut(&cwd) else { return };
            match target {
                Some(tgt) => t.split_agent_at(tgt, Direction::Right, pane, AgentCliPane::pending(), bounds),
                None => t.split_agent(Axis::Row, pane, AgentCliPane::pending(), bounds),
            }
        };
        if !placed {
            self.toast(Toast::error("No room to fork beside this pane: the panes would get too small.".to_string()));
            cx.notify();
            return;
        }
        self.next_pane += 1;
        self.launch_claude_chat(pane, cwd, provider, None, Some(from_session_id), model, window, cx);
    }

    /// Stops and removes live Claude Chat `agent_id` (and resets its pane to the picker if mounted).
    pub(super) fn stop_claude_chat(&mut self, agent_id: AgentId, window: &mut Window, cx: &mut Context<Self>) {
        let mounted = self.pane_showing_agent(agent_id);
        let cwd = if let Some(chat) = self.claude_chats.remove(&agent_id) {
            chat.view.stop(cx);
            Some(chat.cwd)
        } else {
            None
        };
        self.clear_agent_alert(agent_id, cx);
        if let Some(pane) = mounted {
            self.pane_claude.remove(&pane);
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

    /// Ensures a restored `ClaudeChat` pane that already has a provider has a mounted [`LiveClaudeChat`].
    pub(super) fn ensure_claude_chat_started(
        &mut self,
        pane: PaneId,
        ws: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.pane_claude.contains_key(&pane) {
            return;
        }
        let Some(meta) = self.tiles.get(ws).and_then(|t| t.agent(pane)).cloned() else { return };
        let Some(provider) = meta.provider else { return };
        if !agent_cli::is_claude_chat(&provider) {
            return;
        }
        // Re-mount a parked live instance if one already exists for this session (e.g. layout undo/redo).
        if let Some(sid) = meta.session_id.as_deref()
            && let Some(existing) = self.live_agent_of_session(ws, sid)
            && self.claude_chats.contains_key(&existing)
            && self.pane_showing_agent(existing).is_none()
        {
            self.pane_claude.insert(pane, existing);
            self.agent_pickers.remove(&pane);
            return;
        }
        self.launch_claude_chat(pane, ws.to_path_buf(), provider, meta.session_id, None, meta.model, window, cx);
    }

    /// Detaches `pane_claude` entries for closed panes while keeping live instances parked
    /// in `claude_chats` until their workspace/project closes or the user stops them.
    pub(super) fn drop_gone_claude_chats(&mut self, _cx: &Context<Self>) {
        let live_panes: Vec<PaneId> =
            self.tiles.values().flat_map(|t| t.agent_panes().into_iter().map(|(p, _)| p)).collect();
        let gone_panes: Vec<PaneId> = self.pane_claude.keys().copied().filter(|p| !live_panes.contains(p)).collect();
        for p in gone_panes {
            self.detach_pane_claude_chat(p);
        }
    }

    /// Drops all live Claude Chat instances belonging to workspace `cwd` (when closing a project
    /// or removing a worktree). Dropping the last `Entity<ClaudeChatView>` triggers its release cleanup.
    pub(super) fn drop_claude_chats_in(&mut self, cwd: &Path, cx: &Context<Self>) {
        let gone: Vec<AgentId> =
            self.claude_chats.values().filter(|c| same_folder(&c.cwd, cwd)).map(|c| c.id).collect();
        for id in gone {
            self.claude_chats.remove(&id);
            self.clear_agent_alert(id, cx);
            self.pane_claude.retain(|_, v| *v != id);
        }
    }

    /// Handles events emitted by a live [`ClaudeChatView`].
    pub(super) fn on_claude_chat_event(
        &mut self,
        agent_id: AgentId,
        ev: &ClaudeChatEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match ev {
            ClaudeChatEvent::StatusChanged { state, detail } => {
                let looking = self.is_looking_at(agent_id);
                let effective = if *state == AgentState::Done && looking { AgentState::Idle } else { *state };
                let Some((ws, prov_id, was, meta_snapshot)) = self.claude_chats.get_mut(&agent_id).map(|chat| {
                    let was = chat.state;
                    chat.state = effective;
                    chat.detail = detail.clone();
                    let prov_id = chat.meta.provider.clone().unwrap_or_else(|| agent_cli::CLAUDE.to_string());
                    (chat.cwd.clone(), prov_id, was, chat.meta.clone())
                }) else {
                    return;
                };
                if effective != was {
                    let event_pane = self.pane_showing_agent(agent_id).unwrap_or(agent_id);
                    self.agent_event(event_pane, &prov_id, was, effective);
                    let name = self.agent_title(&ws, &meta_snapshot);
                    self.sync_agent_alert(agent_id, &name, effective, cx);
                    self.finish_control_waits(cx);
                    self.sync_awake();
                }
                cx.notify();
            }
            ClaudeChatEvent::SessionId(sid) => {
                let Some((ws, prov_id, meta_snapshot, changed)) = self.claude_chats.get_mut(&agent_id).map(|chat| {
                    let changed = chat.meta.session_id.as_deref() != Some(sid.as_str());
                    if changed {
                        chat.meta.session_id = Some(sid.clone());
                    }
                    let prov_id = chat.meta.provider.clone().unwrap_or_else(|| agent_cli::CLAUDE.to_string());
                    (chat.cwd.clone(), prov_id, chat.meta.clone(), changed)
                }) else {
                    return;
                };
                if changed {
                    if let Some(pane) = self.pane_showing_agent(agent_id)
                        && let Some(t) = self.tiles.get_mut(&ws)
                    {
                        t.set_agent(pane, meta_snapshot);
                        self.save_layouts(cx);
                    }
                    self.edit_groups(&ws, |g| g.set_provider(sid, &prov_id));
                    self.reread_sessions();
                    cx.notify();
                }
            }
            ClaudeChatEvent::TitleChanged(title) => {
                let Some((ws, meta_snapshot, changed)) = self.claude_chats.get_mut(&agent_id).map(|chat| {
                    let changed = chat.meta.title.as_deref() != Some(title.as_str());
                    if changed {
                        chat.meta.title = Some(title.clone());
                    }
                    (chat.cwd.clone(), chat.meta.clone(), changed)
                }) else {
                    return;
                };
                if changed {
                    if let Some(pane) = self.pane_showing_agent(agent_id)
                        && let Some(t) = self.tiles.get_mut(&ws)
                    {
                        t.set_agent(pane, meta_snapshot);
                        self.save_layouts(cx);
                    }
                    cx.notify();
                }
            }
            ClaudeChatEvent::TurnFinished => {
                let Some((ws, prov_id)) = self.claude_chats.get(&agent_id).map(|chat| {
                    (chat.cwd.clone(), chat.meta.provider.clone().unwrap_or_else(|| agent_cli::CLAUDE.to_string()))
                }) else {
                    return;
                };
                if let Some(project) = self.tile_projects.get(&ws).cloned() {
                    self.refresh_worktrees([&project], cx);
                }
                if agent_cli::uses_openrouter(&prov_id) {
                    self.refresh_openrouter_credit(cx);
                }
                self.reread_sessions();
                self.finish_control_waits(cx);
                self.sync_awake();
                cx.notify();
            }
            ClaudeChatEvent::ForkRequested { from_session_id } => {
                self.fork_claude_chat(agent_id, from_session_id.clone(), window, cx);
            }
            ClaudeChatEvent::RunInTerminal(command) => {
                self.run_in_workspace_terminal(command.clone(), window, cx);
            }
            ClaudeChatEvent::OpenFile { path, line } => {
                let resolved = if path.is_absolute() {
                    path.clone()
                } else if let Some(cwd) = self.claude_chats.get(&agent_id).map(|c| &c.cwd) {
                    cwd.join(path)
                } else {
                    path.clone()
                };
                self.open_file_view(resolved, *line, window, cx);
            }
            ClaudeChatEvent::OpenFileChanges { cwd, path } => {
                self.open_file_changes(cwd.clone(), path.clone(), window, cx);
            }
            ClaudeChatEvent::Toast(toast) => {
                self.toast(toast.clone());
                cx.notify();
            }
            ClaudeChatEvent::ToastDone { progress, outcome } => {
                let progress = progress.clone();
                let busy = kynex_core::toasts::Kind::Progress;
                self.toasts.dismiss_where(|t| t.kind == busy && t.title == progress);
                if let Some(t) = outcome {
                    self.toast(t.clone());
                }
                cx.notify();
            }
            ClaudeChatEvent::NewSession => {
                let Some((cwd, provider)) = self
                    .claude_chats
                    .get(&agent_id)
                    .map(|c| (c.cwd.clone(), c.meta.provider.clone().unwrap_or_else(|| agent_cli::CLAUDE.to_string())))
                else {
                    return;
                };
                if let Some(pane) = self.pane_showing_agent(agent_id) {
                    self.launch_claude_chat(pane, cwd, provider, None, None, None, window, cx);
                }
            }
            ClaudeChatEvent::ShowSessions => {
                if let Some(pane) = self.pane_showing_agent(agent_id) {
                    self.show_sessions_pane(pane, window, cx);
                }
            }
            ClaudeChatEvent::OpenSettings(page) => {
                let section = match page {
                    SettingsPage::Providers => super::app_settings::PROVIDERS_SECTION,
                    SettingsPage::Keyboard => super::app_settings::KEYBOARD_SECTION,
                };
                self.open_settings_on(section, cx);
            }
            ClaudeChatEvent::ProjectSettingsChanged(dir) => {
                for chat in self.claude_chats.values().filter(|c| c.id != agent_id) {
                    if let ChatView::Old(view) = &chat.view {
                        let dir = dir.clone();
                        view.update(cx, |v, cx| v.settings_changed(&dir, cx));
                    }
                }
            }
            ClaudeChatEvent::ModelPinned(model) => {
                let Some((ws, meta)) = self.claude_chats.get_mut(&agent_id).map(|chat| {
                    chat.meta.model = model.clone();
                    (chat.cwd.clone(), chat.meta.clone())
                }) else {
                    return;
                };
                if let Some(pane) = self.pane_showing_agent(agent_id)
                    && let Some(t) = self.tiles.get_mut(&ws)
                {
                    t.set_agent(pane, meta);
                }
                self.save_layouts(cx);
            }
        }
    }

    /// Renders a `ClaudeChat` pane: 26px draggable `pane_handle` header + `ClaudeChatView`.
    pub(super) fn render_claude_chat_pane(
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
            self.ensure_claude_chat_started(pane, ws, window, cx);
        }
        let live_meta = self.pane_claude_chat(pane).map(|c| c.meta.clone()).unwrap_or(meta);
        let t = theme(cx).clone();
        let lamp_state = self.agent_lamp(pane);
        let doing = self.pane_claude_chat(pane).map(|c| c.detail.clone()).unwrap_or_default();
        let title = self.agent_title(ws, &live_meta);
        let badge = live_meta.provider.as_deref().map(|id| self.provider_badge(id));
        let zoomed = self.tiles.get(ws).is_some_and(|tl| tl.zoomed().is_some());
        let n = pane as usize;

        let header_content = div()
            .id(("claude-chat-header", n))
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
            .child(lamp(("claude-chat-lamp", n), lamp_state, cx))
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
                    icon_button(("claude-chat-restart", n), Icon::Refresh, cx)
                        .size(px(20.))
                        .tooltip(Tooltip::text("Restart session"))
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

        let body: AnyElement = if let Some(view) = self.pane_claude_chat(pane).map(|c| c.view.clone()) {
            div().size_full().child(view.element()).into_any_element()
        } else {
            self.render_provider_picker(pane, ws, focused, cx)
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
