//! The session's process: starting it, writing to it, reading from it, stopping it.

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::Command;
use std::sync::mpsc::{Sender, channel};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::*;

impl Session {
    /// Starts its process on a thread ([`launch::begin`]) and returns at once.
    /// Until it's up, [`Self::pulse`] says it's starting and what's written to
    /// it waits; [`Self::poll`] takes it in, or reports why it didn't start.
    pub(super) fn spawn(&mut self) {
        self.bypass_on_start();
        let (tx, rx) = channel();
        let spec = launch::Spec {
            uid: self.uid,
            kind: self.kind,
            cwd: self.cwd.clone(),
            settings_dir: self.settings_folder().to_path_buf(),
            model: self.model.clone(),
            provider: self.provider.clone(),
            mode: self.mode,
            bypass: self.bypass,
            resume: self.resume_id().map(str::to_string),
            resume_at: self.resume_at.take(),
            fork: self.fork_from.is_some(),
            extra_env: self.extra_env.clone(),
            wake: self.wake.clone(),
        };
        self.starting = Some(launch::begin(spec, tx));
        self.outbox.clear();
        self.rx = Some(rx);
        self.launched = None;
        self.spawned = Some(Instant::now());
        self.runs += 1;
        self.carried_cost += std::mem::take(&mut self.run_cost);
        self.ready = false;
        self.talked = false;
        // `claude` is asked for its commands and models first, as the Agent SDK does.
        self.write(json!({ "type": "control_request", "request_id": INIT_ID, "request": { "subtype": "initialize" } }));
        self.ultrathink = Default::default();
        if self.thinking_off && self.provider.provider() != crate::provider::Provider::OpenRouterChat {
            self.control(crate::claude::input::thinking::request(false));
        }
    }

    /// Takes in a start that has finished: the process, and what was written
    /// to it meanwhile goes out; or, if it failed, why (when a message waits on it).
    pub(super) fn take_started(&mut self) {
        let Some(rx) = &self.starting else { return };
        let mut started = match rx.try_recv() {
            Ok(s) => s,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.start_failed("failed to start the process".into());
                return;
            }
        };
        self.starting = None;
        self.model = std::mem::take(&mut started.model);
        let (Some(child), Some(stdin)) = (started.child.take(), started.stdin.take()) else {
            let e = started.error.take().unwrap_or_else(|| "failed to start the process".into());
            self.start_failed(e);
            return;
        };
        self.child = Some(child);
        self.stdin = Some(stdin);
        self.launched = started.launched.take();
        self.failed_at = None;
        for line in std::mem::take(&mut self.outbox) {
            self.write_raw(line);
        }
    }

    /// A start failed with `e`: shown if a message was waiting on it (a warm-up's
    /// is left for the next message to report), and warming up backs off.
    pub(super) fn start_failed(&mut self, e: String) {
        self.starting = None;
        self.failed_at = Some(Instant::now());
        if self.busy {
            self.items.push(Item::Error(e));
        }
        self.kill();
    }

    /// The OpenRouter model a request just failed for because it takes no
    /// tool calls, once.
    pub fn take_no_tools(&mut self) -> Option<String> {
        self.no_tools.take()
    }

    /// The commands and models the process reported when it started, once.
    pub fn take_init(&mut self) -> Option<Init> {
        self.init.take()
    }

    /// The usage limits the process last reported, once.
    pub fn take_limits(&mut self) -> Option<crate::claude::usage::Limits> {
        self.limits.take()
    }

    /// Writes `msg` as `claude` takes it.
    pub(super) fn write(&mut self, msg: Value) {
        self.write_line(&msg);
    }

    /// Writes one JSON line to the process.
    pub(super) fn write_line(&mut self, msg: &Value) {
        let mut line = msg.to_string();
        line.push('\n');
        self.write_raw(line);
    }

    /// Writes `line` to the process, or keeps it for when it's up if it's
    /// starting. A write that fails comes back as [`Event::Lost`].
    pub(super) fn write_raw(&mut self, line: String) {
        match &self.stdin {
            Some(stdin) => {
                let _ = stdin.send(line);
            }
            None if self.starting.is_some() => self.outbox.push(line),
            None => {}
        }
    }

    /// Brings the running process up to its project's changed rules, extra
    /// folders or effort without a restart, where the CLI can take them live
    /// (see [`project::Launched::update`]); otherwise marks it [`Self::stale`].
    /// A process that isn't running reads them when it starts.
    pub fn apply_settings(&mut self, perms: &project::Permissions, claude: &project::ClaudeSettings) {
        let claude = &claude.for_provider(self.provider.provider());
        if claude.thinking_off != self.thinking_off {
            self.set_thinking(!claude.thinking_off);
        }
        let Some(launched) = &mut self.launched else { return };
        match launched.update(perms, claude) {
            project::Update::Same => {}
            project::Update::Live(settings) => {
                let id = self.control(json!({ "subtype": "apply_flag_settings", "settings": settings }));
                self.applying.push(id);
            }
            project::Update::Restart => self.stale = true,
        }
    }

    /// Stops background task `id` (as the CLI's own task list does). It stays
    /// in `tasks.running`, marked stopping, until the CLI reports it stopped.
    pub fn stop_task(&mut self, id: &str) {
        let Some(task) = self.tasks.running.iter_mut().find(|t| t.id == id && !t.stopping) else { return };
        task.stopping = true;
        let request = self.control(json!({ "subtype": "stop_task", "task_id": id }));
        self.stopping.insert(request, id.to_string());
    }

    /// Restarts MCP server `name` in the running process, then asks how every
    /// server is doing, which updates [`Self::mcp`]. A refusal (no such server,
    /// no process) shows in the transcript.
    pub fn reconnect_mcp(&mut self, name: &str) {
        if !self.is_running() {
            return;
        }
        let reconnect = self.control(json!({ "subtype": "mcp_reconnect", "serverName": name }));
        let status = self.control(json!({ "subtype": "mcp_status" }));
        self.mcp_requests.extend([reconnect, status]);
    }

    /// Sends a control request; returns its id.
    pub(super) fn control(&mut self, request: Value) -> String {
        self.next_request += 1;
        let id = format!("kynex-{}", self.next_request);
        self.write(json!({ "type": "control_request", "request_id": id, "request": request }));
        id
    }

    pub fn kill(&mut self) {
        let _t = crate::trace::span("Session::kill");
        if let Some(mut child) = self.child.take() {
            // Ending a process can take seconds (it has MCP servers and a browser of its
            // own to take down, and an antivirus looks at every exit), and this runs when a
            // session closes, from the thread that draws the window: end it on another.
            thread::spawn(move || {
                let _ = child.kill();
                let _ = child.wait();
            });
        }
        // A start still running finishes on a thread; what it started is stopped there.
        if let Some(rx) = self.starting.take() {
            thread::spawn(move || drop(rx.recv()));
        }
        self.outbox.clear();
        self.stdin = None;
        self.rx = None;
        self.busy = false;
        self.spawned = None;
        self.ready = false;
        self.in_block = false;
        // Its browser closed with it.
        self.browser = false;
        self.stale = false;
        self.launched = None;
        self.applying.clear();
        self.settle_thoughts();
        self.tasks.clear();
        self.stopping.clear();
        self.steered = false;
        self.late_steer = false;
        // Nobody is left to answer them.
        self.cancel_permissions(|_| true);
    }

    /// Marks the pending permission prompts `which` picks as cancelled.
    pub(super) fn cancel_permissions(&mut self, which: impl Fn(&str) -> bool) {
        self.perm_gen += 1;
        for item in &mut self.items {
            if let Item::Permission { request_id, state, .. } = item
                && *state == PermState::Pending
                && which(request_id)
            {
                *state = PermState::Denied("cancelled".into());
            }
        }
    }

    /// Whether background tasks are running, which closing the process would stop.
    pub fn has_tasks(&self) -> bool {
        !self.tasks.running.is_empty()
    }

    /// Drain pending events from the process. Call when woken.
    pub fn poll(&mut self) {
        self.take_started();
        let Some(rx) = &self.rx else { return };
        let events: Vec<Event> = rx.try_iter().collect();
        if !events.is_empty() {
            self.active_at = Instant::now();
        }
        let (mut exited, mut lost) = (false, false);
        let run = self.runs;
        for ev in events {
            // A new process started while handling these: the rest came from
            // the old one, which is gone.
            if self.runs != run {
                return;
            }
            match ev {
                Event::Line(v) => {
                    self.process_up();
                    self.handle(v);
                }
                Event::Stderr(s) => {
                    let provider = self.provider.provider();
                    let shown: Vec<&str> = s.lines().filter(|l| provider.shows_stderr(l)).collect();
                    if !shown.is_empty() {
                        self.items.push(Item::Error(shown.join(
                            "
",
                        )));
                    }
                }
                Event::Exited => exited = true,
                Event::Lost => lost = true,
            }
        }
        if exited || lost {
            self.settle_thoughts();
            let who = "claude";
            if lost {
                self.items.push(Item::Error(format!("lost connection to {who} process")));
            } else if self.busy {
                self.items.push(Item::Error(format!("{who} process exited")));
            }
            self.kill();
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.kill();
    }
}

/// Reads the process's stdout and stderr on two threads. `Exited` goes out once
/// stdout closes and stderr has had its say, so the reason a process died
/// (a bad `--model`, a failed `--resume`) arrives before it.
pub(super) fn spawn_readers(
    stdout: impl std::io::Read + Send + 'static,
    stderr: impl std::io::Read + Send + 'static,
    tx: Sender<Event>,
    wake: Wake,
) {
    // Dropped when the stderr thread ends.
    let (stderr_open, stderr_done) = channel::<()>();
    let (err_tx, err_wake) = (tx.clone(), wake.clone());
    thread::spawn(move || {
        let _open = stderr_open;
        for line in BufReader::new(stderr).lines() {
            let Ok(line) = line else { break };
            if err_tx.send(Event::Stderr(line)).is_err() {
                return;
            }
            err_wake();
        }
    });
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            let Ok(v) = serde_json::from_str(&line) else { continue };
            if tx.send(Event::Line(v)).is_err() {
                return;
            }
            wake();
        }
        // A grandchild that inherited stderr can hold it open; don't wait on it for long.
        let _ = stderr_done.recv_timeout(STDERR_GRACE);
        let _ = tx.send(Event::Exited);
        wake();
    });
}

/// How long `Exited` waits for stderr to close after stdout has.
pub(super) const STDERR_GRACE: Duration = Duration::from_millis(500);

/// `claude`, to be run in `cwd` without a console window, with the environment
/// as it is now (tools installed since Kynex started) and `provider`'s on top.
/// Blocking: reads the registry on Windows.
pub(crate) fn claude_command(cwd: &Path, provider: &crate::provider::Launch) -> Command {
    let mut cmd = Command::new(crate::cli::program());
    cmd.current_dir(cwd);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let fresh = crate::fresh_env::vars();
    cmd.envs(fresh.iter().map(|(n, v)| (n, v)));
    provider.apply_over(&mut cmd, &fresh);
    cmd
}
