//! Sending messages: a turn, steering one in progress, the queue, the model and mode.

use std::time::Instant;

use serde_json::json;

use super::*;

impl Session {
    /// Sends a message, starting the process if it isn't running. While the
    /// log is still loading it waits in the queue, to go after the history.
    pub fn send(&mut self, text: String, attachments: Vec<Attachment>) {
        self.send_with(text, attachments, false);
    }

    /// [`Self::send`], saying whether the user typed `ultrathink` in it themselves
    /// (see [`crate::claude::input::thinking::Ultrathink::typed`]).
    pub fn send_with(&mut self, text: String, attachments: Vec<Attachment>, ultrathink_typed: bool) {
        if self.loading() {
            self.queue.push(text, attachments);
            return;
        }
        if !self.is_running() {
            self.spawn();
        }
        let mut content = Vec::new();
        let mut images = Vec::new();
        for a in attachments {
            content.push(json!({
                "type": "image",
                "source": { "type": "base64", "media_type": a.media_type, "data": a.base64 }
            }));
            images.push(a.preview);
        }
        // `!` commands run since the last message go with it, as the CLI logs them.
        for block in self.unsent_shell_blocks() {
            content.push(json!({ "type": "text", "text": block }));
        }
        if !text.is_empty() {
            content.push(json!({ "type": "text", "text": text }));
        }
        self.items.push(Item::User { text, images });
        // A new turn: one a late steer may start is this one.
        self.late_steer = false;
        self.busy = true;
        self.talked = true;
        self.conversation = true;
        self.answered = false;
        self.quiet_since = Instant::now();
        self.active_at = Instant::now();
        let mut msg = json!({ "type": "user", "message": { "role": "user", "content": content } });
        if ultrathink_typed && self.ultrathink.typed_raises {
            msg["ultrathink_typed"] = json!(true);
        }
        self.write(msg);
        // A fork's process has the message: it writes the new session's log, which
        // a later process resumes by the id it reports.
        self.fork_from = None;
    }

    /// Starts the process ahead of the first message, so sending doesn't wait
    /// for it (about half a second to well over one). Call when a message is
    /// likely: the composer focused, typing, the session shown. Does nothing if
    /// it's running or starting; a failure to start is left for [`Self::send`]
    /// to report, and isn't retried here for a while. Either way it counts as
    /// activity, so it doesn't fall asleep under you.
    pub fn warm(&mut self) {
        self.active_at = Instant::now();
        let backing_off = self.failed_at.is_some_and(|t| t.elapsed() < START_BACKOFF);
        if !self.is_running() && !backing_off {
            self.spawn();
        }
    }

    /// Whether stopping its process now loses nothing: it isn't working,
    /// waiting on you, holding queued messages, running background tasks or
    /// paired with a browser. Its transcript stays, and it resumes on the next message.
    pub fn can_sleep(&self) -> bool {
        !self.busy && !self.waiting() && self.queue.is_empty() && !self.has_tasks() && !self.browser
    }

    /// Stops its process if it has been idle for `after` as of `now` and
    /// [`Self::can_sleep`]. Returns whether it did.
    pub fn sleep_if_idle(&mut self, after: Duration, now: Instant) -> bool {
        let due = self.is_running() && self.can_sleep() && now.saturating_duration_since(self.active_at) >= after;
        if due {
            self.kill();
        }
        due
    }

    /// Sets the model it starts on. A process warmed up but not talked to yet
    /// stops, to start again on it; one in use keeps its model until it next
    /// starts. On OpenRouter, where a model can go away under a conversation,
    /// an idle one stops too: the next message resumes it on the new model.
    pub fn use_model(&mut self, model: String) {
        if self.is_running() {
            let openrouter = self.provider.provider().is_openrouter();
            if model == self.model || (self.talked && !(openrouter && !self.busy)) {
                return;
            }
            self.kill();
        }
        self.model = model;
    }

    /// What it's doing when nothing in the transcript shows it: starting, waiting
    /// for Claude, between steps. `None` when idle, or while a block streams
    /// in, a tool runs or a prompt waits, which show themselves.
    pub fn pulse(&self) -> Option<crate::claude::pulse::Pulse> {
        use crate::claude::pulse::{Activity, Pulse};
        if !self.busy || self.in_block || self.waiting() {
            return None;
        }
        let mut turn = self.items.iter().rev().take_while(|i| !matches!(i, Item::User { .. }));
        if turn.any(|i| matches!(i, Item::ToolUse { timing, .. } if timing.is_running())) {
            return None;
        }
        Some(match (self.ready, self.answered) {
            (false, _) => Pulse { activity: Activity::Starting, since: self.spawned.unwrap_or(self.quiet_since) },
            (true, false) => Pulse { activity: Activity::Waiting, since: self.quiet_since },
            (true, true) => Pulse { activity: Activity::Working, since: self.quiet_since },
        })
    }

    /// Goes up when an item changed in place in a way that lays the transcript
    /// out differently (a tool call's input came in full), not just when items come.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Stops the turn. Anything queued goes out as soon as it has ended.
    pub fn interrupt(&mut self) {
        self.control(json!({ "subtype": "interrupt" }));
    }

    /// Whether a message can go into the turn in progress without stopping it:
    /// `claude` reads one written mid-turn at its next step (ACP agents can't).
    pub fn can_steer(&self) -> bool {
        self.busy && self.is_running() && !self.loading()
    }

    /// Sends a message into the turn in progress, which takes it in at its next
    /// step instead of after the turn; when it can't ([`Self::can_steer`]), it's
    /// sent as usual (queued while a turn runs).
    pub fn steer(&mut self, text: String, attachments: Vec<Attachment>) {
        let steering = self.can_steer();
        self.send(text, attachments);
        self.steered |= steering;
    }

    /// The queue, into the turn in progress ([`Self::steer`]); sent as usual when idle.
    pub fn steer_queue(&mut self) {
        if let Some(m) = self.queue.take_all() {
            self.steer(m.text, m.attachments);
        }
    }

    /// Sends the queue now: right away when idle, otherwise it stops the turn
    /// and the queue goes as soon as the turn has ended.
    pub fn send_queue_now(&mut self) {
        if self.busy {
            self.interrupt();
        } else if let Some(m) = self.queue.take_all() {
            self.send(m.text, m.attachments);
        }
    }

    /// Switches the permission mode, live if the process is running (like
    /// Shift+Tab in the CLI), otherwise from the next start.
    pub fn set_mode(&mut self, mode: project::Mode) {
        let before = std::mem::replace(&mut self.mode, mode);
        if self.is_running() {
            let id = self.control(json!({ "subtype": "set_permission_mode", "mode": mode.cli() }));
            // A refusal (bypass on a process not allowed it) puts the mode back.
            self.mode_requests.push((id, before));
        }
    }
}
