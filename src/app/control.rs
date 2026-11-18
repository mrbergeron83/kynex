//! The control socket (`kynex_core::control`): answering what `knx project | workspace | pane |
//! session | request` and scripts ask of the running app, and the events they can listen to.
//! What changes something waits for your OK on a card first (`control_cards.rs`). A prompt
//! sent this way is an ordinary message to the session, so it runs under the session's
//! permission mode and a permission prompt is still answered in the app.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use gpui::{Context, Window};
use kynex_core::control::Request;
use kynex_core::control::approval;
use kynex_core::control::protocol::Event;
use kynex_core::control::server::{Call, Gone};
use kynex_core::layout::{Axis, PaneId};
use kynex_core::live::Key;
use kynex_core::session::{Item, Session};
use kynex_core::workspace::same_folder;
use serde_json::{Value, json};

use super::KynexApp;
use super::control_sessions::NewSession;
use super::state::Active;

/// A folder given on the command line (`knx <folder>`) when no running app took it: opened at start.
pub static OPEN_AT_START: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// How long a prompt that waits gives the session to start working (its history may be loading).
pub(super) const START_WITHIN: Duration = Duration::from_secs(60);

/// A client waiting for a session's turn to end, or for a pane (`control_panes.rs`).
pub(super) struct Wait {
    pub(super) target: Target,
    reply: Sender<Result<Value, String>>,
    /// The client hung up: the wait is dropped at the next look.
    gone: Gone,
    /// It has been seen working: only then does "not working" mean the turn ended.
    pub(super) started: bool,
    pub(super) since: Instant,
    /// When it gives up (`--timeout`), and after how many seconds.
    deadline: Option<(Instant, u64)>,
}

/// What a [`Wait`] waits for.
pub(super) enum Target {
    /// A session's turn to end.
    Session(Key),
    /// A command run in shell pane `pane` to finish.
    Run { pane: PaneId, how: Run },
    /// Agent pane `pane` to stop working; it has `start_within` to start.
    Agent { pane: PaneId, start_within: Duration },
}

/// How a command run in a shell pane says it finished.
pub(super) enum Run {
    /// A hooked shell: once it's at a prompt after the `after`th, and stays there for a look
    /// (each line of a command of several lines has its own prompt). `first`: the number the
    /// shell gives the command (a later one if it numbers them and has a line before it).
    Hooked { after: u64, first: u64, settled: Option<u64> },
    /// A hooked shell still starting: `command` is typed at its first prompt.
    Pending { command: String },
    /// A shell without hooks: by the title of marker `n`, typed after it.
    Marked { n: u64 },
}

/// What each session was doing at the last look, to tell the listeners what changed.
pub(super) type Seen = HashMap<Key, (bool, bool)>;

pub(super) type Answer = Result<Value, String>;

impl KynexApp {
    /// Answers every request that came in on the socket; one that changes something puts up
    /// a card first (`control_cards.rs`).
    pub(super) fn serve_control(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let calls = self.control.as_ref().map(|c| c.take()).unwrap_or_default();
        for Call { request, reply, caller, gone } in calls {
            if gone.hung_up() {
                continue;
            }
            if approval::needs_card(&request) {
                self.ask_card(request, caller, reply, cx);
            } else {
                self.dispatch(request, caller, false, reply, gone, window, cx);
            }
        }
        cx.notify();
    }

    /// Does `request` from session `caller`; `reply` gets the answer, now or later.
    /// `worktree`: the card's "Start in a new worktree" was picked. `gone`: the client hung up,
    /// so a wait it asked for is dropped.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn dispatch(
        &mut self,
        request: Request,
        caller: Option<Key>,
        worktree: bool,
        reply: Sender<Answer>,
        gone: Gone,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.control_request(request, caller, worktree, &reply, window, cx) {
            // The answer comes when the turn ends.
            Err(None) => {}
            Ok(v) => {
                let _ = reply.send(Ok(v));
            }
            Err(Some(e)) => {
                let _ = reply.send(Err(e));
            }
        }
        if let Some(wait) = self.control_wait_for.take() {
            self.control_waits.push(Wait { reply, gone, ..wait });
        }
    }

    /// `Err(None)`: the answer is held back (see `control_wait_for`), or comes on `reply`.
    fn control_request(
        &mut self,
        request: Request,
        caller: Option<Key>,
        worktree: bool,
        reply: &Sender<Answer>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Value, Option<String>> {
        let said = |e: &str| Some(e.to_string());
        match request {
            Request::ProjectList => Ok(self.project_list()),
            Request::ProjectCreate { path } => {
                std::fs::create_dir_all(&path).map_err(|e| Some(format!("can't make {path}: {e}")))?;
                self.open_project_at(&path, window, cx).map_err(Some)
            }
            Request::ProjectOpen { path } => self.open_project_at(&path, window, cx).map_err(Some),
            Request::WorkspaceList => self.workspace_list(cx).map_err(Some),
            Request::WorkspaceOpen { path } => {
                let (project, target) = self.workspace_named(&path, cx).map_err(Some)?;
                let _ = project;
                self.select_workspace(&target, cx);
                window.activate_window();
                Ok(json!({ "opened": target }))
            }
            Request::WorkspaceCreate { branch, session, prompt, why } => {
                let start = session.then_some((prompt, why));
                self.create_workspace_for(branch, start, caller, reply.clone(), cx);
                Err(None)
            }
            Request::WorkspaceRemove { path, force } => {
                let (_, target) = self.workspace_named(&path, cx).map_err(Some)?;
                self.remove_workspace_for(target, force, reply.clone(), cx).map_err(Some)?;
                Err(None)
            }
            Request::ProjectRemove { path } => {
                let i = self.saved.projects.iter().position(|p| same_folder(&p.path, std::path::Path::new(&path)));
                let i = i.ok_or_else(|| Some(format!("{path} isn't open")))?;
                self.close_project(i, cx);
                Ok(json!({ "closed": path }))
            }
            Request::PaneList { all } => Ok(self.pane_list(all)),
            Request::PaneRead { pane, lines } => {
                self.start_pane(pane, window, cx);
                self.pane_read(pane, lines, cx).map_err(Some)
            }
            Request::PaneRun { pane, command, wait, timeout } => {
                self.pane_run(pane, &command, wait, timeout, window, cx)
            }
            Request::PanePrompt { pane, text, wait, timeout } => {
                self.start_pane(pane, window, cx);
                self.pane_prompt(pane, &text, wait, timeout, cx)
            }
            Request::PaneWait { pane, timeout } => {
                self.start_pane(pane, window, cx);
                self.pane_wait(pane, timeout, cx)
            }
            Request::PaneSplit { axis } => {
                let axis = match axis.as_deref() {
                    None | Some("row") => Axis::Row,
                    Some("column") => Axis::Column,
                    Some(other) => return Err(Some(format!("`{other}` isn't row or column"))),
                };
                let before = self.focused_pane();
                self.split_pane(axis, window, cx);
                match self.focused_pane() {
                    Some(p) if Some(p) != before => Ok(json!({ "pane": p, "session": self.pane_session_uid(p) })),
                    _ => Err(said("couldn't split: no room, or no session to split from")),
                }
            }
            Request::PaneFocus { pane } => {
                if !self.tiles_have(pane) {
                    return Err(Some(format!("no pane {pane} in the shown workspace")));
                }
                self.focus_pane(pane, window, cx);
                Ok(json!({ "pane": pane }))
            }
            Request::PaneMove { pane, to } => self.control_pane_move(pane, to, window, cx),
            Request::PaneSwap { pane, other } => self.control_pane_swap(pane, other, window, cx),
            Request::PaneResize { pane, dir, by } => self.control_pane_resize(pane, dir, by, window, cx),
            Request::PaneZoom { pane, off } => self.control_pane_zoom(pane, off, window, cx),
            Request::PaneClose { pane } => {
                let Some(pane) = pane.or_else(|| self.focused_pane()) else {
                    return Err(said("no pane to close"));
                };
                if !self.tiles_have(pane) {
                    return Err(Some(format!("no pane {pane} in the shown workspace")));
                }
                self.close_pane_id(pane, window, cx);
                Ok(json!({ "closed": pane }))
            }
            Request::SessionList => Ok(self.session_list()),
            Request::SessionRead { session } => {
                let key = self.resolve_session(&session).map_err(Some)?;
                let s = self.session_by_key(key).ok_or_else(|| Some(no_session(key)))?;
                Ok(json!({ "text": last_said(s), "busy": s.busy, "waiting": s.waiting() }))
            }
            Request::SessionPrompt { session, text, wait, timeout } => {
                let key = self.resolve_session(&session).map_err(Some)?;
                let s = self.session_for(key).ok_or_else(|| Some(no_session(key)))?;
                s.send(text, Vec::new());
                // Working at once, or the message waits (the log is loading): then the next poll sees it start.
                let started = s.busy;
                if wait {
                    self.hold(Target::Session(key), started, timeout, cx);
                    return Err(None);
                }
                Ok(json!({ "sent": true }))
            }
            Request::SessionWait { session, timeout } => {
                let key = self.resolve_session(&session).map_err(Some)?;
                let s = self.session_by_key(key).ok_or_else(|| Some(no_session(key)))?;
                if !s.busy {
                    return Ok(turn_state(s));
                }
                self.hold(Target::Session(key), true, timeout, cx);
                Err(None)
            }
            Request::SessionNew { workspace, prompt, title, pane, why } => {
                let new = NewSession { workspace, prompt, title, place: pane.unwrap_or_default(), why };
                self.control_session_new(new, caller, worktree, reply.clone(), window, cx).map_err(Some)?;
                Err(None)
            }
            Request::SessionFork { session, at, pane } => {
                self.control_session_fork(&session, at, pane.unwrap_or_default(), caller, reply.clone(), window, cx)
                    .map_err(Some)?;
                Err(None)
            }
            Request::SessionOpen { session, pane } => {
                self.control_session_open(session, pane.unwrap_or_default(), caller, window, cx).map_err(Some)
            }
            Request::SessionClose { session } => self.control_session_close(&session, window, cx).map_err(Some),
            Request::SessionRename { session, title } => self.control_session_rename(&session, &title).map_err(Some),
            Request::SessionArchive { session } => self.control_session_archive(&session).map_err(Some),
            Request::SessionPin { session, off } => self.control_session_pin(&session, off).map_err(Some),
            Request::Notify { title, body, pane, session } => {
                self.control_notify(&title, body, pane, session.as_ref(), cx).map_err(Some)
            }
            Request::RequestStatus { request } => self.approvals.status(request).map_err(Some),
            Request::RequestWait { request, timeout } => {
                self.request_wait(request, timeout, reply.clone(), cx);
                Err(None)
            }
            // Handled by the socket itself.
            Request::Events => Ok(json!({ "subscribed": true })),
        }
    }

    /// Makes the answer wait for `target`. `started`: it is working already. `timeout`: the
    /// seconds after which it gives up.
    pub(super) fn hold(&mut self, target: Target, started: bool, timeout: Option<u64>, cx: &mut Context<Self>) {
        let pane = !matches!(target, Target::Session(_));
        let now = Instant::now();
        let deadline = timeout.map(|s| (now + Duration::from_secs(s), s));
        // `serve_control` puts the client's reply with it.
        let (reply, _) = std::sync::mpsc::channel();
        self.control_wait_for = Some(Wait { target, reply, gone: Gone::default(), started, since: now, deadline });
        if pane {
            // Nothing else rings for panes: they are looked at every so often.
            self.watch_pane_waits(cx);
            return;
        }
        // A session that never starts working, or a timeout, must not leave the client
        // waiting for good: one look when either is due.
        for after in [Some(START_WITHIN), timeout.map(Duration::from_secs)].into_iter().flatten() {
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(after + Duration::from_millis(500)).await;
                this.update(cx, |this, cx| this.finish_control_waits(cx)).ok();
            })
            .detach();
        }
    }

    /// Answers the waits whose turn ended, whose session needs you, whose session went away
    /// or never started. Called after every poll of the sessions.
    pub(super) fn finish_control_waits(&mut self, cx: &mut Context<Self>) {
        self.drop_closed_cards();
        self.give_pending_titles();
        self.give_pending_parents();
        if self.control_waits.is_empty() {
            return;
        }
        let waits = std::mem::take(&mut self.control_waits);
        for mut w in waits {
            // Nobody to answer: Ctrl+C on `knx … --wait`.
            if w.gone.hung_up() {
                continue;
            }
            if let Some((at, secs)) = w.deadline
                && Instant::now() >= at
            {
                let _ = w.reply.send(Err(format!("timed out after {secs}s (it may still be running)")));
                continue;
            }
            let session = match w.target {
                Target::Session(k) => k,
                _ => {
                    match self.pane_wait_answer(&mut w, cx) {
                        Some(a) => {
                            let _ = w.reply.send(a);
                        }
                        None => self.control_waits.push(w),
                    }
                    continue;
                }
            };
            let answer: Option<Answer> = match self.session_by_key(session) {
                None => Some(Err(no_session(session))),
                Some(s) => {
                    w.started |= s.busy;
                    if s.waiting() || (w.started && !s.busy) {
                        Some(Ok(turn_state(s)))
                    } else if !w.started && w.since.elapsed() > START_WITHIN {
                        Some(Err("the session didn't start working".into()))
                    } else {
                        None
                    }
                }
            };
            match answer {
                Some(a) => {
                    let _ = w.reply.send(a);
                }
                None => self.control_waits.push(w),
            }
        }
        cx.notify();
    }

    /// Tells the listeners which sessions finished a turn or started needing you since the last look.
    pub(super) fn control_events(&mut self) {
        let Some(server) = &self.control else { return };
        let now: Seen = self.all_sessions().map(|s| (s.uid, (s.busy, s.waiting()))).collect();
        // Opened and closed since the last look, with who started them.
        for s in self.all_sessions().filter(|s| !self.control_seen.contains_key(&s.uid)) {
            let data = json!({ "session": s.uid, "id": s.session_id, "parent": self.parent_key(s.uid) });
            server.broadcast(Event::SessionCreated, data);
        }
        for gone in self.control_seen.keys().filter(|k| !now.contains_key(k)) {
            let parent = self.approvals.maker(&kynex_core::control::approval::Made::Session(*gone));
            server.broadcast(Event::SessionClosed, json!({ "session": gone, "parent": parent }));
        }
        for s in self.all_sessions() {
            let (was_busy, was_waiting) = self.control_seen.get(&s.uid).copied().unwrap_or((false, false));
            if was_busy && !s.busy {
                server.broadcast(Event::TurnEnd, json!({ "session": s.uid, "state": turn_state(s)["state"] }));
            }
            if !was_waiting && s.waiting() {
                server.broadcast(Event::NeedsYou, json!({ "session": s.uid }));
            }
        }
        self.control_seen = now;
    }

    fn project_list(&self) -> Value {
        let projects: Vec<Value> = self
            .saved
            .projects
            .iter()
            .enumerate()
            .map(|(i, p)| {
                json!({
                    "path": p.path,
                    "name": kynex_core::paths::project_name(&p.path),
                    "active": self.saved.active == Active::Project(i),
                })
            })
            .collect();
        json!({ "projects": projects })
    }

    /// Opens `path` as a project, or shows it if it is open already, and brings the window up.
    pub(super) fn open_project_at(
        &mut self,
        path: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Value, String> {
        let dir = PathBuf::from(path);
        if !dir.is_dir() {
            return Err(format!("{path} isn't a folder"));
        }
        match self.saved.projects.iter().position(|p| same_folder(&p.path, &dir)) {
            Some(i) => {
                self.select(Active::Project(i), window, cx);
            }
            None => self.add_project(dir),
        }
        window.activate_window();
        cx.notify();
        Ok(json!({ "opened": path }))
    }

    fn focused_pane(&self) -> Option<u64> {
        let (_, cwd, _) = self.tiling_scope()?;
        self.tiles.get(&cwd)?.focus()
    }

    pub(super) fn tiles_have(&self, pane: u64) -> bool {
        self.tiling_scope().and_then(|(_, cwd, _)| self.tiles.get(&cwd)).is_some_and(|t| t.has(pane))
    }

    fn pane_session_uid(&self, pane: u64) -> Option<Key> {
        let (_, cwd, _) = self.tiling_scope()?;
        self.tiles.get(&cwd)?.key_of(pane)
    }

    fn session_list(&self) -> Value {
        let sessions: Vec<Value> = self
            .all_sessions()
            .map(|s| {
                json!({
                    "session": s.uid,
                    "id": s.session_id,
                    "cwd": s.cwd,
                    "busy": s.busy,
                    "waiting": s.waiting(),
                    "new": s.is_new(),
                })
            })
            .collect();
        json!({ "sessions": sessions })
    }
}

fn no_session(session: Key) -> String {
    format!("no open session {session}")
}

/// What the session last said.
fn last_said(s: &Session) -> String {
    s.items
        .iter()
        .rev()
        .find_map(|i| if let Item::Assistant(t) = i { Some(t.clone()) } else { None })
        .unwrap_or_default()
}

/// How a turn stands: done, or stopped on a prompt for you (`waiting`); with what was said.
fn turn_state(s: &Session) -> Value {
    let state = if s.waiting() {
        "waiting"
    } else if s.busy {
        "working"
    } else {
        "done"
    };
    json!({ "state": state, "text": last_said(s) })
}
