//! Your OK for what `knx` asks to change. A session's shell may run `knx` without asking
//! (its permission mode can allow every command), and whoever asks can claim to be any
//! session, so every request that creates or changes something waits for a card in the app
//! that you answer: **Start**, **Start in a new worktree** where it applies, or **Not now**.
//!
//! A client waits [`SHORT_WAIT`] for the answer. Not answered by then, it is told `pending`
//! with a request id and the card stays; `knx request wait <id>` or `status` collect the
//! outcome later. A card stays until it is answered or the session that asked closes.
//!
//! [`Approvals`] is shared with the threads that carry a started request's answer back, so
//! it locks inside.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use serde_json::{Value, json};

use super::Request;
use super::protocol::Place;

/// How long a client waits for the card's answer before it is told `pending`. Well under
/// the two minutes an agent's shell tool gives a command.
pub const SHORT_WAIT: Duration = Duration::from_secs(30);

/// Finished requests remembered for `request status` / `wait`, newest kept.
const KEEP_DONE: usize = 64;

/// How far up the "made by" links [`Approvals::made_by_or_under`] looks.
const MAX_DEPTH: usize = 16;

pub type Answer = Result<Value, String>;

/// What you picked on a card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    Start,
    /// A new session in a fresh worktree of its own, instead of where it was asked for.
    StartInWorktree,
    NotNow,
}

/// Whether `request` changes something, so it waits for your OK. Reading, waiting,
/// listening and moving around (showing a workspace, focusing a pane) don't.
pub fn needs_card(request: &Request) -> bool {
    !matches!(
        request,
        Request::ProjectList
            | Request::WorkspaceList
            | Request::WorkspaceOpen { .. }
            | Request::PaneList { .. }
            | Request::PaneFocus { .. }
            | Request::PaneMove { .. }
            | Request::PaneSwap { .. }
            | Request::PaneResize { .. }
            | Request::PaneZoom { .. }
            | Request::PaneRead { .. }
            | Request::PaneWait { .. }
            | Request::SessionList
            | Request::SessionRead { .. }
            | Request::SessionWait { .. }
            | Request::RequestWait { .. }
            | Request::RequestStatus { .. }
            | Request::Events
    )
}

/// Whether a card for `request` offers [`Choice::StartInWorktree`]: a new session made where
/// it was asked for, which could as well get a worktree of its own.
pub fn offers_worktree(request: &Request) -> bool {
    matches!(request, Request::SessionNew { .. })
}

/// What a card says: its title, and what goes under it (a prompt, a command) if anything.
pub fn card_text(request: &Request) -> (String, Option<String>) {
    let place = |p: &Option<Place>| match p.unwrap_or_default() {
        Place::Right => "",
        Place::Down => " (in a pane below)",
        Place::Tab => " (in a new tab)",
        Place::Hidden => " (not shown, in the sessions list)",
    };
    let folder = |p: &str| crate::paths::project_name(Path::new(p));
    match request {
        Request::ProjectCreate { path } => (format!("Make the folder {path} and open it as a project"), None),
        Request::ProjectOpen { path } => (format!("Open {path} as a project"), None),
        Request::ProjectRemove { path } => (format!("Close the project {path}"), None),
        Request::WorkspaceCreate { branch, session, prompt, why } => {
            let with = if *session { " with a session in it" } else { "" };
            (format!("Make a workspace on branch {branch}{with}{}", because(why)), prompt.clone())
        }
        Request::WorkspaceRemove { path, force: false } => (format!("Remove the workspace {}", folder(path)), None),
        Request::WorkspaceRemove { path, force: true } => {
            (format!("Remove the workspace {}, throwing away its changes", folder(path)), None)
        }
        Request::PaneSplit { .. } => ("Split the focused pane, with a new session".into(), None),
        Request::PaneClose { pane: Some(p) } => (format!("Close pane {p}"), None),
        Request::PaneClose { pane: None } => ("Close the focused pane".into(), None),
        Request::PaneRun { pane: Some(p), command, .. } => {
            (format!("Run a command in terminal pane {p}"), Some(command.clone()))
        }
        Request::PaneRun { pane: None, command, .. } => {
            ("Run a command in a new terminal pane".into(), Some(command.clone()))
        }
        Request::PanePrompt { pane, text, .. } => (format!("Send a message to agent pane {pane}"), Some(text.clone())),
        Request::SessionPrompt { session, text, .. } => {
            (format!("Send a message to session {session}"), Some(text.clone()))
        }
        Request::SessionNew { workspace, prompt, title, pane, why } => {
            let what = title
                .as_deref()
                .map_or_else(|| "Start a new session".to_string(), |t| format!("Start a new session \"{t}\""));
            let r#where = workspace.as_deref().map(|w| format!(" in {}", folder(w))).unwrap_or_default();
            (format!("{what}{where}{}{}", place(pane), because(why)), prompt.clone())
        }
        Request::SessionFork { session, at, pane } => {
            let at = at.map(|n| format!(" from before its message {n}")).unwrap_or_default();
            (format!("Fork session {session}{at}{}", place(pane)), None)
        }
        Request::SessionOpen { session, pane } => (format!("Open the past session {session}{}", place(pane)), None),
        Request::SessionClose { session } => (format!("Close session {session}"), None),
        Request::SessionRename { session, title } if title.trim().is_empty() => {
            (format!("Take the name off session {session}"), None)
        }
        Request::SessionRename { session, title } => {
            (format!("Rename session {session} to \"{}\"", title.trim()), None)
        }
        Request::SessionArchive { session } => (format!("Archive session {session}"), None),
        Request::SessionPin { session, off: false } => (format!("Pin session {session}"), None),
        Request::SessionPin { session, off: true } => (format!("Unpin session {session}"), None),
        // Never carded.
        _ => ("Change something in Kynex".into(), None),
    }
}

/// A request's reason, as the card says it after what is asked.
fn because(why: &Option<String>) -> String {
    why.as_deref().map(str::trim).filter(|w| !w.is_empty()).map(|w| format!(", to {w}")).unwrap_or_default()
}

/// Something a request made, to tell later whether the one asking to change it made it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Made {
    /// A session, by uid.
    Session(u64),
    Workspace(PathBuf),
}

/// How a request stands.
#[derive(Clone, Debug, PartialEq)]
pub enum State {
    /// Its card is up.
    Asked,
    /// You said start; it is being done.
    Running,
    Done(Answer),
    Declined,
    /// The session that asked closed before you answered.
    AskerClosed,
}

impl State {
    fn outcome(&self) -> Option<Answer> {
        match self {
            Self::Asked | Self::Running => None,
            Self::Done(a) => Some(a.clone()),
            Self::Declined => Some(Err("declined in Kynex (Not now)".into())),
            Self::AskerClosed => Some(Err("the asking session closed before the request was answered".into())),
        }
    }

    fn word(&self) -> &'static str {
        match self {
            Self::Asked => "pending",
            Self::Running => "running",
            Self::Done(Ok(_)) => "done",
            Self::Done(Err(_)) => "failed",
            Self::Declined => "declined",
            Self::AskerClosed => "asker_closed",
        }
    }
}

/// A card to show: which request, who says they ask, and what it says.
#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    pub id: u64,
    pub caller: Option<u64>,
    pub request: Request,
    /// Said on the card when the request is about something the asker didn't make.
    pub note: Option<String>,
}

struct Entry {
    id: u64,
    caller: Option<u64>,
    request: Request,
    note: Option<String>,
    state: State,
    /// The client that asked, while it still waits for the first answer.
    client: Option<Sender<Answer>>,
    /// `request wait`s: (token, reply).
    waiters: Vec<(u64, Sender<Answer>)>,
}

/// The outcome of a request whose asker was told `pending`, for the app to send it the asking
/// session as a message (the agent heard nothing more otherwise).
#[derive(Clone, Debug, PartialEq)]
pub struct Settled {
    pub caller: u64,
    pub id: u64,
    /// What was asked, as the card said it.
    pub asked: String,
    pub answer: Answer,
}

impl Settled {
    /// The message for the asking session.
    pub fn text(&self) -> String {
        match &self.answer {
            Ok(v) => format!("Your Kynex request {} ({}) was approved and done: {v}", self.id, self.asked),
            Err(e) => format!("Your Kynex request {} ({}) didn't go ahead: {e}", self.id, self.asked),
        }
    }
}

impl Entry {
    /// Settles it on `state`, telling everyone still waiting. Returns the outcome for the
    /// asking session when its client had stopped waiting (it was told `pending`).
    fn settle(&mut self, state: State) -> Option<Settled> {
        self.state = state;
        let answer = self.state.outcome()?;
        let told_pending = self.client.is_none();
        if let Some(c) = self.client.take() {
            let _ = c.send(answer.clone());
        }
        for (_, w) in self.waiters.drain(..) {
            let _ = w.send(answer.clone());
        }
        let caller = self.caller.filter(|_| told_pending && self.state != State::AskerClosed)?;
        Some(Settled { caller, id: self.id, asked: card_text(&self.request).0, answer })
    }
}

#[derive(Default)]
struct Inner {
    next: u64,
    entries: Vec<Entry>,
    /// What requests made, and the session that asked for it.
    made: HashMap<Made, u64>,
    /// Outcomes for asking sessions told `pending`, until the app takes them.
    settled: Vec<Settled>,
}

impl Inner {
    fn entry(&mut self, id: u64) -> Option<&mut Entry> {
        self.entries.iter_mut().find(|e| e.id == id)
    }

    /// Forgets the oldest finished requests past [`KEEP_DONE`].
    fn trim(&mut self) {
        let done = self.entries.iter().filter(|e| e.state.outcome().is_some()).count();
        let mut over = done.saturating_sub(KEEP_DONE);
        self.entries.retain(|e| {
            let drop = over > 0 && e.state.outcome().is_some();
            if drop {
                over -= 1;
            }
            !drop
        });
    }
}

/// The requests waiting for your OK, and those answered lately.
#[derive(Clone, Default)]
pub struct Approvals(Arc<Mutex<Inner>>);

impl Approvals {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Puts up a card for `request` from `caller`; `client` gets the outcome, or `pending`
    /// after [`Self::stop_waiting`]. `note`: said on the card (see [`Self::made_by_or_under`]).
    pub fn ask(&self, caller: Option<u64>, request: Request, note: Option<String>, client: Sender<Answer>) -> u64 {
        let mut inner = self.lock();
        inner.next += 1;
        let id = inner.next;
        inner.entries.push(Entry {
            id,
            caller,
            request,
            note,
            state: State::Asked,
            client: Some(client),
            waiters: Vec::new(),
        });
        id
    }

    /// Whether any card is up (cheap, for every frame).
    pub fn any_up(&self) -> bool {
        self.lock().entries.iter().any(|e| e.state == State::Asked)
    }

    /// The cards up, oldest first.
    pub fn cards(&self) -> Vec<Card> {
        self.lock()
            .entries
            .iter()
            .filter(|e| e.state == State::Asked)
            .map(|e| Card { id: e.id, caller: e.caller, request: e.request.clone(), note: e.note.clone() })
            .collect()
    }

    /// The short wait for request `id` is over: if it still has no answer, its client is told
    /// `pending` with the id, and stops waiting.
    pub fn stop_waiting(&self, id: u64) {
        let mut inner = self.lock();
        let Some(e) = inner.entry(id).filter(|e| e.state == State::Asked) else { return };
        if let Some(c) = e.client.take() {
            let _ = c.send(Ok(json!({
                "state": "pending",
                "request": id,
                "note": format!("waiting for the user's OK in Kynex; `knx request wait {id}` collects the outcome"),
            })));
        }
    }

    /// Your answer to card `id`. To start, returns what to do and who asked; the request is
    /// then running until [`Self::finish`]. `None` if there is nothing to do: declined, or
    /// not a card (any more).
    pub fn decide(&self, id: u64, choice: Choice) -> Option<(Request, Option<u64>)> {
        let mut inner = self.lock();
        let e = inner.entry(id).filter(|e| e.state == State::Asked)?;
        if choice == Choice::NotNow {
            let s = e.settle(State::Declined);
            inner.settled.extend(s);
            inner.trim();
            return None;
        }
        e.state = State::Running;
        Some((e.request.clone(), e.caller))
    }

    /// The outcome of running request `id`, for everyone waiting on it.
    pub fn finish(&self, id: u64, answer: Answer) {
        let mut inner = self.lock();
        if let Some(e) = inner.entry(id).filter(|e| e.state == State::Running) {
            let s = e.settle(State::Done(answer));
            inner.settled.extend(s);
        }
        inner.trim();
    }

    /// Takes down the cards of sessions that closed (`open` says which still are). Cards
    /// asked from outside any session stay.
    pub fn drop_closed(&self, open: impl Fn(u64) -> bool) {
        let mut inner = self.lock();
        for e in &mut inner.entries {
            if e.state == State::Asked && e.caller.is_some_and(|c| !open(c)) {
                let _ = e.settle(State::AskerClosed);
            }
        }
        inner.trim();
    }

    /// How request `id` stands, as `request status` answers.
    pub fn status(&self, id: u64) -> Answer {
        let mut inner = self.lock();
        let e = inner.entry(id).ok_or_else(|| no_request(id))?;
        let mut v = json!({ "request": id, "state": e.state.word() });
        match e.state.outcome() {
            Some(Ok(result)) => v["result"] = result,
            Some(Err(error)) => v["error"] = Value::String(error),
            None => {}
        }
        Ok(v)
    }

    /// `request wait`: the outcome if there is one, else `None` and `reply` gets it later
    /// (or from [`Self::give_up`] with `token`).
    pub fn wait(&self, id: u64, token: u64, reply: Sender<Answer>) -> Option<Answer> {
        let mut inner = self.lock();
        let Some(e) = inner.entry(id) else { return Some(Err(no_request(id))) };
        if let Some(a) = e.state.outcome() {
            return Some(a);
        }
        e.waiters.push((token, reply));
        None
    }

    /// A `request wait` timed out: its waiter `token` is told so, if it still waits.
    pub fn give_up(&self, id: u64, token: u64, secs: u64) {
        let mut inner = self.lock();
        let Some(e) = inner.entry(id) else { return };
        if let Some(i) = e.waiters.iter().position(|(t, _)| *t == token) {
            let (_, w) = e.waiters.remove(i);
            let _ = w.send(Err(format!("timed out after {secs}s (request {id} is still {})", e.state.word())));
        }
    }

    /// The outcomes waiting to reach their asking sessions (see [`Settled`]), oldest first.
    pub fn take_settled(&self) -> Vec<Settled> {
        std::mem::take(&mut self.lock().settled)
    }

    /// Whether any outcome waits for [`Self::take_settled`] (cheap).
    pub fn any_settled(&self) -> bool {
        !self.lock().settled.is_empty()
    }

    /// The session that asked for `made`, if a session did.
    pub fn maker(&self, made: &Made) -> Option<u64> {
        self.lock().made.get(made).copied()
    }

    /// Notes that `made` was made at the request of session `by`.
    pub fn record_made(&self, made: Made, by: Option<u64>) {
        if let Some(by) = by {
            self.lock().made.insert(made, by);
        }
    }

    /// Whether `made` was made by session `by`, or by something `by` made, and so on.
    pub fn made_by_or_under(&self, made: &Made, by: u64) -> bool {
        let inner = self.lock();
        let mut at = made.clone();
        for _ in 0..MAX_DEPTH {
            match inner.made.get(&at) {
                Some(&maker) if maker == by => return true,
                Some(&maker) => at = Made::Session(maker),
                None => return false,
            }
        }
        false
    }
}

fn no_request(id: u64) -> String {
    format!("no request {id} (it may be long finished)")
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::channel;

    use super::super::protocol::SessionRef;
    use super::*;

    fn new_session() -> Request {
        Request::SessionNew { workspace: None, prompt: Some("go".into()), title: None, pane: None, why: None }
    }

    #[test]
    fn what_changes_something_needs_a_card() {
        assert!(needs_card(&new_session()));
        assert!(needs_card(&Request::WorkspaceRemove { path: "/w".into(), force: false }));
        assert!(needs_card(&Request::PaneRun { pane: None, command: "ls".into(), wait: false, timeout: None }));
        assert!(needs_card(&Request::SessionPrompt {
            session: SessionRef::Uid(1),
            text: "x".into(),
            wait: false,
            timeout: None
        }));
        assert!(!needs_card(&Request::SessionList));
        assert!(!needs_card(&Request::SessionRead { session: SessionRef::Uid(1) }));
        assert!(!needs_card(&Request::RequestStatus { request: 1 }));
        assert!(!needs_card(&Request::PaneFocus { pane: 1 }));
        assert!(!needs_card(&Request::PaneMove { pane: 1, to: super::super::protocol::PaneTarget::NewTab }));
        assert!(!needs_card(&Request::PaneSwap { pane: 1, other: 2 }));
        assert!(!needs_card(&Request::PaneZoom { pane: 1, off: false }));
        assert!(offers_worktree(&new_session()));
        assert!(!offers_worktree(&Request::SessionClose { session: SessionRef::Uid(1) }));
    }

    #[test]
    fn cards_say_what_is_asked() {
        let (title, detail) = card_text(&Request::SessionNew {
            workspace: Some("/p/app-wt".into()),
            prompt: Some("fix it".into()),
            title: Some("Fix".into()),
            pane: Some(Place::Hidden),
            why: Some("run the tests".into()),
        });
        assert!(title.contains("\"Fix\"") && title.contains("app-wt") && title.contains("not shown"), "{title}");
        assert!(title.ends_with(", to run the tests"), "{title}");
        assert_eq!(detail.as_deref(), Some("fix it"));
        let (title, _) = card_text(&Request::WorkspaceRemove { path: "/p/w".into(), force: true });
        assert!(title.contains("throwing away"), "{title}");
        let (title, _) = card_text(&Request::SessionFork { session: SessionRef::Uid(3), at: Some(2), pane: None });
        assert!(title.contains("before its message 2"), "{title}");
    }

    #[test]
    fn answered_in_time_the_client_gets_the_outcome() {
        let a = Approvals::default();
        let (tx, rx) = channel();
        let id = a.ask(Some(7), new_session(), None, tx);
        assert_eq!(a.cards().len(), 1);
        assert_eq!(a.status(id).unwrap()["state"], "pending");
        let (req, caller) = a.decide(id, Choice::Start).unwrap();
        assert_eq!((req, caller), (new_session(), Some(7)));
        assert!(a.cards().is_empty(), "answered: the card goes");
        assert_eq!(a.status(id).unwrap()["state"], "running");
        a.stop_waiting(id); // too late to say pending: it's running
        assert!(rx.try_recv().is_err());
        a.finish(id, Ok(json!({ "session": 9 })));
        assert_eq!(rx.try_recv().unwrap(), Ok(json!({ "session": 9 })));
        assert_eq!(a.status(id).unwrap()["result"]["session"], 9);
        assert!(a.decide(id, Choice::Start).is_none(), "once");
    }

    #[test]
    fn unanswered_the_client_is_told_pending_and_waits_later() {
        let a = Approvals::default();
        let (tx, rx) = channel();
        let id = a.ask(None, new_session(), None, tx);
        a.stop_waiting(id);
        let pending = rx.try_recv().unwrap().unwrap();
        assert_eq!((pending["state"].as_str(), pending["request"].as_u64()), (Some("pending"), Some(id)));
        let (w1, r1) = channel();
        let (w2, r2) = channel();
        assert!(a.wait(id, 1, w1).is_none());
        assert!(a.wait(id, 2, w2).is_none());
        a.give_up(id, 2, 5);
        assert!(r2.try_recv().unwrap().unwrap_err().contains("timed out after 5s"));
        a.decide(id, Choice::Start);
        a.finish(id, Ok(json!({ "ok": 1 })));
        assert_eq!(r1.try_recv().unwrap(), Ok(json!({ "ok": 1 })));
        assert!(rx.try_recv().is_err(), "the first client was answered already");
        assert_eq!(a.wait(id, 3, channel().0), Some(Ok(json!({ "ok": 1 }))));
    }

    #[test]
    fn declined_nothing_is_done() {
        let a = Approvals::default();
        let (tx, rx) = channel();
        let id = a.ask(Some(1), new_session(), None, tx);
        assert!(a.decide(id, Choice::NotNow).is_none());
        assert!(rx.try_recv().unwrap().unwrap_err().contains("declined"));
        assert_eq!(a.status(id).unwrap()["state"], "declined");
        assert!(a.cards().is_empty());
        assert!(a.decide(id, Choice::Start).is_none(), "declined stays declined");
    }

    #[test]
    fn a_card_goes_when_its_asker_closes() {
        let a = Approvals::default();
        let (tx, rx) = channel();
        let gone = a.ask(Some(1), new_session(), None, tx);
        let kept = a.ask(Some(2), new_session(), None, channel().0);
        let outside = a.ask(None, new_session(), None, channel().0);
        a.drop_closed(|uid| uid == 2);
        assert!(rx.try_recv().unwrap().unwrap_err().contains("closed"));
        assert_eq!(a.status(gone).unwrap()["state"], "asker_closed");
        let up: Vec<u64> = a.cards().iter().map(|c| c.id).collect();
        assert_eq!(up, [kept, outside]);
    }

    #[test]
    fn an_asker_told_pending_hears_the_outcome_later() {
        let a = Approvals::default();
        // Answered in time: the client had it, nothing more to say.
        let quick = a.ask(Some(1), new_session(), None, channel().0);
        a.decide(quick, Choice::Start);
        a.finish(quick, Ok(json!({ "session": 2 })));
        assert!(a.take_settled().is_empty());
        // Told pending, then started: the outcome waits for the app.
        let slow = a.ask(Some(1), new_session(), None, channel().0);
        a.stop_waiting(slow);
        a.decide(slow, Choice::Start);
        assert!(!a.any_settled(), "running: no outcome yet");
        a.finish(slow, Ok(json!({ "session": 3 })));
        let got = a.take_settled();
        assert_eq!((got.len(), got[0].caller, got[0].id), (1, 1, slow));
        assert!(got[0].text().contains("approved and done"), "{}", got[0].text());
        // Told pending, then declined.
        let no = a.ask(Some(4), new_session(), None, channel().0);
        a.stop_waiting(no);
        a.decide(no, Choice::NotNow);
        assert!(a.take_settled()[0].text().contains("didn't go ahead"));
        // From a script, or an asker gone: nobody to tell.
        let script = a.ask(None, new_session(), None, channel().0);
        a.stop_waiting(script);
        a.decide(script, Choice::NotNow);
        let gone = a.ask(Some(5), new_session(), None, channel().0);
        a.stop_waiting(gone);
        a.drop_closed(|_| false);
        assert!(a.take_settled().is_empty());
    }

    #[test]
    fn unknown_requests_say_so() {
        let a = Approvals::default();
        assert!(a.status(42).unwrap_err().contains("no request 42"));
        assert!(a.wait(42, 1, channel().0).unwrap().is_err());
    }

    #[test]
    fn what_a_session_made_and_what_that_made_count_as_its_own() {
        let a = Approvals::default();
        a.record_made(Made::Session(10), Some(1));
        a.record_made(Made::Session(11), Some(10));
        a.record_made(Made::Workspace("/w".into()), Some(11));
        a.record_made(Made::Session(12), None);
        assert!(a.made_by_or_under(&Made::Session(10), 1));
        assert!(a.made_by_or_under(&Made::Workspace("/w".into()), 1));
        assert!(!a.made_by_or_under(&Made::Session(10), 2));
        assert!(!a.made_by_or_under(&Made::Session(12), 1), "made from outside: nobody's");
        // A loop in the links ends.
        a.record_made(Made::Session(20), Some(21));
        a.record_made(Made::Session(21), Some(20));
        assert!(!a.made_by_or_under(&Made::Session(20), 1));
    }

    #[test]
    fn old_finished_requests_are_forgotten() {
        let a = Approvals::default();
        let first = a.ask(None, new_session(), None, channel().0);
        a.decide(first, Choice::NotNow);
        for _ in 0..KEEP_DONE {
            let id = a.ask(None, new_session(), None, channel().0);
            a.decide(id, Choice::NotNow);
        }
        assert!(a.status(first).is_err());
        let up = a.ask(None, new_session(), None, channel().0);
        assert_eq!(a.status(up).unwrap()["state"], "pending", "cards up are never dropped");
    }
}
