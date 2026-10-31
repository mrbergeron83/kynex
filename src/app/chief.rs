//! The chief of staff (`kynex_core::chief`): the calls its tools make, each
//! project's chief worker, and the messages the chief gets about them: turns
//! ended (with the worker's report), questions, risky commands, and workers a
//! restart cut short. `chief_view.rs` draws its view and the reporter.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

use gpui::{AppContext as _, Context};
use kynex_core::chief::{self, Call, Request, State, Worker};
use kynex_core::live::{Key, Live};
use kynex_core::paths::{chief_dir, project_name};
use kynex_core::session::{Item, Kind, Session};
use kynex_core::toasts::{Action, Toast};

use super::KynexApp;
use super::state::{Active, ProjectState, View};

/// The chief's side of the app: its sessions, its server, and the workers it runs.
#[derive(Default)]
pub(super) struct Chief {
    pub(super) live: Live,
    server: Option<chief::Server>,
    /// Each project's chief worker, while its session is open.
    pub(super) workers: HashMap<PathBuf, Key>,
    /// When each working worker's turn started.
    started: HashMap<Key, Instant>,
    /// Workers working, and waiting on the user, as of the last poll.
    was_busy: HashSet<Key>,
    was_waiting: HashSet<Key>,
    /// How many of each worker's turns have been reported.
    seen: HashMap<Key, usize>,
    /// Tool calls already looked at for risky commands.
    checked: HashSet<String>,
    /// The risky commands each worker ran in its current turn.
    warnings: HashMap<Key, Vec<String>>,
    /// Workers a restart cut short have been reported.
    resumed: bool,
}

impl KynexApp {
    /// The Chat tab is showing the chief.
    pub(super) fn in_chief(&self) -> bool {
        self.saved.active == Active::Chat && self.saved.chief_view
    }

    /// The chief's sessions, created (with its saved session resumed) on first
    /// use; its tools start with it.
    pub(super) fn chief_live_mut(&mut self) -> &mut Live {
        self.start_chief_server();
        if self.chief.live.sessions.is_empty() {
            let mut s = match self.saved.chief_session_id.clone() {
                Some(id) => Session::resume(Kind::Chat, chief_dir(), id, self.wake.clone()),
                None => Session::new(Kind::Chat, chief_dir()),
            };
            self.start_session(&mut s);
            s.wake = self.wake.clone();
            self.chief.live.sessions.push(s);
            self.chief.live.active = 0;
        }
        &mut self.chief.live
    }

    /// Starts the chief's MCP server once it's turned on; its sessions get it
    /// from their next start.
    fn start_chief_server(&mut self) {
        if self.chief.server.is_some() || !self.saved.chief_consented {
            return;
        }
        let _ = std::fs::create_dir_all(chief_dir());
        match chief::Server::start(self.wake.clone()) {
            Ok(server) => {
                chief::set_launch_args(server.launch_args());
                self.chief.server = Some(server);
            }
            Err(e) => {
                self.toast(Toast::error(e));
            }
        }
    }

    /// Answers the chief's tool calls and tells it about its workers. Called on every poll.
    pub(super) fn poll_chief(&mut self, cx: &mut Context<Self>) {
        if !self.chief.resumed && self.saved.chief_consented {
            self.chief.resumed = true;
            let cut: Vec<String> = self.saved.chief_working.drain(..).map(|p| project_name(&p)).collect();
            if !cut.is_empty() {
                self.save();
                self.tell_chief(vec![chief::cut_off_notice(&cut)]);
            }
        }
        let requests = self.chief.server.as_ref().map(|s| s.requests()).unwrap_or_default();
        for r in requests {
            self.answer_chief(r, cx);
        }
        self.report_workers();
    }

    fn answer_chief(&mut self, r: Request, cx: &mut Context<Self>) {
        let projects: Vec<PathBuf> = self.saved.projects.iter().map(|p| p.path.clone()).collect();
        let pick = |name: &str| chief::find_project(&projects, name).cloned();
        let call = r.call.clone();
        let answer = match &call {
            Call::Projects => Ok(chief::projects_text(&projects)),
            Call::Status => Ok(chief::status_text(&self.chief_workers())),
            Call::Dispatch { project, prompt } => pick(project).map(|path| self.dispatch(&path, prompt.clone())),
            Call::Result { project } => pick(project).and_then(|path| {
                let s = self.open_worker(&path).ok_or(format!("{project} has no chief worker open"))?;
                let w = self.worker_view(&path, s);
                Ok(chief::result_text(&w, chief::last_reply(&s.items).as_deref()))
            }),
            Call::Stop { project } => pick(project).and_then(|path| {
                let key =
                    self.open_worker(&path).map(|s| s.uid).ok_or(format!("{project} has no chief worker open"))?;
                self.stop_worker(key);
                Ok(format!("Stopped the {} worker's turn.", project_name(&path)))
            }),
            Call::OpenProject { path } => chief::existing_project(path).map(|p| self.add_project_quietly(p)),
            Call::CreateProject { path } => {
                // `git init` runs off the UI thread; the call waits for it.
                let path = path.clone();
                let made = cx.background_spawn(async move { chief::create_project(&path) });
                cx.spawn(async move |this, cx| {
                    let made = made.await;
                    this.update(cx, |this, cx| {
                        r.answer(made.map(|p| this.add_project_quietly(p)));
                        cx.notify();
                    })
                    .ok();
                })
                .detach();
                return;
            }
        };
        r.answer(answer);
    }

    /// Opens `path` as a project without switching the user's tab.
    fn add_project_quietly(&mut self, path: PathBuf) -> String {
        let name = project_name(&path);
        if self.saved.projects.iter().any(|p| p.path == path) {
            return format!("{name} ({}) is already open.", path.display());
        }
        self.saved.projects.push(ProjectState { path: path.clone(), view: View::Code, session_id: None });
        self.save();
        format!("Opened {name} ({}). dispatch can send it work now.", path.display())
    }

    /// Sends `prompt` to the project's worker; a busy one queues it.
    fn dispatch(&mut self, path: &Path, prompt: String) -> String {
        let name = project_name(path);
        let s = self.worker_mut(path);
        if s.busy || !s.queue.is_empty() {
            s.queue.push(prompt, Vec::new());
            return format!("The {name} worker is busy: the work is queued and starts when its turn ends.");
        }
        s.send(prompt, Vec::new());
        let key = s.uid;
        self.chief.started.insert(key, Instant::now());
        format!("Dispatched to the {name} worker. Kynex will message you with its report.")
    }

    /// The project's worker, if its session is open.
    pub(super) fn open_worker(&self, path: &Path) -> Option<&Session> {
        let key = self.chief.workers.get(path)?;
        self.code.get(path)?.sessions.iter().find(|s| s.uid == *key)
    }

    /// The project's worker: the open one, else its saved session resumed, else a new one.
    fn worker_mut(&mut self, path: &Path) -> &mut Session {
        let resume = self.saved.chief_workers.get(path).cloned();
        let spent = self.saved.chief_spent.get(path).copied().unwrap_or_default();
        let key = self.chief.workers.get(path).copied();
        let wake = self.wake.clone();
        let (launch, models) = self.start_parts(path, resume.as_deref());
        let live = self.code.entry(path.to_path_buf()).or_default();
        let open = live
            .sessions
            .iter()
            .position(|s| Some(s.uid) == key || (resume.is_some() && s.session_id.as_deref() == resume.as_deref()));
        let i = open.unwrap_or_else(|| {
            let mut s = match resume {
                Some(id) => Session::resume(Kind::Code, path.to_path_buf(), id, wake.clone()),
                None => Session::new(Kind::Code, path.to_path_buf()),
            };
            super::provider_setup::apply_start(&mut s, launch, &models);
            s.wake = wake;
            // What it used in earlier runs counts too.
            s.spent = spent;
            live.sessions.push(s);
            live.sessions.len() - 1
        });
        let s = &mut live.sessions[i];
        // One you had open runs with prompts: it starts again without them.
        if !s.worker && s.is_running() && !s.busy {
            s.kill();
        }
        s.worker = true;
        if key != Some(s.uid) {
            // Commands it ran before it was a worker aren't news.
            self.chief.checked.extend(s.items.iter().filter_map(|i| match i {
                Item::ToolUse { id, .. } => Some(id.clone()),
                _ => None,
            }));
        }
        self.chief.workers.insert(path.to_path_buf(), s.uid);
        s
    }

    /// Stops a worker's turn, and what it had queued.
    pub(super) fn stop_worker(&mut self, key: Key) {
        if let Some(s) = self.session_for(key) {
            s.queue.take_all();
            if s.busy {
                s.interrupt();
            }
        }
    }

    pub(super) fn stop_everything(&mut self) {
        let keys: Vec<Key> = self.chief.workers.values().copied().collect();
        for k in keys {
            self.stop_worker(k);
        }
        if let Some(s) = self.chief.live.current_mut()
            && s.busy
        {
            s.interrupt();
        }
    }

    /// A worker's session as the chief and the reporter see it.
    fn worker_view(&self, path: &Path, s: &Session) -> Worker {
        worker_of(
            path,
            s,
            self.chief.started.get(&s.uid).copied(),
            self.chief.warnings.get(&s.uid).cloned().unwrap_or_default(),
        )
    }

    /// Every chief worker: the open ones, then the ones asleep.
    pub(super) fn chief_workers(&self) -> Vec<Worker> {
        let mut workers: Vec<Worker> = self
            .chief
            .workers
            .keys()
            .filter_map(|path| Some(self.worker_view(path, self.open_worker(path)?)))
            .collect();
        for path in self.saved.chief_workers.keys().filter(|p| !self.chief.workers.contains_key(*p)) {
            workers.push(Worker {
                path: path.clone(),
                model: self.claude_settings(path).model,
                spent: self.saved.chief_spent.get(path).copied().unwrap_or_default(),
                ..Default::default()
            });
        }
        workers.sort_by_key(|w| w.name().to_lowercase());
        workers
    }

    /// Tells the chief when a worker ends a turn or waits on the user, and
    /// you when a worker runs something risky.
    fn report_workers(&mut self) {
        let mut notices = Vec::new();
        let mut toasts = Vec::new();
        let mut working = Vec::new();
        let workers: Vec<(PathBuf, Key)> = self.chief.workers.iter().map(|(p, k)| (p.clone(), *k)).collect();
        for (path, key) in workers {
            let started = self.chief.started.get(&key).copied();
            let Some(s) = self.code.get_mut(&path).and_then(|l| l.sessions.iter_mut().find(|s| s.uid == key)) else {
                // Its session closed; the next dispatch resumes it.
                self.chief.workers.remove(&path);
                self.chief.was_busy.remove(&key);
                self.chief.was_waiting.remove(&key);
                self.chief.seen.remove(&key);
                self.chief.warnings.remove(&key);
                continue;
            };
            let name = project_name(&path);
            // Risky commands in the current turn, each once.
            let turn = s.items.iter().rposition(|i| matches!(i, Item::User { .. })).unwrap_or(0);
            for item in &s.items[turn..] {
                if let Item::ToolUse { id, name: tool, input, .. } = item
                    && self.chief.checked.insert(id.clone())
                    && let Some(why) = chief::risky(tool, input)
                {
                    let command = serde_json::from_str::<serde_json::Value>(input)
                        .ok()
                        .and_then(|v| v["command"].as_str().map(String::from))
                        .unwrap_or_default();
                    self.chief.warnings.entry(key).or_default().push(why.to_string());
                    toasts.push(
                        Toast::warning(format!("The {name} worker {why}"))
                            .detail(command)
                            .session(key)
                            .action(Action::GoToSession(key)),
                    );
                }
            }
            let (busy, waiting) = (s.busy, s.waiting());
            let (was_busy, was_waiting) = (self.chief.was_busy.contains(&key), self.chief.was_waiting.contains(&key));
            if busy && !was_busy && started.is_none() {
                self.chief.started.insert(key, Instant::now());
            }
            if busy {
                working.push(path.clone());
            }
            // Turns it ended before it became a worker aren't news.
            let reported = *self.chief.seen.entry(key).or_insert_with(|| chief::turns_ended(&s.items, 0).0);
            let (count, mut ended) = chief::turns_ended(&s.items, reported);
            self.chief.seen.insert(key, count);
            // Its process ended mid-turn, with no result.
            if was_busy && !busy && ended.is_empty() {
                ended.push(chief::cut_short(&s.items));
            }
            if !ended.is_empty() {
                let warnings = self.chief.warnings.remove(&key).unwrap_or_default();
                let w = worker_of(&path, s, started, warnings);
                notices.extend(ended.iter().map(|t| chief::turn_notice(&w, t)));
                // The chief hears about it, not you.
                s.finished = false;
                self.saved.chief_spent.insert(path.clone(), s.spent);
                if busy {
                    self.chief.started.insert(key, Instant::now());
                } else {
                    self.chief.started.remove(&key);
                }
            }
            if waiting && !was_waiting {
                notices.push(chief::waiting_notice(&worker_of(&path, s, started, Vec::new())));
            }
            set(&mut self.chief.was_busy, key, busy);
            set(&mut self.chief.was_waiting, key, waiting);
        }
        for t in toasts {
            self.toast(t);
        }
        working.sort();
        if working != self.saved.chief_working || !notices.is_empty() {
            self.saved.chief_working = working;
            self.save();
        }
        self.tell_chief(notices);
    }

    /// Sends the chief Kynex's messages, or queues them while it's busy.
    fn tell_chief(&mut self, notices: Vec<String>) {
        if notices.is_empty() {
            return;
        }
        let Some(s) = self.chief_live_mut().current_mut() else { return };
        for n in notices {
            if s.busy || s.loading() {
                s.queue.push(n, Vec::new());
            } else {
                s.send(n, Vec::new());
            }
        }
    }

    /// Keeps the chief's and its workers' session ids for the next run.
    pub(super) fn sync_chief_ids(&mut self) -> bool {
        let mut changed = false;
        let id = self.chief.live.current().filter(|s| !s.is_new()).and_then(|s| s.session_id.clone());
        if id.is_some() && self.saved.chief_session_id != id {
            self.saved.chief_session_id = id;
            changed = true;
        }
        for (path, key) in &self.chief.workers {
            let id = self
                .code
                .get(path)
                .and_then(|l| l.sessions.iter().find(|s| s.uid == *key))
                .filter(|s| !s.is_new())
                .and_then(|s| s.session_id.clone());
            if let Some(id) = id
                && self.saved.chief_workers.get(path) != Some(&id)
            {
                self.saved.chief_workers.insert(path.clone(), id);
                changed = true;
            }
        }
        changed
    }
}

fn set(keys: &mut HashSet<Key>, key: Key, on: bool) {
    if on {
        keys.insert(key);
    } else {
        keys.remove(&key);
    }
}

/// A worker's session as the chief and the reporter see it.
fn worker_of(path: &Path, s: &Session, started: Option<Instant>, warnings: Vec<String>) -> Worker {
    let report = (!s.busy).then(|| chief::last_reply(&s.items)).flatten().map(|r| chief::parse_report(&r));
    let blocked = report.as_ref().is_some_and(|r| r.status == Some(chief::Status::Blocked));
    let state = if s.waiting() {
        State::NeedsYou
    } else if s.busy {
        State::Working
    } else if blocked {
        State::Blocked
    } else if s.is_running() {
        State::Idle
    } else {
        State::Asleep
    };
    // Subagents in the foreground are tool calls still running; background ones are tasks.
    let turn = s.items.iter().rposition(|i| matches!(i, Item::User { .. })).unwrap_or(0);
    let foreground = s.items[turn..]
        .iter()
        .filter(|i| {
            matches!(i, Item::ToolUse { name, timing, .. } if (name == "Agent" || name == "Task") && timing.is_running())
        })
        .count();
    Worker {
        path: path.to_path_buf(),
        model: if s.model.is_empty() { "default".into() } else { s.model.clone() },
        state,
        turn: started.filter(|_| s.busy).map(|t| t.elapsed()),
        spent: s.spent,
        agents: foreground + s.tasks.running.len(),
        queued: s.queue.items().len(),
        question: report.and_then(|r| r.question).filter(|_| blocked),
        warnings,
    }
}
