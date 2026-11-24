//! What the owner of a field does to its text from outside: put text in at the
//! cursor or over a range, and ask where the cursor is and whether text was pasted.

use std::ops::Range;

use gpui::{Context, ScrollHandle, Window};

use super::{InputEvent, Mark, TextInput};

impl TextInput {
    /// Long pastes ([`kynex_core::claude::input::paste::folds`]) go to the owner to fold.
    pub fn fold_pastes(mut self) -> Self {
        self.fold_pastes = true;
        self
    }

    /// Whether text was pasted since the field was last set.
    pub fn pasted(&self) -> bool {
        self.pasted
    }

    /// Puts `text` where the cursor is, over any selection.
    pub fn insert(&mut self, text: &str, cx: &mut Context<Self>) {
        self.replace(self.selected_range.clone(), text, cx);
    }

    /// Where the cursor is, as a byte offset into [`text`](Self::text); `None`
    /// while text is selected or an IME composition is under way.
    pub fn cursor(&self) -> Option<usize> {
        (self.selected_range.is_empty() && self.marked_range.is_none()).then_some(self.selected_range.end)
    }

    /// Replaces the bytes in `range` with `text` and puts the cursor after it.
    pub fn replace(&mut self, range: Range<usize>, text: &str, cx: &mut Context<Self>) {
        self.content.replace_range(range.clone(), text);
        let end = range.start + text.len();
        self.selected_range = end..end;
        self.selection_reversed = false;
        self.marked_range = None;
        self.goal_x = None;
        cx.emit(InputEvent::Changed);
        cx.notify();
    }

    /// Replaces UTF-8 byte `range` in the field with `new_text` and moves the cursor to its end.
    #[allow(dead_code)]
    pub fn replace_range(&mut self, range: Range<usize>, new_text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let len = self.content.len();
        let mut start = range.start.min(len);
        let mut end = range.end.min(len).max(start);
        while start > 0 && !self.content.is_char_boundary(start) {
            start -= 1;
        }
        while end < len && !self.content.is_char_boundary(end) {
            end += 1;
        }
        let new_text =
            if self.multiline { new_text.replace("\r\n", "\n") } else { new_text.replace(['\r', '\n'], " ") };
        self.content.replace_range(start..end, &new_text);
        let cursor = start + new_text.len();
        self.selected_range = cursor..cursor;
        self.selection_reversed = false;
        self.marked_range.take();
        self.goal_x = None;
        window.notify_text_input_changed(&self.focus_handle);
        cx.emit(InputEvent::Changed);
        cx.notify();
    }

    #[allow(dead_code)]
    pub fn selected_range(&self) -> Range<usize> {
        self.selected_range.clone()
    }

    #[allow(dead_code)]
    pub fn set_marks(&mut self, marks: Vec<Mark>, cx: &mut Context<Self>) {
        if self.marks != marks {
            self.marks = marks;
            cx.notify();
        }
    }

    #[allow(dead_code)]
    pub fn scroll_handle(&self) -> &ScrollHandle {
        &self.scroll
    }
}
