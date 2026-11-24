//! The composer's Claude Code keys beyond sending: ↑ / ↓ through what you've
//! sent, Ctrl+R to search it, Ctrl+S to stash a draft, Esc Esc to clear one
//! (or, with none, to rewind), and Ctrl+G to write it in your editor; and the
//! session's unsent draft, kept with the project so it survives a restart. The
//! rules live in `kynex_core::claude::prompt_history`; this file only asks and draws.

use std::cell::{RefCell, RefMut};
use std::rc::Rc;
use std::time::{Duration, Instant};

use crate::ui::px;
use gpui::{AnyElement, Context, Window, prelude::*};
use kynex_core::claude::prompt_history::{History, Stash};
use kynex_core::toasts::Toast;

use super::ClaudePane;
use super::keys::StashDraft;
use crate::ui::completions::{Completion, completions};

/// A project's history, one for all the panes on its folder (the app keeps them
/// by folder), so one pane's sends and drafts don't overwrite another's.
/// `None` until it's read from disk.
pub type SharedHistory = Rc<RefCell<Option<History>>>;

/// Two Esc presses this close together clear the draft.
const DOUBLE_ESC: Duration = Duration::from_millis(600);

/// How long typing pauses before the draft is saved.
const DRAFT_SAVE_DELAY: Duration = Duration::from_millis(800);

/// Ctrl+R: the composer's text is the query while it's open.
pub(crate) struct HistorySearch {
    /// The draft from before, back on Esc.
    draft: String,
    selected: usize,
}

impl ClaudePane {
    /// Reads the project's history off the UI thread, unless another pane on
    /// the folder has, then brings the session's unsent draft back into an empty composer.
    pub(super) fn load_history(&mut self, cx: &mut Context<Self>) {
        if self.history.borrow().is_some() {
            return self.restore_draft(cx);
        }
        let cwd = self.session.cwd.clone();
        cx.spawn(async move |this, cx| {
            let loaded = cx.background_spawn(async move { History::load(&cwd) }).await;
            this.update(cx, |this, cx| {
                // Another pane's read may have come in first, and been added to since.
                this.history.borrow_mut().get_or_insert(loaded);
                this.restore_draft(cx);
            })
            .ok();
        })
        .detach();
    }

    /// The project's history, empty if it hasn't been read yet.
    fn history(&self) -> RefMut<'_, History> {
        RefMut::map(self.history.borrow_mut(), |h| h.get_or_insert_default())
    }

    /// The session's draft, into an empty composer.
    pub(super) fn restore_draft(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.session.session_id.as_deref() else { return };
        let Some(draft) = self.history().draft(id).map(str::to_string) else { return };
        if self.composer.read(cx).text().is_empty() {
            self.set_composer(draft, cx);
        }
    }

    fn save_history(&self, cx: &mut Context<Self>) {
        let h = self.history().clone();
        let cwd = self.session.cwd.clone();
        cx.background_spawn(async move {
            if let Err(e) = h.save(&cwd) {
                eprintln!("kynex: {e}");
            }
        })
        .detach();
    }

    /// The draft changed: saved once typing pauses, under the session's id.
    pub(super) fn draft_changed(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.session.session_id.clone() else { return };
        self.draft_gen += 1;
        let now = self.draft_gen;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(DRAFT_SAVE_DELAY).await;
            this.update(cx, |this, cx| {
                if this.draft_gen != now {
                    return;
                }
                // Chips go in full and the mode as its character, so nothing is lost.
                let raw = this.composer.read(cx).text().to_string();
                let text = this.input_mode.prefixed(&this.pastes.expand(&raw));
                let text = if raw.trim().is_empty() { String::new() } else { text };
                if this.history().set_draft(&id, &text) {
                    this.save_history(cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// A message just sent (or queued): next in ↑'s list; the session's draft is gone.
    pub(super) fn remember_sent(&mut self, text: &str, cx: &mut Context<Self>) {
        self.history().push(text);
        if let Some(id) = self.session.session_id.clone() {
            self.history().set_draft(&id, "");
        }
        self.save_history(cx);
    }

    pub(super) fn set_composer(&mut self, text: String, cx: &mut Context<Self>) {
        // A recalled `!ls` or `#note` comes back in its mode.
        self.reset_input_mode();
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
        let Some(text) = self.history().older(&current).map(str::to_string) else {
            return false;
        };
        self.set_composer(text, cx);
        true
    }

    /// ↓ past the last line: the next message, then back to the draft.
    pub(super) fn history_down(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(text) = self.history().newer() else { return false };
        self.set_composer(text, cx);
        true
    }

    /// Ctrl+S: puts the draft aside, or brings it back into an empty composer.
    pub(super) fn toggle_stash(&mut self, cx: &mut Context<Self>) {
        let current = self.composer.read(cx).text().to_string();
        let stash = self.history().toggle_stash(&current);
        match stash {
            Stash::Stashed => {
                self.set_composer(String::new(), cx);
                let toast = match crate::ui::shortcut(&StashDraft, cx) {
                    Some(keys) => {
                        Toast::info("Draft stashed").detail(format!("{keys} on an empty message box brings it back"))
                    }
                    None => Toast::info("Draft stashed"),
                };
                self.toast(toast, cx);
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
        // Esc closes a menu or the image viewer first; that press isn't for the draft.
        let covered = self.menu.is_some() || self.viewer.is_some();
        let empty = text.trim().is_empty();
        if covered || (empty && !super::keys::esc_esc_rewinds()) || self.session.busy {
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
        // Already open there.
        if self.editing_outside {
            return;
        }
        let text = self.composer.read(cx).text().to_string();
        let sent = text.clone();
        self.toast(Toast::info("Editing in your editor; close it to bring the message back"), cx);
        self.editing_outside = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { kynex_core::open::edit_text(&text) }).await;
            this.update(cx, |this, cx| {
                this.editing_outside = false;
                match result {
                    Ok(text) => {
                        // Typed here meanwhile: kept, after the editor's text.
                        let now = this.composer.read(cx).text().to_string();
                        let text = match now.strip_prefix(sent.as_str()) {
                            _ if now == sent => text,
                            Some(added) => format!("{text}{added}"),
                            None => format!("{text}\n\n{now}"),
                        };
                        this.set_composer(text, cx);
                    }
                    Err(e) => this.toast(Toast::error(e), cx),
                }
                cx.notify();
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

    fn history_matches(&self, cx: &Context<Self>) -> Vec<String> {
        let query = self.composer.read(cx).text().to_string();
        self.history().search(&query).into_iter().map(str::to_string).collect()
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
                    ..Default::default()
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
