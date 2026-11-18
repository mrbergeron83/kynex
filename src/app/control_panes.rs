//! The control socket's pane commands for terminal and agent-CLI panes: `knx pane list | read |
//! run | prompt | wait`. Panes are found in every workspace shown since Kynex started, not only
//! the shown one, so an agent can drive a pane in the background; one that was never drawn has
//! its program started first.

use std::time::Duration;

use gpui::{Context, Entity, Window};
use kynex_core::agent_panes::AgentState;
use kynex_core::control::protocol::Event;
use kynex_core::layout::PaneId;
use kynex_core::terminal::marker;
use kynex_core::tiling::{Content, Tiling};
use serde_json::{Value, json};

use super::KynexApp;
use super::control::{Answer, Run, Target, Wait};
use crate::terminal::TerminalView;

/// How often waits on panes are looked at.
const LOOK_EVERY: Duration = Duration::from_millis(300);

/// How long an agent pane that already asked something before a message keeps asking before
/// a wait counts it as blocked.
const STILL_ASKING: Duration = Duration::from_secs(3);

/// How long a hooked shell may take to show its first prompt before a command waiting for it
/// is typed with a marker instead.
const FIRST_PROMPT_WITHIN: Duration = Duration::from_secs(20);

/// The most lines of a command's output an answer carries (the last ones).
const OUTPUT_LINES: usize = 2000;

/// What runs in a terminal pane.
enum Runs {
    Shell,
    Command,
    /// An agent CLI, by rule id.
    Agent(String),
}

impl KynexApp {
    /// `knx pane list`: the shown tab's panes, or with `all` every pane of every workspace
    /// shown since Kynex started.
    pub(super) fn pane_list(&self, all: bool) -> Value {
        let shown = self.tiling_scope().map(|(_, ws, _)| ws);
        if !all {
            let Some(ws) = shown else { return json!({ "panes": [] }) };
            let Some(t) = self.tiles.get(&ws) else { return json!({ "panes": [] }) };
            let panes: Vec<Value> = t.visible_panes().into_iter().map(|p| pane_entry(t, p)).collect();
            return json!({ "workspace": ws, "panes": panes });
        }
        let mut workspaces: Vec<_> = self.tiles.iter().collect();
        workspaces.sort_by(|a, b| a.0.cmp(b.0));
        let mut panes = Vec::new();
        for (ws, t) in workspaces {
            let visible = t.visible_panes();
            for p in t.panes() {
                let mut v = pane_entry(t, p);
                v["workspace"] = json!(ws);
                v["shown"] = json!(shown.as_ref() == Some(ws) && visible.contains(&p));
                panes.push(v);
            }
        }
        json!({ "panes": panes })
    }

    /// Starts terminal pane `pane`'s program if it was never drawn (a tab not shown since
    /// Kynex started). Nothing for other panes.
    pub(super) fn start_pane(&mut self, pane: PaneId, window: &mut Window, cx: &mut Context<Self>) {
        if self.term_panes.contains_key(&pane) {
            return;
        }
        let ws = self.tiles.iter().find(|(_, t)| t.term_pane(pane).is_some()).map(|(ws, _)| ws.clone());
        if let Some(ws) = ws {
            // An agent's starts its watch too.
            self.start_terminal_pane(pane, &ws, window, cx);
        }
    }

    /// `knx pane read`: the screen, or the last `lines` of scrollback and screen.
    pub(super) fn pane_read(&self, pane: PaneId, lines: Option<usize>, cx: &Context<Self>) -> Result<Value, String> {
        let (view, _) = self.terminal_of(pane, cx)?;
        let view = view.read(cx);
        let text = match lines {
            Some(n) => view.last_lines(n),
            None => view.screen_text(),
        };
        let text = text.ok_or_else(|| format!("pane {pane} is starting: try again in a moment"))?;
        Ok(json!({ "pane": pane, "text": text.trim_end() }))
    }

    /// `knx pane run`: types a command into a shell pane (a new one with no `pane`); with
    /// `wait`, answers with its exit code and what it printed.
    pub(super) fn pane_run(
        &mut self,
        pane: Option<PaneId>,
        command: &str,
        wait: bool,
        timeout: Option<u64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Value, Option<String>> {
        let pane = match pane {
            Some(p) => {
                self.start_pane(p, window, cx);
                p
            }
            None => self.new_terminal_pane(window, cx).map_err(Some)?,
        };
        let (view, runs) = self.terminal_of(pane, cx).map_err(Some)?;
        match runs {
            Runs::Shell => {}
            Runs::Agent(id) => {
                return Err(Some(format!("pane {pane} runs {}: use `knx pane prompt {pane}`", self.agent_name(&id))));
            }
            Runs::Command => return Err(Some(format!("pane {pane} runs a custom command, not a shell"))),
        }
        if !wait {
            view.update(cx, |v, cx| v.run(command, cx));
            return Ok(json!({ "pane": pane, "sent": true }));
        }
        // A shell that exited starts again; its marks start over with it.
        if view.read(cx).exited() {
            view.update(cx, |v, cx| v.restart(cx));
        }
        let (marks, hooks) = view.read_with(cx, |v, _| (v.marks(), v.has_hooks()));
        let how = if marks.hooked {
            if let Some(n) = marks.running {
                return Err(Some(format!("pane {pane} is busy: its command {n} is still running")));
            }
            view.update(cx, |v, cx| v.run(command, cx));
            hooked_run(marks)
        } else if hooks {
            Run::Pending { command: command.to_string() }
        } else {
            self.marked_run(&view, pane, command, cx)?
        };
        self.hold(Target::Run { pane, how }, true, timeout, cx);
        Err(None)
    }

    /// Types `command` with a marker, for a shell without the hooks.
    fn marked_run(
        &mut self,
        view: &Entity<TerminalView>,
        pane: PaneId,
        command: &str,
        cx: &mut Context<Self>,
    ) -> Result<Run, Option<String>> {
        self.control_marker += 1;
        let n = self.control_marker;
        if !view.update(cx, |v, cx| v.run_marked(command, n, cx)) {
            let shell = view.read(cx).shell.program.display().to_string();
            return Err(Some(format!(
                "--wait doesn't know when a command ends in {shell}; run it without (pane {pane})"
            )));
        }
        Ok(Run::Marked { n })
    }

    /// A new terminal pane below everything in the shown tab, with a shell starting; the focus
    /// stays where it was.
    fn new_terminal_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Result<PaneId, String> {
        let (_, ws, _) = self.tiling_scope().ok_or("open a project first")?;
        let id = self.next_pane;
        let ratio = self.terminal_ratio(window);
        let bounds = self.layout_bounds(cx);
        let t = self.tiles.get_mut(&ws).ok_or("open a project first")?;
        let focus = t.focus();
        if !t.add_terminal(id, ratio, bounds) {
            return Err("no room for a terminal pane in the shown tab".into());
        }
        if let Some(f) = focus {
            t.focus_pane(f);
        }
        self.next_pane += 1;
        // A pane of its own gets a shell of its own, not the folder's hidden ones.
        let panel = Self::new_term_panel(&ws, window, cx);
        self.term_panes.insert(id, panel.clone());
        panel.update(cx, |p, cx| {
            p.set_pane(id);
            p.ensure_open(window, cx);
        });
        cx.notify();
        Ok(id)
    }

    /// `knx pane prompt`: types a message into an agent pane and sends it with the agent's
    /// key; with `wait`, answers when it's done or blocked.
    pub(super) fn pane_prompt(
        &mut self,
        pane: PaneId,
        text: &str,
        wait: bool,
        timeout: Option<u64>,
        cx: &mut Context<Self>,
    ) -> Result<Value, Option<String>> {
        let (view, id) = self.agent_terminal(pane, cx).map_err(Some)?;
        let rule = self.agent_rules.iter().find(|r| r.id == id).cloned();
        if !view.read(cx).paste(text) {
            return Err(Some(format!("{} in pane {pane} is starting: try again in a moment", self.agent_name(&id))));
        }
        let (key, after) =
            rule.as_ref().map_or((b"\r".to_vec(), Duration::from_millis(80)), |r| (r.submit_bytes(), r.submit_after()));
        cx.spawn(async move |_, cx| {
            cx.background_executor().timer(after).await;
            view.update(cx, |v, _| v.write(key));
        })
        .detach();
        if wait {
            let start_within = rule.map_or(Duration::from_secs(120), |r| r.start_within());
            // Not started, whatever it shows now: that's from before the message.
            self.hold(Target::Agent { pane, start_within }, false, timeout, cx);
            return Err(None);
        }
        Ok(json!({ "pane": pane, "sent": true }))
    }

    /// `knx pane wait`: answers when an agent pane isn't working any more.
    pub(super) fn pane_wait(
        &mut self,
        pane: PaneId,
        timeout: Option<u64>,
        cx: &mut Context<Self>,
    ) -> Result<Value, Option<String>> {
        let (view, _) = self.agent_terminal(pane, cx).map_err(Some)?;
        match self.agent_state(pane) {
            AgentState::Working => {
                self.hold(Target::Agent { pane, start_within: Duration::ZERO }, true, timeout, cx);
                Err(None)
            }
            AgentState::NeedsYou => Ok(agent_answer(pane, "blocked", &view, cx)),
            AgentState::Idle | AgentState::Done => Ok(agent_answer(pane, "done", &view, cx)),
        }
    }

    /// The answer to a wait on a pane, once there is one. Called by `finish_control_waits`.
    pub(super) fn pane_wait_answer(&mut self, w: &mut Wait, cx: &mut Context<Self>) -> Option<Answer> {
        match &mut w.target {
            Target::Session(_) => None,
            Target::Run { pane, how } => {
                let pane = *pane;
                let Ok((view, _)) = self.terminal_of(pane, cx) else {
                    return Some(Err(format!("pane {pane} closed")));
                };
                if view.read(cx).exited() {
                    return Some(Err(format!("the shell in pane {pane} exited")));
                }
                match how {
                    Run::Pending { command } => {
                        let marks = view.read(cx).marks();
                        if marks.hooked {
                            let command = std::mem::take(command);
                            view.update(cx, |v, cx| v.run(&command, cx));
                            *how = hooked_run(marks);
                        } else if w.since.elapsed() > FIRST_PROMPT_WITHIN {
                            // Its hooks never spoke (a profile that replaces them): the marker.
                            let command = std::mem::take(command);
                            match self.marked_run(&view, pane, &command, cx) {
                                Ok(run) => *how = run,
                                Err(e) => return Some(Err(e.unwrap_or_default())),
                            }
                        }
                        None
                    }
                    Run::Hooked { after, first, settled } => {
                        let v = view.read(cx);
                        let marks = v.marks();
                        let (n, code) = marks.last.unwrap_or((0, 0));
                        // Not yet at a prompt after it, or (a shell that numbers commands) the
                        // prompt is after an empty Enter, not the command.
                        if marks.prompts <= *after || marks.running.is_some() || (n > 0 && n < *first) {
                            *settled = None;
                            return None;
                        }
                        // The next line of a command of several starts within a look.
                        if *settled != Some(marks.prompts) {
                            *settled = Some(marks.prompts);
                            return None;
                        }
                        // A shell that numbers its commands links their output: none is "".
                        let output = (n > 0).then(|| {
                            let outputs = (*first..=n).filter_map(|i| v.output(i));
                            outputs.filter(|o| !o.trim().is_empty()).collect::<Vec<_>>().join("\n")
                        });
                        Some(Ok(run_answer(pane, code, output)))
                    }
                    Run::Marked { n } => {
                        let v = view.read(cx);
                        let code = v.take_done(*n)?;
                        Some(Ok(run_answer(pane, code, Some(v.wrapped_output(*n).unwrap_or_default()))))
                    }
                }
            }
            Target::Agent { pane, start_within } => {
                let (pane, start_within) = (*pane, *start_within);
                let Ok((view, id)) = self.agent_terminal(pane, cx) else {
                    return Some(Err(format!("pane {pane} closed")));
                };
                let state = self.agent_state(pane);
                w.started |= state == AgentState::Working;
                // Asking something before it started on the message is asking it of the one before:
                // blocked only once it took the message, or kept asking a while.
                if state == AgentState::NeedsYou && (w.started || w.since.elapsed() > STILL_ASKING) {
                    Some(Ok(agent_answer(pane, "blocked", &view, cx)))
                } else if w.started && state != AgentState::Working {
                    Some(Ok(agent_answer(pane, "done", &view, cx)))
                } else if !w.started && w.since.elapsed() > start_within {
                    let name = self.agent_name(&id);
                    Some(Err(format!("{name} in pane {pane} didn't start working within {}s", start_within.as_secs())))
                } else {
                    None
                }
            }
        }
    }

    /// Looks at the waits every so often while one waits on a pane: nothing else rings for them.
    pub(super) fn watch_pane_waits(&mut self, cx: &mut Context<Self>) {
        if self.pane_wait_watch {
            return;
        }
        self.pane_wait_watch = true;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(LOOK_EVERY).await;
                let more = this
                    .update(cx, |this, cx| {
                        this.finish_control_waits(cx);
                        let more = this.control_waits.iter().any(|w| !matches!(w.target, Target::Session(_)));
                        this.pane_wait_watch = more;
                        more
                    })
                    .unwrap_or(false);
                if !more {
                    break;
                }
            }
        })
        .detach();
    }

    /// Tells the listeners an agent pane finished a turn or wants you, as for sessions.
    pub(super) fn agent_event(&self, pane: PaneId, id: &str, was: AgentState, now: AgentState) {
        let Some(server) = &self.control else { return };
        match now {
            AgentState::NeedsYou => server.broadcast(Event::NeedsYou, json!({ "pane": pane, "agent": id })),
            AgentState::Done | AgentState::Idle if was == AgentState::Working => {
                server.broadcast(Event::TurnEnd, json!({ "pane": pane, "agent": id, "state": "done" }));
            }
            _ => {}
        }
    }

    fn agent_state(&self, pane: PaneId) -> AgentState {
        self.agent_trackers.get(&pane).map(|t| t.state()).unwrap_or_default()
    }

    /// An agent pane's terminal and its agent's rule id.
    fn agent_terminal(&self, pane: PaneId, cx: &Context<Self>) -> Result<(Entity<TerminalView>, String), String> {
        match self.terminal_of(pane, cx)? {
            (view, Runs::Agent(id)) => Ok((view, id)),
            (_, Runs::Shell) => Err(format!("pane {pane} is a shell: use `knx pane run {pane}`")),
            (_, Runs::Command) => Err(format!("pane {pane} runs a custom command, not an agent")),
        }
    }

    /// Pane `pane`'s shown terminal tab, in any workspace, and what runs in it.
    fn terminal_of(&self, pane: PaneId, cx: &Context<Self>) -> Result<(Entity<TerminalView>, Runs), String> {
        let tiles = self.tiles.values().find(|t| t.has(pane)).ok_or_else(|| format!("no pane {pane}"))?;
        let tp = match tiles.content(pane) {
            Some(Content::Terminal) => tiles.term_pane(pane).ok_or_else(|| format!("no pane {pane}"))?,
            Some(Content::Session(k)) => {
                return Err(format!("pane {pane} is session {k}: use `knx session read | prompt | wait {k}`"));
            }
            _ => return Err(format!("pane {pane} isn't a terminal or agent pane")),
        };
        let runs = match (&tp.agent, &tp.command) {
            (Some(id), _) => Runs::Agent(id.clone()),
            (None, Some(_)) => Runs::Command,
            (None, None) => Runs::Shell,
        };
        let view = self
            .term_panes
            .get(&pane)
            .and_then(|p| p.read(cx).active_view())
            .ok_or_else(|| format!("pane {pane} is starting: try again in a moment"))?;
        Ok((view, runs))
    }
}

/// How a command typed now into a hooked shell whose marks are `marks` says it finished.
fn hooked_run(marks: kynex_core::terminal::ShellMarks) -> Run {
    let last = marks.last.map_or(0, |(n, _)| n);
    Run::Hooked { after: marks.prompts, first: last + 1, settled: None }
}

/// A pane as `knx pane list` gives it.
fn pane_entry(t: &Tiling, p: PaneId) -> Value {
    let agent = t.term_pane(p).and_then(|tp| tp.agent.clone());
    let (kind, session) = match t.content(p) {
        Some(Content::Session(k)) => ("session", Some(k)),
        Some(Content::Terminal) if agent.is_some() => ("agent", None),
        Some(Content::Terminal) => ("terminal", None),
        Some(Content::Git) => ("git", None),
        Some(Content::Timeline) => ("timeline", None),
        Some(Content::Tree) => ("sessions", None),
        Some(Content::Agents) => ("agents", None),
        None => ("unknown", None),
    };
    let mut v = json!({ "pane": p, "kind": kind, "session": session, "focused": Some(p) == t.focus() });
    if let Some(id) = agent {
        v["agent"] = json!(id);
    }
    v
}

/// What a finished `pane run --wait` answers: its exit code and what it printed (the last
/// [`OUTPUT_LINES`]; `null` if the shell doesn't say).
fn run_answer(pane: PaneId, code: i32, output: Option<String>) -> Value {
    let mut v = json!({ "pane": pane, "exit": code, "output": null });
    if let Some(out) = output {
        let lines = out.trim_end().lines().count();
        v["output"] = json!(marker::tail(&out, OUTPUT_LINES));
        if lines > OUTPUT_LINES {
            v["truncated"] = json!(true);
        }
    }
    v
}

/// How an agent pane's turn stands, with its screen.
fn agent_answer(pane: PaneId, state: &str, view: &Entity<TerminalView>, cx: &Context<KynexApp>) -> Value {
    json!({ "pane": pane, "state": state, "text": view.read(cx).screen_text().unwrap_or_default().trim_end() })
}
