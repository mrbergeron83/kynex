//! Reading a session's log back into its transcript: on reopening it, on a
//! fork, and after a rewind.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;

use serde_json::Value;

use super::*;

/// A session log being read back on a thread: its newest turns, then the rest.
pub(super) struct Loader {
    pub(super) rx: Receiver<Loaded>,
    /// The newest turns are in.
    pub(super) shown: bool,
}

pub(super) enum Loaded {
    Recent(Replayed),
    Older(Vec<Item>),
}

/// Log entries replayed into a transcript, and what they say about the session.
pub(super) struct Replayed {
    pub(super) items: Vec<Item>,
    pub(super) context: ContextUse,
    pub(super) reply_model: String,
    pub(super) tasks: Tasks,
    pub(super) finished: bool,
}

impl Replayed {
    /// `entries` of log `path` replayed, oldest first. Their images stay in
    /// the log, as [`LoggedImage`]s, when the log is known.
    pub(super) fn of(path: Option<&Arc<PathBuf>>, entries: impl IntoIterator<Item = (Span, Value)>) -> Self {
        let mut s = Session::new(Kind::Chat, PathBuf::new());
        for (span, v) in entries {
            s.origin = path.map(|p| (p.clone(), span));
            s.replay_entry(v);
        }
        s.origin = None;
        Self {
            items: std::mem::take(&mut s.items),
            context: s.context,
            reply_model: std::mem::take(&mut s.reply_model),
            tasks: std::mem::take(&mut s.tasks),
            finished: s.finished,
        }
    }
}

/// Entries to show at once when a session opens; the walk goes on to the start
/// of their turn, and the rest of the history follows.
pub(super) const RECENT_ENTRIES: usize = 60;

/// Reads session `id`'s log back (see [`crate::claude::session_log::Branch`]) into
/// `tx`. Nothing is sent if there's no log; it stops if `tx` closes.
pub(super) fn read_history(cwd: &Path, id: &str, until: Option<&str>, tx: &Sender<Loaded>, wake: &Wake) {
    use crate::claude::session_log::{Branch, read_span};
    let send = |l: Loaded| {
        let sent = tx.send(l).is_ok();
        wake();
        sent
    };
    let Some(path) = crate::sessions::find_claude_transcript(cwd, id) else { return };
    let Ok(mut file) = std::fs::File::open(&path) else { return };
    let path = Some(Arc::new(path));
    let walked = Branch { recent: RECENT_ENTRIES }.walk(&mut file, until, |mut recent| {
        recent.reverse();
        send(Loaded::Recent(Replayed::of(path.as_ref(), recent)))
    });
    let Ok(Some(walked)) = walked else { return };
    if walked.older.is_empty() {
        return;
    }
    // One entry at a time: only the transcript's text is kept, not the lines.
    let mut buf = Vec::new();
    let older = walked.older.iter().rev().filter_map(|s| Some((*s, read_span(&mut file, *s, &mut buf)?)));
    send(Loaded::Older(Replayed::of(path.as_ref(), older).items));
}

impl Session {
    /// Reopen a previous conversation: replay its transcript from Claude Code's
    /// own session log, and `--resume` it on the next message. The log is read
    /// on a thread of its own, newest turns first, so this returns at once;
    /// `wake` rings as they come in, for [`Self::take_history`].
    pub fn resume(kind: Kind, cwd: PathBuf, id: String, wake: Wake) -> Self {
        let mut s = Self::new(kind, cwd);
        // What it cost before it was closed: the CLI's counter starts over (#341).
        s.carried_cost = crate::claude::costs::load(&id).total_usd;
        s.total_cost = s.carried_cost;
        s.session_id = Some(id);
        s.reopened = true;
        s.conversation = true;
        s.wake = wake;
        s.load(None);
        s
    }

    /// A copy of session `from` to take somewhere else (`/fork`): it shows its
    /// conversation, and its process continues it as a new session with
    /// `--fork-session`; `from` stays as it is. It has no id of its own until its
    /// process reports one.
    pub fn fork(kind: Kind, cwd: PathBuf, from: String, wake: Wake) -> Self {
        let mut s = Self::new(kind, cwd);
        s.fork_from = Some(from);
        s.reopened = true;
        s.wake = wake;
        s.load(None);
        s
    }

    /// The session a fork continues, until its first message is sent.
    pub fn forked_from(&self) -> Option<&str> {
        self.fork_from.as_deref()
    }

    /// Starts filling the transcript from the session log's current branch, up
    /// to and including entry `until` if given.
    pub(super) fn load(&mut self, until: Option<String>) {
        let (tx, rx) = channel();
        // An unsent fork's conversation is its original's, whatever id its process reported.
        let id = self.fork_from.clone().or_else(|| self.session_id.clone()).unwrap_or_default();
        let (cwd, wake) = (self.cwd.clone(), self.wake.clone());
        thread::spawn(move || {
            read_history(&cwd, &id, until.as_deref(), &tx, &wake);
            // The last ring comes after the channel closes, so the app sees it closed.
            drop(tx);
            wake();
        });
        self.loader = Some(Loader { rx, shown: false });
    }

    /// The log is still being read and its newest turns aren't in yet:
    /// messages wait in the queue until they are.
    pub fn loading(&self) -> bool {
        self.loader.as_ref().is_some_and(|l| !l.shown)
    }

    /// The whole log is in, older turns too (or there was none to read): the
    /// transcript's first message is the session's first.
    pub fn history_loaded(&self) -> bool {
        self.loader.is_none()
    }

    /// Takes in the history read back since last time. Returns how many items
    /// came in front of the ones there were, which moved down by as many, or
    /// `None` if nothing came. Call it when woken, before [`Self::poll`].
    pub fn take_history(&mut self) -> Option<usize> {
        let loader = self.loader.as_mut()?;
        let mut came = Vec::new();
        let done = loop {
            match loader.rx.try_recv() {
                Ok(l) => came.push(l),
                Err(std::sync::mpsc::TryRecvError::Empty) => break false,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => break true,
            }
        };
        let shown = loader.shown || came.iter().any(|l| matches!(l, Loaded::Recent(_)));
        loader.shown = shown;
        if done {
            self.loader = None;
        }
        let mut added = None;
        for l in came {
            let n = match l {
                Loaded::Recent(r) => {
                    self.context = r.context;
                    self.reply_model = r.reply_model;
                    self.tasks = r.tasks;
                    self.finished |= r.finished;
                    r.items
                }
                Loaded::Older(items) => items,
            };
            let after = std::mem::replace(&mut self.items, n);
            added = Some(added.unwrap_or(0) + self.items.len());
            self.items.extend(after);
        }
        if done && !shown {
            self.items.insert(0, Item::Error("previous transcript not found; continuing the session anyway".into()));
            added = Some(added.unwrap_or(0) + 1);
        }
        // Messages written while it loaded go out now.
        if added.is_some() && !self.loading() && !self.busy {
            self.send_queue_now();
        }
        added
    }

    /// Goes back to just before `point`: its transcript ends there and the next
    /// message continues from there, leaving out what came after (it stays in
    /// the log, on its own branch). Back to the first message, it starts over
    /// as a new session. Stops the process; the log is read back as on [`Self::resume`].
    pub fn rewind_to(&mut self, point: &crate::claude::rewind::Point) {
        self.kill();
        self.items.clear();
        self.revision += 1;
        self.perm_gen += 1;
        self.context = ContextUse::default();
        self.queue = Queue::default();
        self.finished = false;
        self.loader = None;
        match &point.before {
            Some(before) => {
                self.load(Some(before.clone()));
                self.resume_at = Some(before.clone());
            }
            None => {
                self.session_id = None;
                self.conversation = false;
                self.resume_at = None;
                // Back to the start: a fork that sent nothing has nothing to continue.
                self.fork_from = None;
            }
        }
    }

    /// One entry of a session log, into the transcript.
    pub(crate) fn replay_entry(&mut self, v: Value) {
        if v["isMeta"] == true {
            return;
        }
        if v["type"] == "user" && !written_by_cli(&v) {
            self.replay_user(&v["message"]["content"]);
        } else if let Some(n) = v["message"]["content"].as_str().and_then(tasks::parse_notification) {
            self.items.push(Item::Task(n));
        }
        self.handle(v);
    }

    pub(super) fn replay_user(&mut self, content: &Value) {
        // `!` commands and their output, the CLI's or sent with a message, show as such.
        let (text, sent) = match content {
            Value::String(t) if self.replay_shell(t) => return,
            Value::String(t) => (t.clone(), 0),
            Value::Array(blocks) => (
                blocks
                    .iter()
                    .filter_map(|b| b["text"].as_str())
                    .filter(|t| !self.replay_shell(t))
                    .collect::<Vec<_>>()
                    .join("\n"),
                blocks.iter().filter(|b| b["type"] == "image").count(),
            ),
            _ => return,
        };
        if text.is_empty() && sent == 0 {
            return;
        }
        // A slash command is logged in `<command-name>` wrappers; show it as typed.
        let text = crate::claude::commands::unwrap(&text).unwrap_or(text);
        let images = match &self.origin {
            Some(origin) => logged_in(content, origin, "/message/content"),
            None => previews_in(content),
        };
        // Images that can't be shown are at least counted.
        let missing = sent.saturating_sub(images.len());
        let text = if missing > 0 { format!("[{missing} image(s)] {text}") } else { text };
        self.items.push(Item::User { text, images });
    }
}
