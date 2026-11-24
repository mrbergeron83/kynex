//! Permission, plan and question prompts: keeps a [`PromptView`] in the
//! composer's place for the session's oldest pending request, and sends the
//! answer back.

use gpui::{Context, Entity, Focusable, Subscription, Window, prelude::*};
use kynex_core::claude::session::{self, Item, PermState};
use kynex_core::project::Permissions;

use kynex_core::toasts::Toast;

use super::{ClaudePane, Key};
use crate::prompt::{PromptAnswer, PromptView, Request};

/// A prompt up in the pane: whose request it is, and its view.
pub struct ActivePrompt {
    pub key: Key,
    id: String,
    pub view: Entity<PromptView>,
    _sub: Subscription,
}

impl ClaudePane {
    /// Adds "always allow in this project" rules to the project's Kynex settings.
    fn save_rules(&mut self, rules: Vec<String>, cx: &mut Context<Self>) {
        if rules.is_empty() {
            return;
        }
        // A session in a workspace allows its project's rules, not its folder's own.
        let dir = self.session.settings_folder().to_path_buf();
        let mut perms = Permissions::load(&dir);
        if perms.allow(rules) {
            // The live process has them as session rules already; later ones read them from the file.
            if let Err(e) = perms.save(&dir) {
                self.toast(Toast::error(e), cx);
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
        if let Some(s) = self.session_for(key) {
            let ix =
                s.items.iter().position(|i| matches!(i, Item::Permission { request_id: r, .. } if *r == request_id));
            if let Some(ix) = ix {
                rules = s.answer_permission_with(ix, answer, comment);
            }
        }
        self.save_rules(rules, cx);
        self.prompt = None;
        self.composer.read(cx).focus_handle(cx).focus(window, cx);
        self.sync(cx);
        cx.notify();
    }

    /// Keeps the prompt view in step with the oldest pending request of the session.
    pub(super) fn sync_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // `waiting()` is cached, so the usual answer, nothing pending, costs no scan.
        let s = &self.session;
        let pending = s.waiting().then_some(s).and_then(|s| {
            let mut it = s.items.iter().filter_map(|i| request_for(s, i));
            let first = it.next()?;
            Some((s.uid, first, it.count()))
        });
        let Some((key, req, queued)) = pending else {
            self.prompt = None;
            return;
        };
        if self.prompt.as_ref().is_none_or(|p| p.key != key || p.id != req.id) {
            let id = req.id.clone();
            // Only a prompt for the pane you're in takes the keyboard.
            let take_focus = self.focus.contains_focused(window, cx);
            let view = cx.new(|cx| PromptView::new(req, take_focus, window, cx));
            let sub = cx.subscribe_in(&view, window, {
                let id = id.clone();
                move |this, _, PromptAnswer(answer, comment), window, cx| {
                    this.answer(key, id.clone(), answer.clone(), comment, window, cx)
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
