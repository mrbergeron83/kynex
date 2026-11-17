//! The card you answer before a `knx` request changes something (`kynex_core::control::
//! approval`): put up in the asking session's pane, in the composer's place like a permission
//! prompt, or in the focused pane when that session isn't on screen or a script asked. Its
//! answer starts the request, whose outcome goes to whoever still waits for it.

use std::collections::HashSet;
use std::sync::mpsc::{Sender, channel};

use gpui::{Context, Window};
use kynex_core::control::Request;
use kynex_core::control::approval::{self, Card, Choice, Made, SHORT_WAIT};
use kynex_core::live::Key;
use kynex_core::session::{Answer as PromptChoice, Kind};
use serde_json::{Value, json};

use super::KynexApp;
use crate::prompt::{self, Request as PromptRequest};

type Answer = Result<Value, String>;

impl KynexApp {
    /// Puts up a card for `request`; `reply` gets the outcome, or `pending` after the short wait.
    pub(super) fn ask_card(
        &mut self,
        request: Request,
        caller: Option<Key>,
        reply: Sender<Answer>,
        cx: &mut Context<Self>,
    ) {
        if caller.is_some() && matches!(request, Request::WorkspaceRemove { force: true, .. }) {
            let _ = reply
                .send(Err("`workspace remove --force` isn't offered to a session: ask the user to remove it".into()));
            return;
        }
        let note = self.card_note(&request, caller, cx);
        let id = self.approvals.ask(caller, request, note, reply);
        let approvals = self.approvals.clone();
        cx.spawn(async move |_, cx| {
            cx.background_executor().timer(SHORT_WAIT).await;
            approvals.stop_waiting(id);
        })
        .detach();
        cx.notify();
    }

    /// What the card says when the request is about something the asking session didn't make
    /// (itself, or what it or what it made asked for).
    fn card_note(&mut self, request: &Request, caller: Option<Key>, cx: &mut Context<Self>) -> Option<String> {
        let c = caller?;
        let made = match request {
            Request::SessionClose { session }
            | Request::SessionRename { session, .. }
            | Request::SessionArchive { session }
            | Request::SessionPin { session, .. }
            | Request::SessionPrompt { session, .. } => match self.resolve_session(session) {
                Ok(key) if key == c => return None,
                Ok(key) => Some(Made::Session(key)),
                Err(_) => None,
            },
            Request::WorkspaceRemove { path, .. } => {
                self.workspace_named(path, cx).ok().map(|(_, folder)| Made::Workspace(folder))
            }
            _ => return None,
        };
        let theirs = made.is_some_and(|m| self.approvals.made_by_or_under(&m, c));
        (!theirs).then(|| format!("Session {c} didn't start this: it isn't one of its own."))
    }

    /// `request wait`: the outcome now if there is one, else when it comes or `timeout` runs out.
    pub(super) fn request_wait(
        &mut self,
        id: u64,
        timeout: Option<u64>,
        reply: Sender<Answer>,
        cx: &mut Context<Self>,
    ) {
        self.approval_token += 1;
        let token = self.approval_token;
        if let Some(a) = self.approvals.wait(id, token, reply.clone()) {
            let _ = reply.send(a);
            return;
        }
        if let Some(secs) = timeout {
            let approvals = self.approvals.clone();
            cx.spawn(async move |_, cx| {
                cx.background_executor().timer(std::time::Duration::from_secs(secs)).await;
                approvals.give_up(id, token, secs);
            })
            .detach();
        }
    }

    /// Takes down the cards of sessions that closed.
    pub(super) fn drop_closed_cards(&mut self) {
        if !self.approvals.any_up() {
            return;
        }
        let open: HashSet<Key> = self.all_sessions().map(|s| s.uid).collect();
        self.approvals.drop_closed(|k| open.contains(&k));
    }

    /// The card to show in the pane of session `shown`: one its session asked, or, in the
    /// focused pane, one whose asker isn't on screen (or that no session asked).
    pub(super) fn card_for(&self, shown: Key) -> Option<Card> {
        if !self.approvals.any_up() {
            return None;
        }
        let focused = self.focused_key() == Some(shown);
        let on_screen = |k: Key| {
            self.tiling_scope()
                .and_then(|(_, ws, _)| self.tiles.get(&ws))
                .is_some_and(|t| t.visible().iter().any(|(_, key)| *key == k))
        };
        self.approvals
            .cards()
            .into_iter()
            .find(|c| c.caller == Some(shown) || focused && !c.caller.is_some_and(|k| k != shown && on_screen(k)))
    }

    /// The prompt view's request for `card`.
    pub(super) fn card_prompt(&self, card: &Card, kind: Kind) -> PromptRequest {
        let (title, detail) = approval::card_text(&card.request);
        let asker = match card.caller {
            Some(k) => match self.session_title(k) {
                Some(t) => format!("Session {k} ({t}) asks:"),
                None => format!("Session {k} asks:"),
            },
            None => "A command outside any session asks:".to_string(),
        };
        PromptRequest {
            id: format!("knx-{}", card.id),
            tool: prompt::CARD_TOOL.into(),
            input: json!({
                "title": title,
                "detail": detail,
                "asker": asker,
                "note": card.note,
                "worktree": approval::offers_worktree(&card.request),
            }),
            suggestions: Value::Null,
            description: String::new(),
            why: String::new(),
            kind,
        }
    }

    /// Your answer to card `id`: Start does the request (in a new worktree, for its second
    /// choice), anything else is Not now.
    pub(super) fn answer_card(&mut self, id: u64, answer: &PromptChoice, window: &mut Window, cx: &mut Context<Self>) {
        let choice = match answer {
            PromptChoice::Once => Choice::Start,
            PromptChoice::Session => Choice::StartInWorktree,
            _ => Choice::NotNow,
        };
        self.pane.prompt = None;
        self.focus_input(window, cx);
        cx.notify();
        let Some((request, caller)) = self.approvals.decide(id, choice) else { return };
        let (tx, rx) = channel();
        let approvals = self.approvals.clone();
        // The outcome may come much later (a prompt that waits for its turn): a thread holds on.
        std::thread::spawn(move || {
            let answer = rx.recv().unwrap_or_else(|_| Err("Kynex dropped the request".into()));
            approvals.finish(id, answer);
        });
        self.dispatch(request, caller, choice == Choice::StartInWorktree, tx, window, cx);
    }
}
