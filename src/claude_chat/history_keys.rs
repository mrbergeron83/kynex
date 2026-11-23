//! The composer's keys beyond sending, as the old chat had them: ↑ / ↓ through
//! what you've sent (queued messages come back first), Ctrl+R to search it,
//! Ctrl+S to stash a draft, Esc Esc to clear one; drafts kept per session so
//! they survive a restart. And the queue: what's queued goes out together when
//! the turn ends, Ctrl+Enter sends it now, Alt+Enter steers it into the turn.
//! The rules live in `kynex_core::claude::prompt_history`.

use std::time::{Duration, Instant};

use gpui::{AppContext, Context};
use kynex_core::claude::PromptBlock;
use kynex_core::claude::prompt_history::{History, Stash};

use super::ClaudeChatView;
use super::state::{CompletionPopup, QueuedMessage};

/// Two Esc presses this close together clear the draft.
const DOUBLE_ESC: Duration = Duration::from_millis(600);

/// How long typing pauses before the draft is saved.
const DRAFT_SAVE_DELAY: Duration = Duration::from_millis(800);

impl ClaudeChatView {
    /// Reads the project's history off the UI thread, then brings back the
    /// session's unsent draft into an empty composer.
    pub(super) fn load_prompt_history(&mut self, cx: &mut Context<Self>) {
        let ws = self.config.workspace.clone();
        cx.spawn(async move |this, cx| {
            let loaded = cx.background_spawn(async move { History::load(&ws) }).await;
            this.update(cx, |this, cx| {
                this.prompt_history = loaded;
                let draft = this.session_id.as_deref().and_then(|s| this.prompt_history.draft(s)).map(str::to_string);
                if let Some(d) = draft
                    && this.composer_input.read(cx).text().is_empty()
                {
                    this.set_composer(d, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    fn save_prompt_history(&self, cx: &mut Context<Self>) {
        let h = self.prompt_history.clone();
        let ws = self.config.workspace.clone();
        cx.background_spawn(async move {
            if let Err(e) = h.save(&ws) {
                eprintln!("kynex: {e}");
            }
        })
        .detach();
    }

    /// A message just sent (or queued): next in ↑'s list; the session's draft is gone.
    pub(super) fn remember_sent(&mut self, text: &str, cx: &mut Context<Self>) {
        self.prompt_history.push(text);
        if let Some(sid) = self.session_id.clone() {
            self.prompt_history.set_draft(&sid, "");
        }
        self.save_prompt_history(cx);
    }

    /// The composer changed: save it as the session's draft once typing pauses.
    pub(super) fn draft_changed(&mut self, cx: &mut Context<Self>) {
        let Some(sid) = self.session_id.clone() else { return };
        self.draft_gen += 1;
        let gen_now = self.draft_gen;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(DRAFT_SAVE_DELAY).await;
            this.update(cx, |this, cx| {
                if this.draft_gen != gen_now {
                    return;
                }
                let text = this.composer_input.read(cx).text().to_string();
                if this.prompt_history.set_draft(&sid, &text) {
                    this.save_prompt_history(cx);
                }
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn set_composer(&mut self, text: String, cx: &mut Context<Self>) {
        self.composer_input.update(cx, |inp, cx| inp.set_text(text, cx));
        self.sync_composer_marks(cx);
        cx.notify();
    }

    /// Whether the composer's cursor is on its first (`up`) or last line.
    pub(super) fn cursor_on_edge_line(&self, up: bool, cx: &Context<Self>) -> bool {
        let inp = self.composer_input.read(cx);
        let (text, cursor) = (inp.text(), inp.cursor_offset().min(inp.text().len()));
        if up { !text[..cursor].contains('\n') } else { !text[cursor..].contains('\n') }
    }

    /// ↑ past the composer's first line: queued messages come back first, as in
    /// Claude Code, then earlier messages one by one.
    pub(super) fn history_up(&mut self, cx: &mut Context<Self>) -> bool {
        if self.take_back_queued(None, cx) {
            return true;
        }
        let current = self.composer_input.read(cx).text().to_string();
        let Some(text) = self.prompt_history.older(&current).map(str::to_string) else { return false };
        self.set_composer(text, cx);
        true
    }

    /// ↓ past the last line: the next message, then back to the draft.
    pub(super) fn history_down(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(text) = self.prompt_history.newer() else { return false };
        self.set_composer(text, cx);
        true
    }

    /// Ctrl+R: searches what you've sent, the composer's text as the query.
    pub(super) fn open_history_search(&mut self, cx: &mut Context<Self>) {
        if matches!(self.completion, Some(CompletionPopup::History { .. })) {
            return;
        }
        let draft = self.composer_input.read(cx).text().to_string();
        self.completion = Some(CompletionPopup::History { items: Vec::new(), selected: 0, draft });
        self.set_composer(String::new(), cx);
        self.refresh_history_matches(cx);
    }

    /// Refreshes the Ctrl+R matches for the composer's text; false if it isn't open.
    pub(super) fn refresh_history_matches(&mut self, cx: &mut Context<Self>) -> bool {
        let query = self.composer_input.read(cx).text().to_string();
        let found: Vec<String> = self.prompt_history.search(&query).into_iter().take(50).map(str::to_string).collect();
        let Some(CompletionPopup::History { items, selected, .. }) = &mut self.completion else { return false };
        *items = found;
        *selected = 0;
        true
    }

    /// Esc on the Ctrl+R search: the draft from before comes back.
    pub(super) fn cancel_history_search(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(CompletionPopup::History { draft, .. }) = &self.completion else { return false };
        let draft = draft.clone();
        self.completion = None;
        self.set_composer(draft, cx);
        true
    }

    /// Ctrl+S: puts the draft aside, or brings it back into an empty composer.
    pub(super) fn toggle_stash(&mut self, cx: &mut Context<Self>) {
        let current = self.composer_input.read(cx).text().to_string();
        match self.prompt_history.toggle_stash(&current) {
            Stash::Stashed => {
                self.set_composer(String::new(), cx);
                self.push_notice("Draft stashed. Ctrl+S on an empty message box brings it back.", false, cx);
            }
            Stash::Restored(text) => self.set_composer(text, cx),
            Stash::Nothing => return,
        }
        self.save_prompt_history(cx);
    }

    /// Esc with Claude idle: a second Esc soon after clears the draft (it stays
    /// in ↑'s list), or on an empty composer opens rewind. False if this Esc
    /// wasn't one of those.
    pub(super) fn double_esc(&mut self, cx: &mut Context<Self>) -> bool {
        let text = self.composer_input.read(cx).text().to_string();
        if self.turn_active || (text.trim().is_empty() && self.session_id.is_none()) {
            self.last_esc = None;
            return false;
        }
        if self.last_esc.take().is_some_and(|t| t.elapsed() < DOUBLE_ESC) {
            if text.trim().is_empty() {
                self.open_rewind(cx);
            } else {
                self.prompt_history.push(&text);
                self.save_prompt_history(cx);
                self.set_composer(String::new(), cx);
            }
        } else {
            self.last_esc = Some(Instant::now());
        }
        true
    }

    /// Takes queued message `id` (the newest with `None`) back into the
    /// composer to edit. Ones carrying images stay queued. False if none.
    pub(super) fn take_back_queued(&mut self, id: Option<u64>, cx: &mut Context<Self>) -> bool {
        let pos = match id {
            Some(id) => self.queued.iter().position(|m| m.id == id),
            None => self.queued.iter().rposition(|m| m.row_images.is_empty()),
        };
        let Some(pos) = pos.filter(|&p| self.queued[p].row_images.is_empty()) else { return false };
        let Some(msg) = self.queued.remove(pos) else { return false };
        let text = msg
            .blocks
            .iter()
            .find_map(|b| match b {
                PromptBlock::Text(t) => Some(t.clone()),
                _ => None,
            })
            .unwrap_or(msg.display_text);
        let current = self.composer_input.read(cx).text().trim().to_string();
        let text = if current.is_empty() { text } else { format!("{text}\n\n{current}") };
        self.set_composer(text, cx);
        true
    }

    /// Everything queued, as one message: texts one after another, images and
    /// chips together. `None` with nothing queued.
    pub(super) fn take_queue_batch(&mut self) -> Option<QueuedMessage> {
        let mut all = std::mem::take(&mut self.queued).into_iter();
        let mut batch = all.next()?;
        for m in all {
            batch.display_text = format!("{}\n\n{}", batch.display_text, m.display_text);
            // One text block for the whole batch, images first as the composer sends them.
            let (images, texts): (Vec<_>, Vec<_>) =
                batch.blocks.drain(..).chain(m.blocks).partition(|b| matches!(b, PromptBlock::Image { .. }));
            let text = texts
                .into_iter()
                .filter_map(|b| match b {
                    PromptBlock::Text(t) => Some(t),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n\n");
            batch.blocks = images;
            if !text.is_empty() {
                batch.blocks.push(PromptBlock::Text(text));
            }
            batch.file_chips.extend(m.file_chips);
            batch.paste_labels.extend(m.paste_labels);
            batch.row_images.extend(m.row_images);
        }
        Some(batch)
    }

    /// Ctrl+Enter: what's typed joins the queue, and the queue goes now,
    /// stopping the running turn (it's sent when the turn ends).
    pub(super) fn send_queue_now(&mut self, window: &mut gpui::Window, cx: &mut Context<Self>) {
        self.submit_composer(false, window, cx);
        if self.turn_active && !self.queued.is_empty() {
            self.cancel_turn(cx);
        }
    }

    /// Alt+Enter mid-turn: what's typed and everything queued steer the turn now.
    pub(super) fn steer_all(&mut self, window: &mut gpui::Window, cx: &mut Context<Self>) {
        if !self.turn_active {
            self.submit_composer(false, window, cx);
            return;
        }
        self.submit_composer(false, window, cx);
        if let Some(b) = self.take_queue_batch() {
            self.send_blocks_now(b.display_text, b.blocks, b.file_chips, b.paste_labels, b.row_images, true, cx);
        }
    }
}
