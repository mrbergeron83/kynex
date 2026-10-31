//! Names for sessions that Claude Code didn't name: one at a time, in the
//! background. The shown session gets one; past sessions only if the user
//! turned that on in Settings, since each name is a call on their quota.

use gpui::{Context, prelude::*};
use kynex_core::session::SessionInfo;
use kynex_core::titles;

use super::KynexApp;

/// With past sessions named, the most recent ones; older ones keep their
/// first message as the name.
pub(super) const NAME_RECENT: usize = 10;

impl KynexApp {
    /// Starts naming the next unnamed session in `items`, unless one is underway:
    /// the shown one first, so its header has a name soon after its first message.
    pub(super) fn name_sessions(&mut self, items: &[SessionInfo], cx: &mut Context<Self>) {
        if self.naming.is_some() {
            return;
        }
        let shown = self.active_session().and_then(|s| s.session_id.clone());
        let wanted = |s: &&SessionInfo| !s.named && !self.naming_tried.contains(&s.id);
        let recent = if self.saved.name_past_sessions { NAME_RECENT } else { 0 };
        let next = items
            .iter()
            .find(|s| shown.as_deref() == Some(s.id.as_str()) && wanted(s))
            .or_else(|| items.iter().take(recent).find(wanted));
        let Some(next) = next else { return };
        let (id, first) = (next.id.clone(), next.first.clone());
        // OpenRouter sessions, and any while signed out of Anthropic (no Haiku to
        // ask), are named by their first words.
        let dir = self.active_dir();
        let provider = match self.groups_for(&dir).provider_of(&id) {
            kynex_core::provider::Provider::Anthropic if self.anthropic_signed_out() => {
                kynex_core::provider::Provider::OpenRouter
            }
            other => other,
        };
        self.naming = Some(id.clone());
        self.naming_tried.insert(id.clone());
        cx.spawn(async move |this, cx| {
            let named = cx.background_spawn(async move { titles::generate(&first, provider).map(|t| (id, t)) }).await;
            this.update(cx, |this, cx| {
                this.naming = None;
                match named {
                    Ok((id, title)) => {
                        titles::remember(&id, &title);
                        // Re-read the list so the name shows (and the next one starts).
                        this.sessions = None;
                    }
                    Err(e) => eprintln!("kynex: couldn't name a session: {e}"),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The shown session's name, if it has one yet: the one given by hand first.
    pub(super) fn session_name(&self, id: &str) -> Option<String> {
        if let Some(name) = self.active_session().and_then(|s| self.organize.name_of(&s.cwd, id)) {
            return Some(name);
        }
        let list = self.sessions.as_ref()?;
        list.items.iter().find(|s| s.id == id && s.named).map(|s| s.title.clone())
    }
}
