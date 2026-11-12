//! The control socket (`kynex_core::control`): answering what `knx context | pane | session`
//! and scripts ask of the running app, and the events they can listen to. A prompt sent this
//! way is an ordinary message to the session, so it runs under the session's permission mode
//! and a permission prompt is still answered in the app.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use gpui::{Context, Window};
use kynex_core::control::Request;
use kynex_core::control::server::Call;
use kynex_core::layout::Axis;
use kynex_core::live::Key;
use kynex_core::session::{Item, Session};
use kynex_core::tiling::Content;
use kynex_core::workspace::same_folder;
use serde_json::{Value, json};

use super::KynexApp;
use super::state::Active;

/// A folder given on the command line (`knx <folder>`) when no running app took it: opened at start.
pub static OPEN_AT_START: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// How long a prompt that waits gives the session to start working (its history may be loading).
const START_WITHIN: Duration = Duration::from_secs(60);

/// A client waiting for a session's turn to end.
pub(super) struct Wait {
    session: Key,
    reply: Sender<Result<Value, String>>,
    /// The session has been seen working: only then does "not working" mean the turn ended.
    started: bool,
    since: Instant,
}

/// What each session was doing at the last look, to tell the listeners what changed.
pub(super) type Seen = HashMap<Key, (bool, bool)>;

type Answer = Result<Value, String>;

impl KynexApp {
    /// Answers every request that came in on the socket.
    pub(super) fn serve_control(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let calls = self.control.as_ref().map(|c| c.take()).unwrap_or_default();
        for Call { request, reply } in calls {
            match self.control_request(request, &reply, window, cx) {
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
                self.control_waits.push(Wait { reply, ..wait });
            }
        }
        cx.notify();
    }

    /// `Err(None)`: the answer is held back (see `control_wait_for`).
    fn control_request(
        &mut self,
        request: Request,
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
            Request::WorkspaceCreate { branch } => {
                self.create_workspace_for(branch, reply.clone(), cx);
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
                self.close_project(i);
                Ok(json!({ "closed": path }))
            }
            Request::PaneList => Ok(self.pane_list()),
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
                let s = self.session_by_key(session).ok_or_else(|| Some(no_session(session)))?;
                Ok(json!({ "text": last_said(s), "busy": s.busy, "waiting": s.waiting() }))
            }
            Request::SessionPrompt { session, text, wait } => {
                let s = self.all_sessions_mut().find(|s| s.uid == session).ok_or_else(|| Some(no_session(session)))?;
                s.send(text, Vec::new());
                // Working at once, or the message waits (the log is loading): then the next poll sees it start.
                let started = s.busy;
                if wait {
                    self.hold_for(session, started, cx);
                    return Err(None);
                }
                Ok(json!({ "sent": true }))
            }
            Request::SessionWait { session } => {
                let s = self.session_by_key(session).ok_or_else(|| Some(no_session(session)))?;
                if !s.busy {
                    return Ok(turn_state(s));
                }
                self.hold_for(session, true, cx);
                Err(None)
            }
            // Handled by the socket itself.
            Request::Events => Ok(json!({ "subscribed": true })),
        }
    }

    /// Makes the answer wait for `session`'s turn to end. `started`: it is working already.
    fn hold_for(&mut self, session: Key, started: bool, cx: &mut Context<Self>) {
        // `serve_control` puts the client's reply with it.
        let (reply, _) = std::sync::mpsc::channel();
        self.control_wait_for = Some(Wait { session, reply, started, since: Instant::now() });
        // A session that never starts working must not leave the client waiting for good.
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(START_WITHIN + Duration::from_secs(1)).await;
            this.update(cx, |this, cx| this.finish_control_waits(cx)).ok();
        })
        .detach();
    }

    /// Answers the waits whose turn ended, whose session needs you, whose session went away
    /// or never started. Called after every poll of the sessions.
    pub(super) fn finish_control_waits(&mut self, cx: &mut Context<Self>) {
        if self.control_waits.is_empty() {
            return;
        }
        let waits = std::mem::take(&mut self.control_waits);
        for mut w in waits {
            let answer: Option<Answer> = match self.session_by_key(w.session) {
                None => Some(Err(no_session(w.session))),
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
        for s in self.all_sessions() {
            let (was_busy, was_waiting) = self.control_seen.get(&s.uid).copied().unwrap_or((false, false));
            if was_busy && !s.busy {
                server.broadcast("turn_end", json!({ "session": s.uid, "state": turn_state(s)["state"] }));
            }
            if !was_waiting && s.waiting() {
                server.broadcast("needs_you", json!({ "session": s.uid }));
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

    fn pane_list(&self) -> Value {
        let Some((_, cwd, _)) = self.tiling_scope() else { return json!({ "panes": [] }) };
        let Some(t) = self.tiles.get(&cwd) else { return json!({ "panes": [] }) };
        let focus = t.focus();
        let panes: Vec<Value> = t
            .visible_panes()
            .into_iter()
            .map(|p| {
                let (kind, session) = match t.content(p) {
                    Some(Content::Session(k)) => ("session", Some(k)),
                    Some(Content::Terminal) => ("terminal", None),
                    Some(Content::Git) => ("git", None),
                    Some(Content::Timeline) => ("timeline", None),
                    Some(Content::Tree) => ("sessions", None),
                    None => ("unknown", None),
                };
                json!({ "pane": p, "kind": kind, "session": session, "focused": Some(p) == focus })
            })
            .collect();
        json!({ "workspace": cwd, "panes": panes })
    }

    fn focused_pane(&self) -> Option<u64> {
        let (_, cwd, _) = self.tiling_scope()?;
        self.tiles.get(&cwd)?.focus()
    }

    fn tiles_have(&self, pane: u64) -> bool {
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
