//! Tabs and sessions: which project is active, which of its sessions is shown,
//! and opening, switching and closing them.

use std::path::PathBuf;

use gpui::{Context, Focusable, PathPromptOptions, Window};
use kynex_core::live::{Key, Live};
use kynex_core::paths::chat_dir;
use kynex_core::session::{Kind, Session};

use super::KynexApp;
use super::state::{Active, ProjectState, View};

impl KynexApp {
    /// Every open session, in every tab.
    pub(super) fn all_sessions_mut(&mut self) -> impl Iterator<Item = &mut Session> {
        self.chat.sessions.iter_mut().chain(self.code.values_mut().flat_map(|l| l.sessions.iter_mut()))
    }

    /// Copy live session ids into the saved state so they survive a restart.
    pub(super) fn sync_session_ids(&mut self) {
        let mut changed = false;
        let chat_id = self.chat.current().and_then(|s| s.session_id.clone());
        if self.saved.chat_session_id != chat_id {
            self.saved.chat_session_id = chat_id;
            changed = true;
        }
        for p in &mut self.saved.projects {
            if let Some(s) = self.code.get(&p.path).and_then(Live::current)
                && p.session_id != s.session_id
            {
                p.session_id = s.session_id.clone();
                changed = true;
            }
        }
        if changed {
            self.save();
        }
    }

    pub(super) fn active_project(&self) -> Option<&ProjectState> {
        match self.saved.active {
            Active::Project(i) => self.saved.projects.get(i),
            Active::Chat => None,
        }
    }

    /// The shown session's uid.
    pub(super) fn active_key(&self) -> Option<Key> {
        self.active_session().map(|s| s.uid)
    }

    pub(super) fn session_for(&mut self, key: Key) -> Option<&mut Session> {
        self.all_sessions_mut().find(|s| s.uid == key)
    }

    fn active_live(&self) -> Option<&Live> {
        match self.saved.active {
            Active::Chat => Some(&self.chat),
            Active::Project(i) => self.saved.projects.get(i).and_then(|p| self.code.get(&p.path)),
        }
    }

    pub(super) fn active_session(&self) -> Option<&Session> {
        self.active_live().and_then(Live::current)
    }

    /// The open sessions in the active tab's folder, created (with the saved
    /// session resumed) on first use.
    pub(super) fn live_mut(&mut self) -> Option<&mut Live> {
        let (kind, path, resume, live) = match self.saved.active {
            Active::Chat => (Kind::Chat, chat_dir(), None, &mut self.chat),
            Active::Project(i) => {
                let p = self.saved.projects.get(i)?;
                if p.view != View::Code {
                    return None;
                }
                (Kind::Code, p.path.clone(), p.session_id.clone(), self.code.entry(p.path.clone()).or_default())
            }
        };
        let (model, wake) = (self.saved.model.clone(), self.wake.clone());
        if live.sessions.is_empty() {
            let mut s = match resume {
                Some(id) => Session::resume(kind, path, id),
                None => Session::new(kind, path),
            };
            s.model = model;
            s.wake = wake;
            live.sessions.push(s);
            live.active = 0;
        }
        Some(live)
    }

    /// The session the composer talks to.
    pub(super) fn session_mut(&mut self) -> Option<&mut Session> {
        self.live_mut()?.current_mut()
    }

    /// Shows `s` in the active tab. Sessions it replaces keep running if
    /// they're working; idle ones close.
    pub(super) fn open_session(&mut self, mut s: Session) {
        s.model = self.saved.model.clone();
        s.wake = self.wake.clone();
        let Some(live) = self.live_mut() else { return };
        let closed = live.show(s);
        self.forget(&closed);
    }

    /// Shows the open session with this CLI id, or resumes it from its transcript.
    /// An archived session comes out of the archive.
    pub(super) fn switch_session(&mut self, kind: Kind, cwd: PathBuf, id: String) {
        self.edit_groups(&cwd, |g| {
            g.unarchive(&id);
        });
        if !self.show_open(|s| s.session_id.as_deref() == Some(id.as_str())) {
            self.open_session(Session::resume(kind, cwd, id));
        }
    }

    /// Shows an open session in the active tab, if one matches.
    pub(super) fn show_open(&mut self, f: impl Fn(&Session) -> bool) -> bool {
        let Some(live) = self.live_mut() else { return false };
        if !live.show_where(f) {
            return false;
        }
        let closed = live.prune();
        self.forget(&closed);
        true
    }

    /// Closes an open session that isn't working (its transcript stays).
    pub(super) fn close_open(&mut self, key: Key) {
        if self.live_mut().is_some_and(|l| l.close(key)) {
            self.forget(&[key]);
        }
    }

    /// Drops the view state of sessions that closed.
    fn forget(&mut self, closed: &[Key]) {
        for k in closed {
            self.threads.remove(k);
        }
        self.expanded.retain(|(k, _)| !closed.contains(k));
        self.open_runs.retain(|(k, _)| !closed.contains(k));
        self.forget_drafts(closed);
    }

    pub(super) fn select(&mut self, a: Active, window: &mut Window, cx: &mut Context<Self>) {
        if self.saved.active != a {
            self.saved.active = a;
            self.menu = None;
            self.save();
            self.composer.read(cx).focus_handle(cx).focus(window, cx);
        }
        cx.notify();
    }

    pub(super) fn add_project(&mut self, path: PathBuf) {
        let idx = match self.saved.projects.iter().position(|p| p.path == path) {
            Some(i) => i,
            None => {
                self.saved.projects.push(ProjectState { path, view: View::Code, session_id: None });
                self.saved.projects.len() - 1
            }
        };
        self.saved.active = Active::Project(idx);
        self.save();
    }

    pub(super) fn close_project(&mut self, i: usize) {
        let p = self.saved.projects.remove(i);
        let closed: Vec<Key> = self.code.remove(&p.path).into_iter().flat_map(|l| l.sessions).map(|s| s.uid).collect();
        self.forget(&closed);
        self.git.remove(&p.path);
        self.worktrees.remove(&p.path);
        self.saved.active = match self.saved.active {
            Active::Project(a) if a > i => Active::Project(a - 1),
            Active::Project(a) if a == i && i > 0 => Active::Project(i - 1),
            Active::Project(a) if a == i => {
                if self.saved.projects.is_empty() {
                    Active::Chat
                } else {
                    Active::Project(0)
                }
            }
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
