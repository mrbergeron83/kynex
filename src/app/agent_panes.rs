//! Other agent CLIs in terminal panes (`kynex_core::agent_panes`): "New agent pane" starts
//! one in the workspace's folder, and every half second each agent pane's screen is read
//! against its agent's rule. What it says drives the pane's lamp, the tab's and the
//! workspace's rollups, and the same toasts, sounds and notifications as a session. An
//! agent that printed its resume line is started with it after a restart.

use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui::{AnyElement, Context, MouseButton, SystemNotification, Window, div, prelude::*};
use kynex_core::agent_panes::{self, AgentRule, AgentState, Tracker};
use kynex_core::attention::{self, Why};
use kynex_core::layout::{Direction, PaneId};
use kynex_core::terminal::{ShellKind, ShellSpec};
use kynex_core::tiling::{TermPane, Tool};
use kynex_core::toasts::Toast;
use kynex_core::user_commands;
use kynex_core::workspace::same_folder;

use super::KynexApp;
use crate::theme::theme;
use crate::ui::{Lamp, lamp, px};

/// How often agent screens are read.
const LOOK_EVERY: Duration = Duration::from_millis(500);

impl KynexApp {
    /// Reads the agent rules again, in the background.
    pub(super) fn refresh_agent_rules(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let (rules, errors) = cx.background_spawn(async { agent_panes::rules(&agent_panes::user_dir()) }).await;
            this.update(cx, |this, cx| {
                for e in errors {
                    eprintln!("kynex: {e}");
                }
                if this.agent_rules != rules {
                    this.agent_rules = rules;
                    // An agent pane that had no rule tries again.
                    let agents: Vec<PaneId> = this.tiles.values().flat_map(|t| t.agents()).map(|(p, _)| p).collect();
                    this.command_starting.retain(|p| !agents.contains(p));
                    this.rebuild_palette_if_open(cx);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// The agent pane `pane`'s state, as a lamp.
    pub(super) fn agent_lamp(&self, pane: PaneId) -> Lamp {
        match self.agent_trackers.get(&pane).map(Tracker::state).unwrap_or_default() {
            AgentState::Idle => Lamp::Idle,
            AgentState::Working => Lamp::Working,
            AgentState::NeedsYou => Lamp::NeedsYou,
            AgentState::Done => Lamp::Done,
        }
    }

    /// The agent panes of workspace `ws`, in any tab: (pane, agent's name, lamp).
    pub(super) fn agents_in(&self, ws: &Path) -> Vec<(PaneId, String, Lamp)> {
        let Some(t) = self.tiles.iter().find(|(f, _)| same_folder(f, ws)).map(|(_, t)| t) else { return Vec::new() };
        t.agents().into_iter().map(|(p, id)| (p, self.agent_name(&id), self.agent_lamp(p))).collect()
    }

    pub(super) fn agent_name(&self, id: &str) -> String {
        self.agent_rules.iter().find(|r| r.id == id).map_or_else(|| id.to_string(), |r| r.name.clone())
    }

    /// Starts agent `id` in a pane on the right of pane `target` (the session pane when `None`).
    pub(super) fn open_agent_pane(
        &mut self,
        id: String,
        target: Option<PaneId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((_, ws, _)) = self.tiling_scope() else {
            self.toast(Toast::error("Agent panes open in a project's Code view.".to_string()));
            cx.notify();
            return;
        };
        let Some(t) = self.tiles.get(&ws) else { return };
        let Some(target) = target.or_else(|| t.session_focus()) else { return };
        let pane = self.next_pane;
        let bounds = self.layout_bounds(cx);
        let tool = Tool::Terminal(TermPane { agent: Some(id.clone()), ..TermPane::default() });
        let Some(t) = self.tiles.get_mut(&ws) else { return };
        if !t.split_tool(target, Direction::Right, pane, tool, 0.5, bounds) {
            self.toast(Toast::error("No room to split here: the panes would get too small.".to_string()));
            cx.notify();
            return;
        }
        self.next_pane += 1;
        let panel = Self::new_term_panel(&ws, window, cx);
        self.term_panes.insert(pane, panel);
        self.start_agent(pane, id, None, ws, window, cx);
        self.show_focused_pane(window, cx);
        self.watch_agents(cx);
    }

    /// Agent pane `pane` from last time, drawn with nothing running: its agent again,
    /// resumed if it said how.
    pub(super) fn restart_agent_pane(&mut self, pane: PaneId, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, ws, _)) = self.tiling_scope() else { return };
        let Some(tp) = self.tiles.get(&ws).and_then(|t| t.term_pane(pane)).cloned() else { return };
        let Some(id) = tp.agent else { return };
        // The rules aren't read yet: next time it's drawn.
        if self.agent_rules.is_empty() || !self.command_starting.insert(pane) {
            return;
        }
        self.start_agent(pane, id, tp.resume, ws, window, cx);
        self.watch_agents(cx);
    }

    /// Finds agent `id`'s program (in the background) and runs it in pane `pane`'s panel.
    fn start_agent(
        &mut self,
        pane: PaneId,
        id: String,
        resume: Option<String>,
        ws: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(rule) = self.agent_rules.iter().find(|r| r.id == id).cloned() else {
            if let Some(p) = self.term_panes.get(&pane).cloned() {
                p.update(cx, |p, cx| p.show_error(format!("There's no agent \"{id}\" (Kynex's agents folder)."), cx));
            }
            return;
        };
        let session = self.focused_key().and_then(|k| self.session_by_key(k)).and_then(|s| s.session_id.clone());
        let spec = cx.background_spawn(async move { agent_spec(&rule, resume.as_deref(), &ws, session.as_deref()) });
        cx.spawn_in(window, async move |this, cx| {
            let spec = spec.await;
            this.update_in(cx, |this, window, cx| {
                let Some(panel) = this.term_panes.get(&pane).cloned() else { return };
                match spec {
                    Ok(spec) => {
                        this.command_starting.remove(&pane);
                        panel.update(cx, |p, cx| {
                            p.open_spec(spec, window, cx);
                        });
                    }
                    Err(e) => panel.update(cx, |p, cx| p.show_error(e, cx)),
                }
            })
            .ok();
        })
        .detach();
    }

    /// Reads agent screens every so often while there are agent panes.
    fn watch_agents(&mut self, cx: &mut Context<Self>) {
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

    /// Reads each agent pane's screen: its state, toasts for what's new, its resume id.
    /// False once there are no agent panes, which stops the watch.
    fn look_at_agents(&mut self, cx: &mut Context<Self>) -> bool {
        let scope = self.tiling_scope().and_then(|(_, ws, _)| self.tiles.get(&ws));
        let focus = scope.and_then(|t| t.focus());
        let in_sight = scope.map(|t| t.visible_panes()).unwrap_or_default();
        let mut panes: Vec<(PathBuf, PaneId, String)> = Vec::new();
        for (ws, t) in &self.tiles {
            panes.extend(t.agents().into_iter().map(|(p, id)| (ws.clone(), p, id)));
        }
        if panes.is_empty() {
            self.agent_watch = false;
            self.agent_trackers.clear();
            return false;
        }
        self.agent_trackers.retain(|p, _| panes.iter().any(|(_, q, _)| q == p));
        let mut changed = false;
        for (ws, pane, id) in panes {
            let Some(rule) = self.agent_rules.iter().find(|r| r.id == id).cloned() else { continue };
            let Some(screen) = self.term_panes.get(&pane).and_then(|p| p.read(cx).screen_text(cx)) else { continue };
            let looking = self.window_active && focus == Some(pane);
            let tracker = self.agent_trackers.entry(pane).or_default();
            let was = tracker.state();
            let now = tracker.update(agent_panes::detect(&rule, &screen), looking);
            if now != was {
                changed = true;
                // Like a session's: no toast for what's in front of you.
                let on_screen = self.window_active && in_sight.contains(&pane);
                if !on_screen {
                    self.tell_agent(pane, &rule.name, &ws, now, cx);
                }
            }
            if let Some(resume) = agent_panes::resume_id(&rule, &screen)
                && let Some(t) = self.tiles.get_mut(&ws)
            {
                t.set_resume(pane, resume);
            }
        }
        if changed {
            cx.notify();
        }
        true
    }

    /// An agent pane newly wants you: a toast (or a notification, from the background) and
    /// a sound, as a session's would.
    fn tell_agent(&mut self, pane: PaneId, name: &str, ws: &Path, now: AgentState, cx: &mut Context<Self>) {
        let why = match now {
            AgentState::NeedsYou => Why::Answer,
            AgentState::Done => Why::Finished,
            _ => return,
        };
        if !self.saved.notify {
            return;
        }
        let folder = ws.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if !self.window_active {
            cx.show_system_notification(SystemNotification {
                tag: format!("agent-{pane}").into(),
                title: name.to_string().into(),
                body: why.body().into(),
                actions: Vec::new(),
            });
        } else {
            let toast = if why == Why::Finished { Toast::success("") } else { Toast::warning("") };
            let toast = self.toast(toast.detail(format!("{folder} · {}", why.body())));
            toast.title = name.to_string();
        }
        if self.saved.sounds
            && let Some(sound) = attention::loudest([why])
        {
            kynex_core::sound::play(sound);
        }
    }

    /// An agent pane's body: a slim bar with its lamp and name over the terminal.
    pub(super) fn render_agent_pane(
        &mut self,
        pane: PaneId,
        id: &str,
        body: AnyElement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx).clone();
        let name = self.agent_name(id);
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap_1p5()
                    .h(px(26.))
                    .px_2()
                    .border_b_1()
                    .border_color(t.border_variant)
                    .text_sm()
                    .child(lamp(("agent-lamp", pane as usize), self.agent_lamp(pane), cx))
                    .child(div().text_color(t.text).child(name))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, ev: &gpui::MouseDownEvent, _, cx| {
                            this.open_menu(super::menus::Menu::Pane(pane), ev.position, gpui::Anchor::TopLeft, cx)
                        }),
                    ),
            )
            .child(div().flex_1().min_h_0().child(body))
            .into_any_element()
    }
}

/// How to start `rule` in `ws`, resumed with `resume` if given. Blocks on PATH.
fn agent_spec(rule: &AgentRule, resume: Option<&str>, ws: &Path, session: Option<&str>) -> Result<ShellSpec, String> {
    let argv = agent_panes::resume_argv(rule, resume);
    let program = user_commands::find_program(&argv[0], ws)?;
    let branch = kynex_core::git::status(ws).ok().and_then(|s| s.head.branch().map(str::to_string));
    Ok(ShellSpec {
        kind: ShellKind::default_shell(),
        program,
        args: argv[1..].to_vec(),
        env: user_commands::env(ws, branch.as_deref(), session),
        title: Some(rule.name.clone()),
    })
}
