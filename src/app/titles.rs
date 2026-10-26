//! Names for sessions that Claude Code didn't name: one at a time, in the
//! background, for the most recent ones in the sessions panel.

use gpui::{Context, prelude::*};
use kynex_core::session::SessionInfo;
use kynex_core::titles;

use super::KynexApp;

/// Older sessions keep their first message as the name.
const NAME_RECENT: usize = 30;

impl KynexApp {
    /// Starts naming the next unnamed session in `items`, unless one is underway.
    pub(super) fn name_sessions(&mut self, items: &[SessionInfo], cx: &mut Context<Self>) {
        if self.naming.is_some() {
            return;
        }
        let Some(next) = items.iter().take(NAME_RECENT).find(|s| !s.named && !self.naming_tried.contains(&s.id)) else {
            return;
        };
        let (id, first) = (next.id.clone(), next.first.clone());
        self.naming = Some(id.clone());
        self.naming_tried.insert(id.clone());
        cx.spawn(async move |this, cx| {
            let named = cx.background_spawn(async move { titles::generate(&first).map(|t| (id, t)) }).await;
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

    /// The shown session's name, if it has one yet.
    pub(super) fn session_name(&self, id: &str) -> Option<String> {
        let list = self.sessions.as_ref()?;
        list.items.iter().find(|s| s.id == id && s.named).map(|s| s.title.clone())
    }
}
