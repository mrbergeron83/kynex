//! Each session's half-written message: the composer's text and attachments are
//! put aside when another session is shown and come back with this one. Text
//! is saved in `state.json` by CLI session id, so it survives a restart.

use gpui::Context;
use kynex_core::live::Key;
use kynex_core::session::{Attachment, Session};

use super::KynexApp;
use super::state::Active;

/// A message being written in a session that isn't shown.
#[derive(Default)]
pub(super) struct Draft {
    text: String,
    attachments: Vec<Attachment>,
}

/// Where a session's draft is kept: its CLI id, or its uid until it has one.
fn draft_key(s: &Session) -> String {
    match &s.session_id {
        Some(id) => id.clone(),
        None => format!("#{}", s.uid),
    }
}

impl KynexApp {
    /// Swaps the composer's contents when the shown session changes. Called
    /// every render, so every way of switching (tabs, the sessions panel,
    /// new, close) is covered.
    pub(super) fn sync_draft(&mut self, key: Option<Key>, cx: &mut Context<Self>) {
        let Some(s) = key.and_then(|k| self.session_by_key(k)) else { return };
        let (uid, key) = (s.uid, draft_key(s));
        match &mut self.pane.draft_for {
            // Same session; it may just have got its CLI id.
            Some((shown, k)) if *shown == uid => *k = key,
            _ => {
                self.stash_draft(cx);
                let draft = self.drafts.remove(&key).unwrap_or_default();
                let text = self.saved.drafts.remove(&key).unwrap_or(draft.text);
                self.pane.composer.update(cx, |c, cx| c.set_text(text, cx));
                self.pane.attachments = draft.attachments;
                self.pane.draft_for = Some((uid, key));
            }
        }
    }

    /// Puts the shown session's draft aside, and saves its text.
    pub(super) fn stash_draft(&mut self, cx: &mut Context<Self>) {
        let Some((_, key)) = self.pane.draft_for.take() else { return };
        let text = self.pane.composer.read(cx).text().to_string();
        let attachments = std::mem::take(&mut self.pane.attachments);
        if !key.starts_with('#') {
            if text.trim().is_empty() {
                self.saved.drafts.remove(&key);
            } else {
                self.saved.drafts.insert(key.clone(), text.clone());
            }
            self.save();
        }
        if !text.trim().is_empty() || !attachments.is_empty() {
            self.drafts.insert(key, Draft { text, attachments });
        }
    }

    /// Closes the new sessions left in the shown folder: not shown, in no pane, no draft
    /// put aside. Runs after [`Self::sync_draft`], so a draft just left is counted.
    pub(super) fn close_unused_sessions(&mut self) {
        let (drafts, saved) = (&self.drafts, &self.saved.drafts);
        let live = match self.saved.active {
            Active::Chat => Some(&mut self.chat),
            Active::Project(i) => self.saved.projects.get(i).and_then(|p| self.code.get_mut(&p.path)),
        };
        let Some(live) = live else { return };
        let closed = live.close_unused(|s| {
            let key = draft_key(s);
            drafts.contains_key(&key) || saved.contains_key(&key)
        });
        self.forget(&closed);
    }

    /// Drops the drafts of sessions that closed before they had a CLI id;
    /// there's no transcript to come back to.
    pub(super) fn forget_drafts(&mut self, closed: &[Key]) {
        for k in closed {
            self.drafts.remove(&format!("#{k}"));
        }
    }
}
