//! The chat view's keyboard: the composer's own keys (history, stash, queue),
//! completion popup navigation, find, turn jumps, card shortcuts and the Esc stack.

use gpui::{Context, Focusable, KeyDownEvent, Window};
use kynex_core::claude::PermissionAnswer;

use super::runs::Scroll;
use super::*;

impl ClaudeChatView {
    pub(super) fn jump_user_turn(&mut self, dir: isize, cx: &mut Context<Self>) {
        let turns: Vec<usize> =
            self.rows.iter().enumerate().filter_map(|(i, r)| r.is_user_turn().then_some(i)).collect();
        if turns.is_empty() {
            return;
        }
        // The top item's first row: turns are compared in rows, the list scrolls in items.
        let top_item = self.list_state.logical_scroll_top().item_ix;
        let top = self.items.get(top_item).map_or(0, |it| it.start());
        let target = if dir < 0 {
            turns.iter().rev().copied().find(|&i| i < top).unwrap_or(turns[0])
        } else {
            turns.iter().copied().find(|&i| i > top).unwrap_or(*turns.last().unwrap())
        };
        self.jump_to_row(target, cx);
    }

    // -----------------------------------------------------------------------
    // Top-level keyboard handler
    // -----------------------------------------------------------------------

    /// Intercepts completion popup navigation (`Up`/`Down`/`Tab`) before `TextInput`
    /// handles vertical cursor movement inside multiline text.
    pub(super) fn on_capture_key_down(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let m = &ev.keystroke.modifiers;
        let secondary = if cfg!(target_os = "macos") { m.platform } else { m.control };
        let key = ev.keystroke.key.as_str();
        // Transcript keys that work even while typing: Page Up / Down, Ctrl+Home / End, Ctrl+O.
        let plain = !secondary && !m.alt && !m.shift;
        let scroll = match key {
            "pageup" if plain => Some(Scroll::Page(-1.)),
            "pagedown" if plain => Some(Scroll::Page(1.)),
            "home" if secondary && !m.alt && !m.shift => Some(Scroll::Top),
            "end" if secondary && !m.alt && !m.shift => Some(Scroll::Bottom),
            _ => None,
        };
        if let Some(by) = scroll {
            cx.stop_propagation();
            self.scroll_keys(by, cx);
            return;
        }
        let expand_key = if cfg!(target_os = "macos") { m.control && !m.shift } else { m.control && m.shift };
        // Ctrl+Shift+T (Cmd+Shift+T): the timeline.
        if secondary && m.shift && !m.alt && key == "t" {
            cx.stop_propagation();
            self.toggle_timeline(cx);
            return;
        }
        if expand_key && !m.alt && key == "o" {
            cx.stop_propagation();
            self.toggle_expand_tools(cx);
            return;
        }
        // Ctrl+C with text selected in the transcript copies it.
        if secondary
            && !m.alt
            && !m.shift
            && key == "c"
            && let Some(text) = crate::ui::selectable::selected_text(cx)
        {
            cx.stop_propagation();
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
            return;
        }
        if self.completion.is_none()
            && self.pending_card.is_none()
            && self.composer_input.read(cx).is_focused(window)
            && self.composer_keys(key, secondary, m.alt, m.shift, window, cx)
        {
            cx.stop_propagation();
            return;
        }
        if self.completion.is_none() {
            return;
        }
        if secondary || m.alt {
            return;
        }
        match ev.keystroke.key.as_str() {
            "up" => {
                cx.stop_propagation();
                self.step_completion(-1, cx);
            }
            "down" => {
                cx.stop_propagation();
                self.step_completion(1, cx);
            }
            "tab" if !m.shift => {
                cx.stop_propagation();
                self.accept_completion(window, cx);
            }
            _ => {}
        }
    }

    /// The composer's own keys: Ctrl+R / Ctrl+S, Ctrl+Enter / Alt+Enter for the
    /// queue, ↑ / ↓ through history at the first / last line. True if taken.
    pub(super) fn composer_keys(
        &mut self,
        key: &str,
        secondary: bool,
        alt: bool,
        shift: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        match key {
            "r" if secondary && !alt && !shift => self.open_history_search(cx),
            "s" if secondary && !alt && !shift => self.toggle_stash(cx),
            "g" if secondary && !alt && !shift => self.edit_in_editor(cx),
            "t" if alt && !secondary && !shift => self.toggle_thinking(cx),
            "backspace" if !secondary && !alt => return self.leave_input_mode(cx),
            "enter" if secondary && !alt && !shift => self.send_queue_now(window, cx),
            "enter" if alt && !secondary => self.steer_all(window, cx),
            "up" if !secondary && !alt && !shift && self.cursor_on_edge_line(true, cx) => return self.history_up(cx),
            "down" if !secondary && !alt && !shift && self.cursor_on_edge_line(false, cx) => {
                return self.history_down(cx);
            }
            _ => return false,
        }
        true
    }

    pub(super) fn on_key_down(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = ev.keystroke.key.as_str();
        let m = &ev.keystroke.modifiers;
        let secondary = if cfg!(target_os = "macos") { m.platform } else { m.control };

        // Home / End and ↑ / ↓ scroll the transcript when the message box doesn't have them.
        if !secondary && !m.alt && !m.shift && !self.composer_input.read(cx).is_focused(window) {
            let by = match key {
                "home" => Some(Scroll::Top),
                "end" => Some(Scroll::Bottom),
                "up" => Some(Scroll::Line(-1.)),
                "down" => Some(Scroll::Line(1.)),
                _ => None,
            };
            if let Some(by) = by
                && self.pending_card.is_none()
            {
                cx.stop_propagation();
                self.scroll_keys(by, cx);
                return;
            }
        }

        // Ctrl+F / Cmd+F -> Transcript find
        if secondary && !m.alt && !m.shift && key.eq_ignore_ascii_case("f") {
            cx.stop_propagation();
            self.toggle_find(window, cx);
            return;
        }

        // Shift+Tab -> Cycle permission mode
        if m.shift && !secondary && !m.alt && key == "tab" {
            cx.stop_propagation();
            let next = self.permission_mode.cycle_next(self.active_model_supports_auto());
            self.set_permission_mode(next, cx);
            return;
        }

        // Alt+Up / Alt+Down -> Jump between user turns
        if m.alt && !secondary && !m.shift && (key == "up" || key == "down") {
            cx.stop_propagation();
            self.jump_user_turn(if key == "up" { -1 } else { 1 }, cx);
            return;
        }

        // Alt+Enter outside the composer -> Mid-turn steer of everything queued
        if m.alt && !secondary && key == "enter" {
            cx.stop_propagation();
            self.steer_all(window, cx);
            return;
        }

        // Tab -> Accept prompt suggestion when idle & composer empty
        if !m.shift
            && !m.alt
            && !secondary
            && key == "tab"
            && !self.turn_active
            && self.composer_input.read(cx).text().trim().is_empty()
            && let Some(sug) = self.prompt_suggestion.take()
        {
            cx.stop_propagation();
            self.composer_input.update(cx, |inp, cx| inp.set_text(sug, cx));
            self.composer_input.read(cx).focus_handle(cx).focus(window, cx);
            cx.notify();
            return;
        }

        // 1 / 2 / 3 shortcuts for Permission & ExitPlanMode cards (when not typing in inputs)
        if !secondary
            && !m.alt
            && !m.shift
            && matches!(key, "1" | "2" | "3")
            && !self.composer_input.read(cx).is_focused(window)
            && !self.find_input.read(cx).is_focused(window)
            && self.handle_card_digit(key, window, cx)
        {
            cx.stop_propagation();
            return;
        }

        // Esc priority stack: Lightbox -> Subagent Peek -> Popover -> Completion -> Find -> Card Deny -> Interrupt
        if key == "escape" {
            if self.lightbox.is_some() {
                cx.stop_propagation();
                self.close_lightbox(cx);
                cx.notify();
            } else if self.peek_subagent.is_some() {
                cx.stop_propagation();
                self.peek_subagent = None;
                cx.notify();
            } else if self.open_popover.is_some() {
                cx.stop_propagation();
                self.open_popover = None;
                cx.notify();
            } else if self.memory_menu {
                cx.stop_propagation();
                self.memory_menu = false;
                cx.notify();
            } else if self.input_mode != kynex_core::claude::input::input_mode::InputMode::Prompt
                && self.leave_input_mode(cx)
            {
                cx.stop_propagation();
            } else if self.completion.is_some() {
                cx.stop_propagation();
                if !self.cancel_history_search(cx) {
                    self.completion = None;
                }
                cx.notify();
            } else if self.find.open {
                cx.stop_propagation();
                self.close_find(window, cx);
            } else if let Some(card) = self.pending_card.clone() {
                cx.stop_propagation();
                let note = self.take_card_input(cx);
                let req_id = card.req_id().to_string();
                self.answer_permission(&req_id, PermissionAnswer::Deny { message: note, interrupt: false }, window, cx);
            } else if self.double_esc(cx) {
                cx.stop_propagation();
            } else if self.turn_active {
                cx.stop_propagation();
                self.cancel_turn(cx);
            }
        }
    }
}
