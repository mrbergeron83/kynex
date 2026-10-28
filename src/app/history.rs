//! The composer's Claude Code keys beyond sending: ↑ / ↓ through what you've
//! sent, Ctrl+R to search it, Ctrl+S to stash a draft, Esc Esc to clear one
//! (or, with none, to rewind), and Ctrl+G to write it in your editor. The rules live in
//! `kynex_core::history`; this file only asks and draws.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use gpui::{AnyElement, Context, Window, prelude::*, px};
use kynex_core::history::{History, Stash};

use super::KynexApp;
use super::keys::StashDraft;
use crate::ui::completions::{Completion, completions};

/// Two Esc presses this close together clear the draft.
const DOUBLE_ESC: Duration = Duration::from_millis(600);

/// Ctrl+R: the composer's text is the query while it's open.
pub(super) struct HistorySearch {
    /// The draft from before, back on Esc.
    draft: String,
    selected: usize,
}

impl KynexApp {
    /// The shown project's history, loaded the first time it's needed.
    fn history(&mut self) -> Option<(PathBuf, &mut History)> {
        let cwd = self.active_session()?.cwd.clone();
        let h = self.histories.entry(cwd.clone()).or_insert_with(|| History::load(&cwd));
        Some((cwd, h))
    }

    fn save_history(&mut self, cx: &mut Context<Self>) {
        let Some((cwd, h)) = self.history() else { return };
        let h = h.clone();
        cx.background_spawn(async move {
            if let Err(e) = h.save(&cwd) {
                eprintln!("kynex: {e}");
            }
        })
        .detach();
    }

    /// A message just sent (or queued): next in ↑'s list.
    pub(super) fn remember_sent(&mut self, text: &str, cx: &mut Context<Self>) {
        if let Some((_, h)) = self.history() {
            h.push(text);
            self.save_history(cx);
        }
    }

    pub(super) fn set_composer(&mut self, text: String, cx: &mut Context<Self>) {
        self.composer.update(cx, |c, cx| c.set_text(text, cx));
        cx.notify();
    }

    /// ↑ past the composer's first line: queued messages come back first, as in
    /// Claude Code, then earlier messages one by one.
    pub(super) fn history_up(&mut self, cx: &mut Context<Self>) -> bool {
        if self.take_back_queue(cx) {
            return true;
        }
        let current = self.composer.read(cx).text().to_string();
        let Some(text) = self.history().and_then(|(_, h)| h.older(&current).map(str::to_string)) else {
            return false;
        };
        self.set_composer(text, cx);
        true
    }

    /// ↓ past the last line: the next message, then back to the draft.
    pub(super) fn history_down(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(text) = self.history().and_then(|(_, h)| h.newer()) else { return false };
        self.set_composer(text, cx);
        true
    }

    /// Ctrl+S: puts the draft aside, or brings it back into an empty composer.
    pub(super) fn toggle_stash(&mut self, cx: &mut Context<Self>) {
        let current = self.composer.read(cx).text().to_string();
        let Some((_, h)) = self.history() else { return };
        match h.toggle_stash(&current) {
            Stash::Stashed => {
                self.set_composer(String::new(), cx);
                self.notice = Some(match crate::ui::shortcut(&StashDraft, cx) {
                    Some(keys) => format!("Draft stashed. {keys} on an empty message box brings it back."),
                    None => "Draft stashed.".into(),
                });
            }
            Stash::Restored(text) => self.set_composer(text, cx),
            Stash::Nothing => return,
        }
        self.save_history(cx);
    }

    /// Esc with Claude idle: a second Esc soon after clears the draft (it stays
    /// in ↑'s list), or on an empty composer opens the rewind menu, unless
    /// `keymap.json` moved rewind to other keys. False if this Esc wasn't one of those.
    pub(super) fn double_esc(&mut self, cx: &mut Context<Self>) -> bool {
        let text = self.composer.read(cx).text().to_string();
        // Esc closes a menu or modal first; that press isn't for the draft.
        let covered = self.menu.is_some()
            || self.settings.is_some()
            || self.about_open
            || self.git_modal.is_some()
            || self.viewer.is_some();
        let empty = text.trim().is_empty();
        if covered || (empty && !super::keys::esc_esc_rewinds()) || self.active_session().is_some_and(|s| s.busy) {
            self.last_esc = None;
            return false;
        }
        if self.last_esc.take().is_some_and(|t| t.elapsed() < DOUBLE_ESC) {
            if empty {
                self.open_rewind(cx);
            } else {
                self.remember_sent(&text, cx);
                self.set_composer(String::new(), cx);
            }
        } else {
            self.last_esc = Some(Instant::now());
        }
        true
    }

    /// Ctrl+G: the draft opens in your editor and comes back when it closes.
    pub(super) fn edit_in_editor(&mut self, cx: &mut Context<Self>) {
        let text = self.composer.read(cx).text().to_string();
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { kynex_core::open::edit_text(&text) }).await;
            this.update(cx, |this, cx| match result {
                Ok(text) => this.set_composer(text, cx),
                Err(e) => {
                    this.notice = Some(e);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Ctrl+R: opens the search, or with it open, takes the selected match.
    pub(super) fn start_history_search(&mut self, cx: &mut Context<Self>) {
        if self.history_search.is_some() {
            return self.take_history_match(cx);
        }
        let draft = self.composer.read(cx).text().to_string();
        self.history_search = Some(HistorySearch { draft, selected: 0 });
        self.set_composer(String::new(), cx);
    }

    fn history_matches(&mut self, cx: &mut Context<Self>) -> Vec<String> {
        let query = self.composer.read(cx).text().to_string();
        self.history().map(|(_, h)| h.search(&query).into_iter().map(str::to_string).collect()).unwrap_or_default()
    }

    /// ↑ / ↓ while searching; false if the search isn't open.
    pub(super) fn history_search_move(&mut self, down: bool, cx: &mut Context<Self>) -> bool {
        if self.history_search.is_none() {
            return false;
        }
        let n = self.history_matches(cx).len();
        if let Some(s) = &mut self.history_search
            && n > 0
        {
            // The list reads newest first, top down, like the slash popup.
            s.selected = if down { (s.selected + 1) % n } else { (s.selected + n - 1) % n };
        }
        cx.notify();
        true
    }

    /// Enter / Tab while searching: the selected message into the composer.
    pub(super) fn take_history_match(&mut self, cx: &mut Context<Self>) {
        let Some(s) = self.history_search.take() else { return };
        let text = self.history_matches(cx).into_iter().nth(s.selected).unwrap_or(s.draft);
        self.set_composer(text, cx);
    }

    /// Esc while searching: back to the draft. False if the search isn't open.
    pub(super) fn cancel_history_search(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(s) = self.history_search.take() else { return false };
        self.set_composer(s.draft, cx);
        true
    }

    pub(super) fn history_searching(&self) -> bool {
        self.history_search.is_some()
    }

    /// The search's popup above the composer.
    pub(super) fn render_history_search(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.history_search.as_ref()?;
        let matches = self.history_matches(cx);
        let s = self.history_search.as_mut()?;
        s.selected = s.selected.min(matches.len().saturating_sub(1));
        let selected = s.selected;
        let items: Vec<Completion> = matches
            .iter()
            .map(|m| {
                let mut lines = m.lines();
                let first: String = lines.next().unwrap_or("").chars().take(120).collect();
                let more = lines.count();
                Completion {
                    label: first.into(),
                    hint: "".into(),
                    detail: if more > 0 { format!("+{more} lines").into() } else { "".into() },
                }
            })
            .collect();
        let this = cx.entity().downgrade();
        let list = completions(&items, selected, "No earlier message matches", cx, move |i, _, cx| {
            this.update(cx, |this, cx| {
                if let Some(s) = &mut this.history_search {
                    s.selected = i;
                }
                this.take_history_match(cx);
            })
            .ok();
        });
        Some(list.w_full().max_w(px(560.)).into_any_element())
    }

    /// Whether the composer has focus, for keys that only mean something there.
    pub(super) fn composer_focused(&self, window: &Window, cx: &Context<Self>) -> bool {
        self.composer.read(cx).is_focused(window)
    }
}
