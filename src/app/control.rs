//! The control socket (`kynex_core::control`): answering what `knx project | workspace | pane |
//! session | hook` and scripts ask of the running app, and the events they can listen to.

use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use gpui::{Context, Window};
use kynex_core::control::Request;
use kynex_core::control::server::{Call, Gone};
use kynex_core::hooks::HookReport;
use kynex_core::layout::{Axis, PaneId};
use kynex_core::workspace::same_folder;
use serde_json::{Value, json};

use super::KynexApp;
use super::control_sessions::NewSession;

/// A folder given on the command line (`knx <folder>`) when no running app took it: opened at start.
pub static OPEN_AT_START: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// A client waiting for a command or agent pane turn (`control_panes.rs`).
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
    /// A command run in shell pane `pane` to finish.
    Run { pane: PaneId, how: Run },
    /// Agent pane `pane` to stop working; it has `start_within` to start.
    Agent { pane: PaneId, start_within: Duration },
}

/// How a command run in a shell pane says it finished.
pub(super) enum Run {
    /// A hooked shell: once it's at a prompt after the `after`th, and stays there for a look.
    Hooked { after: u64, first: u64, settled: Option<u64> },
    /// A hooked shell still starting: `command` is typed at its first prompt.
    Pending { command: String },
    /// A shell without hooks: by the title of marker `n`, typed after it.
    Marked { n: u64 },
}

pub(super) type Answer = Result<Value, String>;

impl KynexApp {
    /// Answers every request that came in on the socket.
    pub(super) fn serve_control(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let calls = self.control.as_ref().map(|c| c.take()).unwrap_or_default();
        for Call { request, reply, caller, gone } in calls {
            if gone.hung_up() {
                continue;
            }
            self.dispatch(request, caller, reply, gone, window, cx);
        }
        cx.notify();
    }

    fn dispatch(
        &mut self,
        request: Request,
        caller: Option<u64>,
        reply: Sender<Answer>,
        gone: Gone,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.control_request(request, caller, &reply, window, cx) {
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
        caller: Option<u64>,
        reply: &Sender<Answer>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Value, Option<String>> {
        let said = |e: &str| Some(e.to_string());
        match request {
            Request::Hook { pane, provider, state, session_id, detail, subagent, background } => {
                let report = HookReport { state, session_id, detail, subagent, background };
                self.handle_hook_event(pane, &provider, report, cx);
                Ok(json!({ "ok": true }))
            }
            Request::ProjectList => Ok(self.project_list()),
            Request::ProjectCreate { path } => {
                std::fs::create_dir_all(&path).map_err(|e| Some(format!("can't make {path}: {e}")))?;
                self.open_project_at(&path, window, cx).map_err(Some)
            }
            Request::ProjectOpen { path } => self.open_project_at(&path, window, cx).map_err(Some),
            Request::ProjectRemove { path } => {
                let i = self.saved.projects.iter().position(|p| same_folder(&p.path, std::path::Path::new(&path)));
                let i = i.ok_or_else(|| Some(format!("{path} isn't open")))?;
                self.close_project(i, cx);
                Ok(json!({ "closed": path }))
            }
            Request::WorkspaceList => self.workspace_list(cx).map_err(Some),
            Request::WorkspaceOpen { path } => {
                let (_, target) = self.workspace_named(&path, cx).map_err(Some)?;
                self.select_workspace(&target, cx);
                window.activate_window();
                Ok(json!({ "opened": target }))
            }
            Request::WorkspaceCreate { branch, provider } => {
                self.create_workspace_for(branch, provider, caller, reply.clone(), window, cx);
                Err(None)
            }
            Request::WorkspaceRemove { path, force } => {
                let (_, target) = self.workspace_named(&path, cx).map_err(Some)?;
                self.remove_workspace_for(target, force, reply.clone(), cx).map_err(Some)?;
                Err(None)
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
                let before = self.focused_pane_id();
                self.split_pane(axis, window, cx);
                match self.focused_pane_id() {
                    Some(p) if Some(p) != before => Ok(json!({ "pane": p })),
                    _ => Err(said("couldn't split: no room, or no workspace open")),
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
                let Some(pane) = pane.or_else(|| self.focused_pane_id()) else {
                    return Err(said("no pane to close"));
                };
                if !self.tiles_have(pane) {
                    return Err(Some(format!("no pane {pane} in the shown workspace")));
                }
                self.close_pane_id(pane, window, cx);
                Ok(json!({ "closed": pane }))
            }
            Request::SessionList => Ok(self.session_list()),
            Request::SessionNew { workspace, provider, model, pane } => {
                let new = NewSession { workspace, provider, model, place: pane.unwrap_or_default() };
                self.control_session_new(new, caller, window, cx).map_err(Some)
            }
            Request::SessionOpen { session, pane } => {
                self.control_session_open(session, pane.unwrap_or_default(), caller, window, cx).map_err(Some)
            }
            Request::SessionRename { session, title } => self.control_session_rename(&session, &title).map_err(Some),
            Request::SessionArchive { session } => self.control_session_archive(&session).map_err(Some),
            Request::SessionPin { session, off } => self.control_session_pin(&session, off).map_err(Some),
            // Handled by the socket itself.
            Request::Events => Ok(json!({ "subscribed": true })),
        }
    }

    /// Makes the answer wait for `target`. `started`: it is working already. `timeout`: the
    /// seconds after which it gives up.
    pub(super) fn hold(&mut self, target: Target, started: bool, timeout: Option<u64>, cx: &mut Context<Self>) {
        let now = Instant::now();
        let deadline = timeout.map(|s| (now + Duration::from_secs(s), s));
        let (reply, _) = std::sync::mpsc::channel();
        self.control_wait_for = Some(Wait { target, reply, gone: Gone::default(), started, since: now, deadline });
        self.watch_pane_waits(cx);
    }

    /// Answers the waits whose command or agent turn ended, or whose deadline passed.
    pub(super) fn finish_control_waits(&mut self, cx: &mut Context<Self>) {
        if self.control_waits.is_empty() {
            return;
        }
        let waits = std::mem::take(&mut self.control_waits);
        for mut w in waits {
            if w.gone.hung_up() {
                continue;
            }
            if let Some((at, secs)) = w.deadline
                && Instant::now() >= at
            {
                let _ = w.reply.send(Err(format!("timed out after {secs}s (it may still be running)")));
                continue;
            }
            match self.pane_wait_answer(&mut w, cx) {
                Some(a) => {
                    let _ = w.reply.send(a);
                }
                None => self.control_waits.push(w),
            }
        }
        cx.notify();
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
                    "active": self.saved.active_project == Some(i),
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
                self.select_project(i, window, cx);
            }
            None => self.add_project(dir),
        }
        window.activate_window();
        cx.notify();
        Ok(json!({ "opened": path }))
    }

    pub(super) fn tiles_have(&self, pane: u64) -> bool {
        self.tiling_scope().and_then(|(_, cwd)| self.tiles.get(&cwd)).is_some_and(|t| t.has(pane))
    }

    fn session_list(&self) -> Value {
        let mut sessions: Vec<(PathBuf, u64, Value)> = self
            .agent_instances
            .values()
            .map(|inst| {
                let state = inst.tracker.state();
                let detail = (!inst.tracker.detail().is_empty()).then(|| inst.tracker.detail().to_string());
                (
                    inst.cwd.clone(),
                    inst.id,
                    json!({
                        "instance": inst.id,
                        "pane": self.pane_showing_agent(inst.id),
                        "provider": inst.meta.provider,
                        "id": inst.meta.session_id,
                        "model": inst.meta.model,
                        "title": self.agent_title(&inst.cwd, &inst.meta),
                        "cwd": inst.cwd,
                        "state": format!("{state:?}").to_lowercase(),
                        "detail": detail,
                    }),
                )
            })
            .chain(self.claude_chats.values().map(|chat| {
                let state = chat.state;
                let detail = (!chat.detail.is_empty()).then(|| chat.detail.clone());
                (
                    chat.cwd.clone(),
                    chat.id,
                    json!({
                        "instance": chat.id,
                        "pane": self.pane_showing_agent(chat.id),
                        "provider": chat.meta.provider,
                        "id": chat.meta.session_id,
                        "model": chat.meta.model,
                        "title": self.agent_title(&chat.cwd, &chat.meta),
                        "cwd": chat.cwd,
                        "state": format!("{state:?}").to_lowercase(),
                        "detail": detail,
                    }),
                )
            }))
            .collect();
        sessions.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
        let sessions: Vec<Value> = sessions.into_iter().map(|(_, _, v)| v).collect();
        json!({ "sessions": sessions })
    }
}
