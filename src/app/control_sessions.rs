//! The control socket's session commands, done once their card was answered: a new session
//! (in a worktree of its own if you picked that), a fork, a past one opened, and closing,
//! renaming, archiving and pinning. Sessions made this way don't take the focus: they open in
//! a pane beside the one that asked (or the focused one), in a new tab, or only in the list.

use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

use gpui::{AppContext, Context, Window};
use kynex_core::control::approval::Made;
use kynex_core::control::protocol::{Place, SessionRef};
use kynex_core::git;
use kynex_core::layout::{Direction, PaneId};
use kynex_core::live::Key;
use kynex_core::provider::Provider;
use kynex_core::rewind::{self, Point};
use kynex_core::session::{Kind, Session};
use kynex_core::workspace::{self, same_folder};
use serde_json::{Value, json};

use super::KynexApp;

type Answer = Result<Value, String>;

/// What `session new` asks for.
pub(super) struct NewSession {
    pub workspace: Option<String>,
    pub prompt: Option<String>,
    pub title: Option<String>,
    pub place: Place,
}

impl KynexApp {
    /// The open session `r` names, by uid or CLI id.
    pub(super) fn resolve_session(&self, r: &SessionRef) -> Result<Key, String> {
        self.all_sessions()
            .find(|s| r.names(s.uid, s.session_id.as_deref()))
            .map(|s| s.uid)
            .ok_or_else(|| format!("no open session {r}"))
    }

    /// The project holding open session `key`.
    fn project_of(&self, key: Key) -> Option<PathBuf> {
        self.code.iter().find(|(_, l)| l.sessions.iter().any(|s| s.uid == key)).map(|(p, _)| p.clone())
    }

    /// The project a request is about: the asking session's, else the shown one.
    pub(super) fn request_project(&self, caller: Option<Key>) -> Result<PathBuf, String> {
        caller
            .and_then(|c| self.project_of(c))
            .or_else(|| self.workspace_project())
            .ok_or_else(|| "no project is shown".to_string())
    }

    /// Whether `project` is open and may start one more session (checked again after any
    /// wait, since other sessions may have started meanwhile).
    fn room_in(&self, project: &Path) -> Result<(), String> {
        self.code.get(project).ok_or_else(|| format!("{} isn't open", project.display()))?.room_for_new()
    }

    /// `session new`: checks what it can at once; `reply` gets the session.
    pub(super) fn control_session_new(
        &mut self,
        new: NewSession,
        caller: Option<Key>,
        worktree: bool,
        reply: Sender<Answer>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let project = self.request_project(caller)?;
        if new.place == Place::Hidden && new.prompt.is_none() {
            return Err("a session that isn't shown (`--pane none`) needs a --prompt".into());
        }
        let folder = match &new.workspace {
            Some(w) => {
                let wanted = PathBuf::from(w);
                let found = self.workspaces_of(&project, cx).into_iter().find(|(e, _)| e.at(&wanted));
                found.map(|(e, _)| e.path).ok_or_else(|| format!("{w} isn't a workspace of {}", project.display()))?
            }
            None => caller
                .and_then(|c| self.session_by_key(c))
                .map(|s| s.cwd.clone())
                .or_else(|| self.tiling_scope().filter(|(p, _, _)| *p == project).map(|(_, ws, _)| ws))
                .unwrap_or_else(|| project.clone()),
        };
        self.room_in(&project)?;
        if !worktree {
            let _ = reply.send(self.start_control_session(&project, folder, new, caller));
            return Ok(());
        }
        // A worktree of its own, on a new branch named for it.
        let about = new.title.clone().or_else(|| new.prompt.clone()).unwrap_or_default();
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let branch = workspace::branch_for(&about, stamp);
        cx.spawn(async move |this, cx| {
            let root = project.clone();
            let made = cx
                .background_spawn(async move {
                    let base = git::default_branch(&root);
                    workspace::create(&root, &branch, base.as_deref())
                })
                .await;
            this.update(cx, |this, cx| {
                let answer = made.and_then(|path| {
                    this.refresh_worktrees([&project], cx);
                    this.approvals.record_made(Made::Workspace(path.clone()), caller);
                    // A workspace never shown has no panes. With a prompt the session works
                    // there, listed; without one it would be tidied away, so the workspace comes up.
                    if new.prompt.is_some() {
                        let hidden = NewSession { place: Place::Hidden, ..new };
                        return this.start_control_session(&project, path, hidden, caller);
                    }
                    let shown = this.workspace_project().as_deref() == Some(project.as_path());
                    if shown {
                        this.select_workspace(&path, cx);
                    }
                    this.session_in_new_workspace(&project, path.clone(), None, new.title, shown, caller)
                        .map(|s| json!({ "session": s["session"], "workspace": path }))
                });
                let _ = reply.send(answer);
                cx.notify();
            })
            .ok();
        })
        .detach();
        Ok(())
    }

    /// A new session in `folder`, sent its prompt and placed; the answer for the client.
    fn start_control_session(
        &mut self,
        project: &Path,
        folder: PathBuf,
        new: NewSession,
        caller: Option<Key>,
    ) -> Answer {
        self.room_in(project)?;
        let s = self.new_control_session(folder.clone(), new.prompt, new.title, caller);
        let key = s.uid;
        let pane = self.place_session(project, &folder, s, new.place, caller)?;
        Ok(json!({ "session": key, "workspace": folder, "pane": pane }))
    }

    /// A new session in `folder`, started, sent `prompt` and named `title` (once it has an id).
    pub(super) fn new_control_session(
        &mut self,
        folder: PathBuf,
        prompt: Option<String>,
        title: Option<String>,
        caller: Option<Key>,
    ) -> Session {
        let mut s = Session::new(Kind::Code, folder);
        self.adopt_project_settings(&mut s);
        self.start_model(&mut s);
        s.wake = self.wake.clone();
        if let Some(p) = prompt.filter(|p| !p.trim().is_empty()) {
            s.send(p, Vec::new());
        }
        if let Some(t) = title {
            self.pending_titles.push((s.uid, t));
        }
        self.approvals.record_made(Made::Session(s.uid), caller);
        s
    }

    /// Opens `s` in `project` without taking the focus: beside or below the asking (else the
    /// focused) pane of its workspace, in a new tab there, or only in the list. A workspace
    /// with no panes yet, or no room, lists it only. Returns its pane.
    fn place_session(
        &mut self,
        project: &Path,
        folder: &Path,
        s: Session,
        place: Place,
        caller: Option<Key>,
    ) -> Result<Option<PaneId>, String> {
        let key = s.uid;
        let live = self.code.get_mut(project).ok_or_else(|| format!("{} isn't open", project.display()))?;
        live.sessions.push(s);
        if place == Place::Hidden {
            return Ok(None);
        }
        let (pane, bounds) = (self.next_pane, self.tile_bounds());
        let Some(t) = self.tiles.iter_mut().find(|(ws, _)| same_folder(ws, folder)).map(|(_, t)| t) else {
            return Ok(None);
        };
        let before = t.focus();
        let Some(target) = caller.and_then(|c| t.pane_of(c)).or(before) else { return Ok(None) };
        let placed = match place {
            Place::Right => t.split_at(target, Direction::Right, pane, key, bounds),
            Place::Down => t.split_at(target, Direction::Down, pane, key, bounds),
            Place::Tab => t.new_tab(pane, key),
            Place::Hidden => false,
        };
        if !placed {
            return Ok(None);
        }
        if let Some(b) = before {
            t.focus_pane(b);
        }
        self.next_pane += 1;
        Ok(Some(pane))
    }

    /// `session fork`: a copy of open session `r`, from before its `at`th message if given.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn control_session_fork(
        &mut self,
        r: &SessionRef,
        at: Option<usize>,
        place: Place,
        caller: Option<Key>,
        reply: Sender<Answer>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let key = self.resolve_session(r)?;
        let project = self.project_of(key).ok_or("only a project's sessions can be forked")?;
        let s = self.session_by_key(key).ok_or_else(|| format!("no open session {r}"))?;
        if s.provider().provider() == Provider::Grok {
            return Err("Grok sessions can't be forked".into());
        }
        let id = s
            .session_id
            .clone()
            .filter(|_| !s.items.is_empty() && s.forked_from().is_none())
            .ok_or_else(|| format!("session {r} has nothing to fork yet"))?;
        let cwd = s.cwd.clone();
        self.room_in(&project)?;
        let Some(n) = at else {
            let _ = reply.send(self.finish_fork(&project, cwd, id, None, place, caller));
            return Ok(());
        };
        cx.spawn(async move |this, cx| {
            let (dir, from) = (cwd.clone(), id.clone());
            let points = cx.background_spawn(async move { rewind::points(&dir, &from) }).await;
            this.update(cx, |this, cx| {
                let answer = points
                    .and_then(|p| point_at(&p, n))
                    .and_then(|point| this.finish_fork(&project, cwd, id, Some(point), place, caller));
                let _ = reply.send(answer);
                cx.notify();
            })
            .ok();
        })
        .detach();
        Ok(())
    }

    fn finish_fork(
        &mut self,
        project: &Path,
        cwd: PathBuf,
        id: String,
        point: Option<Point>,
        place: Place,
        caller: Option<Key>,
    ) -> Answer {
        if place == Place::Hidden {
            return Err("a fork has to be shown: `--pane right`, `down` or `tab`".into());
        }
        self.room_in(project)?;
        let mut fork = Session::fork(Kind::Code, cwd.clone(), id.clone(), self.wake.clone());
        if let Some(p) = &point {
            fork.rewind_to(p);
        }
        self.adopt_project_settings(&mut fork);
        self.start_model(&mut fork);
        fork.wake = self.wake.clone();
        let key = fork.uid;
        self.approvals.record_made(Made::Session(key), caller);
        let pane = self.place_session(project, &cwd, fork, place, caller)?;
        Ok(json!({ "session": key, "forked_from": id, "pane": pane }))
    }

    /// `session open`: a past session of the project by its CLI id, or the open one.
    pub(super) fn control_session_open(
        &mut self,
        id: String,
        place: Place,
        caller: Option<Key>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Answer {
        if let Some(s) = self.all_sessions().find(|s| s.session_id.as_deref() == Some(id.as_str())) {
            return Ok(json!({ "session": s.uid, "already_open": true }));
        }
        if place == Place::Hidden {
            return Err("a past session opens in a pane: `--pane right`, `down` or `tab`".into());
        }
        let project = self.request_project(caller)?;
        let cwd = self
            .folder_of_listed(&id)
            .filter(|f| self.workspaces_of(&project, cx).iter().any(|(w, _)| w.at(f)))
            .ok_or_else(|| format!("no past session {id} in {}'s workspaces", project.display()))?;
        self.room_in(&project)?;
        self.edit_groups(&cwd, |g| {
            g.unarchive(&id);
        });
        let mut s = Session::resume(Kind::Code, cwd.clone(), id, self.wake.clone());
        self.adopt_project_settings(&mut s);
        self.start_model(&mut s);
        s.wake = self.wake.clone();
        let key = s.uid;
        let pane = self.place_session(&project, &cwd, s, place, caller)?;
        Ok(json!({ "session": key, "pane": pane }))
    }

    /// `session close`: its pane closes, and it does (its transcript stays). Not while it works.
    pub(super) fn control_session_close(
        &mut self,
        r: &SessionRef,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Answer {
        let key = self.resolve_session(r)?;
        if self.session_by_key(key).is_some_and(|s| s.busy) {
            return Err(format!("session {r} is working; it can be closed once its turn ends"));
        }
        for t in self.tiles.values_mut() {
            if let Some(p) = t.pane_of(key) {
                t.close_pane(p);
            }
        }
        let closed = self.chat.close(key) || self.code.values_mut().any(|l| l.close(key));
        if !closed {
            return Err(format!("session {r} couldn't be closed"));
        }
        self.forget(&[key]);
        self.show_focused_pane(window, cx);
        Ok(json!({ "closed": key }))
    }

    /// The folder and CLI id of session `r`, open or past; an open one may have no id yet.
    fn session_place(&self, r: &SessionRef) -> Result<(PathBuf, Option<String>, Option<Key>), String> {
        if let Ok(key) = self.resolve_session(r) {
            let s = self.session_by_key(key).ok_or_else(|| format!("no open session {r}"))?;
            return Ok((s.cwd.clone(), s.session_id.clone(), Some(key)));
        }
        match r {
            SessionRef::Id(id) => {
                let cwd = self.folder_of_listed(id).ok_or_else(|| format!("no session {id} in the list"))?;
                Ok((cwd, Some(id.clone()), None))
            }
            SessionRef::Uid(_) => Err(format!("no open session {r}")),
        }
    }

    /// `session rename`: by hand, as the list's Rename does; held until a new session has an id.
    pub(super) fn control_session_rename(&mut self, r: &SessionRef, title: &str) -> Answer {
        let (cwd, id, key) = self.session_place(r)?;
        match (id, key) {
            (Some(id), _) => self.edit_groups(&cwd, |g| g.rename(&id, title)),
            (None, Some(key)) => self.pending_titles.push((key, title.to_string())),
            (None, None) => return Err(format!("no session {r}")),
        }
        Ok(json!({ "renamed": r.to_string(), "title": title.trim() }))
    }

    /// `session archive`: out of the list, as the list's Archive does.
    pub(super) fn control_session_archive(&mut self, r: &SessionRef) -> Answer {
        let (cwd, id, _) = self.session_place(r)?;
        let id = id.ok_or_else(|| format!("session {r} has nothing to archive yet"))?;
        self.edit_groups(&cwd, |g| g.archive(&id));
        Ok(json!({ "archived": id }))
    }

    /// `session pin`: above the rest, or not with `off`.
    pub(super) fn control_session_pin(&mut self, r: &SessionRef, off: bool) -> Answer {
        let (cwd, id, _) = self.session_place(r)?;
        let id = id.ok_or_else(|| format!("session {r} has nothing to pin yet"))?;
        self.edit_groups(&cwd, |g| {
            if g.is_pinned(&id) == off {
                g.toggle_pin(&id);
            }
        });
        Ok(json!({ "session": id, "pinned": !off }))
    }

    /// Names the sessions asked to be named once they have an id; drops those that closed.
    pub(super) fn give_pending_titles(&mut self) {
        if self.pending_titles.is_empty() {
            return;
        }
        for (key, title) in std::mem::take(&mut self.pending_titles) {
            let Some(s) = self.session_by_key(key) else { continue };
            match s.session_id.clone() {
                Some(id) => {
                    let cwd = s.cwd.clone();
                    self.edit_groups(&cwd, |g| g.rename(&id, &title));
                }
                None => self.pending_titles.push((key, title)),
            }
        }
    }

    /// After a workspace a request made: a session in it, shown there (`workspace create
    /// --session`), since the workspace comes up as one made by hand does.
    pub(super) fn session_in_new_workspace(
        &mut self,
        project: &Path,
        path: PathBuf,
        prompt: Option<String>,
        title: Option<String>,
        shown: bool,
        caller: Option<Key>,
    ) -> Answer {
        self.room_in(project)?;
        // Not shown, it has no pane, and a session with nothing sent would be tidied away.
        if !shown && prompt.is_none() {
            return Ok(json!({
                "session": null,
                "note": "the project isn't shown, so a session with no prompt wasn't started; ask with --prompt",
            }));
        }
        let s = self.new_control_session(path, prompt, title, caller);
        let key = s.uid;
        let Some(live) = self.code.get_mut(project) else { return Err(format!("{} isn't open", project.display())) };
        live.sessions.push(s);
        if shown {
            live.focus(key);
        }
        Ok(json!({ "session": key }))
    }
}

/// The rewind point before message `n` (from 1) of a session's `points`.
fn point_at(points: &[Point], n: usize) -> Result<Point, String> {
    let p = n
        .checked_sub(1)
        .and_then(|i| points.get(i))
        .ok_or_else(|| format!("no message {n}: the session has {} (counted from 1)", points.len()))?;
    if p.before.is_none() {
        return Err("there's nothing before message 1 to fork: use `knx session new`".into());
    }
    Ok(p.clone())
}
