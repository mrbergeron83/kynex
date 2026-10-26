//! Permission, plan and question prompts: keeps a [`PromptView`] in the
//! composer's place for the active session's oldest pending request, and
//! sends the answer back.

use std::path::Path;

use gpui::{Context, Entity, Focusable, Subscription, Window, prelude::*};
use kynex_core::live::Key;
use kynex_core::project::Permissions;
use kynex_core::session::{self, Item, PermState};

use super::KynexApp;
use crate::prompt::{PromptAnswer, PromptView, Request};

pub struct ActivePrompt {
    key: Key,
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
                self.notice = Some(e);
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
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut rules = Vec::new();
        let mut cwd = None;
        if let Some(s) = self.session_for(key) {
            let ix =
                s.items.iter().position(|i| matches!(i, Item::Permission { request_id: r, .. } if *r == request_id));
            if let Some(ix) = ix {
                rules = s.answer_permission(ix, answer);
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

    /// Keeps the prompt view in step with the active session's oldest pending request.
    pub(super) fn sync_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let pending = self.active_session().and_then(|s| {
            let mut it = s.items.iter().filter_map(|i| match i {
                Item::Permission { request_id, tool, input, suggestions, description, state: PermState::Pending } => {
                    Some(Request {
                        id: request_id.clone(),
                        tool: tool.clone(),
                        input: input.clone(),
                        suggestions: suggestions.clone(),
                        description: description.clone(),
                        kind: s.kind,
                    })
                }
                _ => None,
            });
            let first = it.next()?;
            Some((s.uid, first, it.count()))
        });
        let Some((key, req, queued)) = pending else {
            self.prompt = None;
            return;
        };
        if self.prompt.as_ref().is_none_or(|p| p.key != key || p.id != req.id) {
            let id = req.id.clone();
            let view = cx.new(|cx| PromptView::new(req, window, cx));
            let sub = cx.subscribe_in(&view, window, {
                let id = id.clone();
                move |this, _, PromptAnswer(answer), window, cx| {
                    this.answer(key, id.clone(), answer.clone(), window, cx)
                }
            });
            self.prompt = Some(ActivePrompt { key, id, view, _sub: sub });
        }
        if let Some(p) = &self.prompt {
            p.view.update(cx, |v, _| v.queued = queued);
        }
    }
}
