//! Permission, plan and question prompts: keeps a [`PromptView`] in the
//! composer's place for the active session's oldest pending request, and
//! sends the answer back.

use std::path::Path;

use gpui::{Context, Entity, Focusable, Subscription, Window, prelude::*};
use kynex_core::live::Key;
use kynex_core::project::Permissions;
use kynex_core::session::{self, Item, PermState};

use kynex_core::toasts::Toast;

use super::KynexApp;
use crate::prompt::{PromptAnswer, PromptView, Request};

pub struct ActivePrompt {
    pub key: Key,
    id: String,
    pub view: Entity<PromptView>,
    _sub: Subscription,
}

impl KynexApp {
    /// Adds "always allow in this project" rules to the project's Kynex settings.
    fn save_rules(&mut self, cwd: &Path, rules: Vec<String>) {
        if rules.is_empty() {
            return;
        }
        let mut perms = Permissions::load(cwd);
        if perms.allow(rules) {
            if let Err(e) = perms.save(cwd) {
                self.toast(Toast::error(e));
            }
            if let Some(s) = &mut self.settings {
                s.perms = perms;
            }
        }
    }

    fn answer(
        &mut self,
        key: Key,
        request_id: String,
        answer: session::Answer,
        comment: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut rules = Vec::new();
        let mut cwd = None;
        if let Some(s) = self.session_for(key) {
            let ix =
                s.items.iter().position(|i| matches!(i, Item::Permission { request_id: r, .. } if *r == request_id));
            if let Some(ix) = ix {
                rules = s.answer_permission_with(ix, answer, comment);
                cwd = Some(s.cwd.clone());
            }
        }
        if let Some(cwd) = cwd {
            self.save_rules(&cwd, rules);
        }
        self.prompt = None;
        self.composer.read(cx).focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    /// Keeps the prompt view in step with the oldest pending request: in KNX Ops
    /// the stack's question (a worker's, which blocks the chief's composer until
    /// answered), else the active session's.
    pub(super) fn sync_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let stacked = if self.in_chief() { self.chief_focus() } else { None };
        // The request to show, its session (a stacked question has the worker's), how many wait
        // behind it, and the question's number when it's a stacked one.
        let (pending, ops) = if let Some(ask) = stacked {
            let queued = self.question_stack().len().saturating_sub(1);
            let key = self.chief.workers.get(&ask.path).copied().unwrap_or_default();
            let req = Request {
                id: format!("ops-{}", ask.id),
                tool: "AskUserQuestion".into(),
                input: ask.input,
                suggestions: serde_json::Value::Null,
                description: String::new(),
                why: String::new(),
                kind: session::Kind::Chat,
            };
            (Some((key, req, queued)), Some(ask.id))
        } else {
            let pending = self.active_session().and_then(|s| {
                let mut it = s.items.iter().filter_map(|i| request_for(s, i));
                let first = it.next()?;
                Some((s.uid, first, it.count()))
            });
            (pending, None)
        };
        let Some((key, req, queued)) = pending else {
            self.prompt = None;
            return;
        };
        if self.prompt.as_ref().is_none_or(|p| p.key != key || p.id != req.id) {
            let id = req.id.clone();
            let view = cx.new(|cx| PromptView::new(req, window, cx));
            let sub = cx.subscribe_in(&view, window, {
                let id = id.clone();
                move |this, _, PromptAnswer(answer, comment), window, cx| match ops {
                    // A worker's question waits on Kynex, not on a session's prompt.
                    Some(n) => {
                        this.answer_ask(n, answer);
                        this.prompt = None;
                        this.composer.read(cx).focus_handle(cx).focus(window, cx);
                        cx.notify();
                    }
                    None => this.answer(key, id.clone(), answer.clone(), comment, window, cx),
                }
            });
            self.prompt = Some(ActivePrompt { key, id, view, _sub: sub });
        }
        if let Some(p) = &self.prompt {
            p.view.update(cx, |v, _| v.queued = queued);
        }
    }
}

/// The prompt for a pending permission or question in `s`, if `item` is one.
fn request_for(s: &session::Session, item: &Item) -> Option<Request> {
    match item {
        Item::Permission { request_id, tool, input, suggestions, description, why, state: PermState::Pending } => {
            Some(Request {
                id: request_id.clone(),
                tool: tool.clone(),
                input: input.clone(),
                suggestions: suggestions.clone(),
                description: description.clone(),
                why: why.clone(),
                kind: s.kind,
            })
        }
        _ => None,
    }
}
