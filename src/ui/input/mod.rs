//! A text field, single- or multi-line with soft wrapping. Built on gpui's
//! `examples/input.rs` (IME, UTF-16 ranges, selection), extended to several
//! lines the way Zed's own fields work: the text is shaped with
//! `shape_text`, one wrapped line per `\n`-separated line.

use std::ops::Range;

use gpui::{
    App, Bounds, ClipboardEntry, ClipboardItem, Context, CursorStyle, EntityInputHandler, EventEmitter, FocusHandle,
    Focusable, Hsla, Image, KeyBinding, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point,
    ScrollHandle, SharedString, UTF16Selection, Window, actions, div, point, prelude::*, px,
};
use unicode_segmentation::UnicodeSegmentation;

mod edit;
mod element;

use element::{Layout, TextElement};

actions!(
    input,
    [
        Backspace,
        BackspaceWord,
        Delete,
        DeleteWord,
        Left,
        Right,
        Up,
        Down,
        WordLeft,
        WordRight,
        SelectLeft,
        SelectRight,
        SelectUp,
        SelectDown,
        SelectWordLeft,
        SelectWordRight,
        SelectAll,
        Home,
        End,
        SelectHome,
        SelectEnd,
        Paste,
        Cut,
        Copy,
        Enter,
        Newline,
    ]
);

const CONTEXT: &str = "TextInput";

pub fn bind_keys(cx: &mut App) {
    let c = Some(CONTEXT);
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, c),
        KeyBinding::new("ctrl-backspace", BackspaceWord, c),
        KeyBinding::new("alt-backspace", BackspaceWord, c),
        KeyBinding::new("delete", Delete, c),
        KeyBinding::new("ctrl-delete", DeleteWord, c),
        KeyBinding::new("left", Left, c),
        KeyBinding::new("right", Right, c),
        KeyBinding::new("up", Up, c),
        KeyBinding::new("down", Down, c),
        KeyBinding::new("ctrl-left", WordLeft, c),
        KeyBinding::new("ctrl-right", WordRight, c),
        KeyBinding::new("alt-left", WordLeft, c),
        KeyBinding::new("alt-right", WordRight, c),
        KeyBinding::new("shift-left", SelectLeft, c),
        KeyBinding::new("shift-right", SelectRight, c),
        KeyBinding::new("shift-up", SelectUp, c),
        KeyBinding::new("shift-down", SelectDown, c),
        KeyBinding::new("ctrl-shift-left", SelectWordLeft, c),
        KeyBinding::new("ctrl-shift-right", SelectWordRight, c),
        KeyBinding::new("alt-shift-left", SelectWordLeft, c),
        KeyBinding::new("alt-shift-right", SelectWordRight, c),
        KeyBinding::new(crate::ui::keys("secondary-a"), SelectAll, c),
        KeyBinding::new("home", Home, c),
        KeyBinding::new("end", End, c),
        KeyBinding::new("cmd-left", Home, c),
        KeyBinding::new("cmd-right", End, c),
        KeyBinding::new("shift-home", SelectHome, c),
        KeyBinding::new("shift-end", SelectEnd, c),
        KeyBinding::new(crate::ui::keys("secondary-v"), Paste, c),
        KeyBinding::new("shift-insert", Paste, c),
        KeyBinding::new(crate::ui::keys("secondary-x"), Cut, c),
        KeyBinding::new(crate::ui::keys("secondary-c"), Copy, c),
        KeyBinding::new("enter", Enter, c),
        KeyBinding::new("shift-enter", Newline, c),
        KeyBinding::new("ctrl-j", Newline, c),
    ]);
}

/// Line threshold at which a pasted block is emitted as [`InputEvent::PasteText`] when
/// [`TextInput::intercept_long_paste`] is enabled.
pub const LONG_PASTE_LINES: usize = 10;
/// Character threshold at which a pasted block is emitted as [`InputEvent::PasteText`] when
/// [`TextInput::intercept_long_paste`] is enabled.
pub const LONG_PASTE_CHARS: usize = 1_000;

pub enum InputEvent {
    Changed,
    /// Enter in a field that sends (Shift+Enter adds a line instead).
    Submit,
    /// Paste with an image on the clipboard: the owner decides what to do with it.
    #[allow(dead_code)]
    PasteImages(Vec<Image>),
    /// Paste of long text (>= 10 lines or >= 1,000 chars) when `intercept_long_paste` is enabled.
    #[allow(dead_code)]
    PasteText(String),
    /// A paste long enough to fold ([`TextInput::fold_pastes`]): the owner puts
    /// something in its place with [`TextInput::insert`].
    PasteFolded(String),
    /// A click on an atomic [`Mark`] (these bytes of the text).
    #[allow(dead_code)]
    MarkClicked(Range<usize>),
}

/// Text the owner colours: a keyword, or a chip that stands for more.
#[derive(Clone, Debug, PartialEq)]
pub struct Mark {
    pub range: Range<usize>,
    pub color: Hsla,
    pub background: Option<Hsla>,
    /// Deleted as a whole by Backspace / Delete, and clicking it says so
    /// ([`InputEvent::MarkClicked`]) instead of placing the cursor.
    pub atomic: bool,
}

impl EventEmitter<InputEvent> for TextInput {}

pub struct TextInput {
    focus_handle: FocusHandle,
    content: String,
    placeholder: SharedString,
    multiline: bool,
    mono: bool,
    /// Shows `*`s instead of the text, and won't copy or cut it: for secrets.
    masked: bool,
    /// Emit `InputEvent::PasteText` instead of inserting raw text when a paste is long.
    intercept_long_paste: bool,
    /// Long pastes go to the owner ([`InputEvent::PasteFolded`]) instead of in.
    fold_pastes: bool,
    /// Text was pasted since the field was last set.
    pasted: bool,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    /// The x to keep while moving up and down through lines of different lengths.
    goal_x: Option<Pixels>,
    last_layout: Option<Layout>,
    last_bounds: Option<Bounds<Pixels>>,
    is_selecting: bool,
    /// The scroll area the field sits in, if any, kept scrolled to the cursor.
    scroll: ScrollHandle,
    /// The cursor and text length last scrolled into view; the view follows
    /// the cursor only when one of them changes, so the wheel stays free.
    revealed: Option<(usize, usize)>,
    /// What the owner colours; ranges that no longer fit the text are left out.
    marks: Vec<Mark>,
}

impl TextInput {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            content: String::new(),
            placeholder: SharedString::default(),
            multiline: false,
            mono: false,
            masked: false,
            intercept_long_paste: false,
            fold_pastes: false,
            pasted: false,
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            goal_x: None,
            last_layout: None,
            last_bounds: None,
            is_selecting: false,
            scroll: ScrollHandle::new(),
            revealed: None,
            marks: Vec::new(),
        }
    }

    /// The atomic mark `offset` falls in: strictly inside, or (`edge`) at its end too.
    fn atomic_at(&self, offset: usize, edge: bool) -> Option<Range<usize>> {
        self.marks
            .iter()
            .filter(|m| m.atomic && m.range.end <= self.content.len())
            .map(|m| m.range.clone())
            .find(|r| r.start < offset && (offset < r.end || (edge && offset == r.end)))
    }

    pub fn multiline(mut self) -> Self {
        self.multiline = true;
        self
    }

    pub fn mono(mut self) -> Self {
        self.mono = true;
        self
    }

    /// A secret: shown as `*`s, one per byte so offsets and selections match
    /// the hidden text, and never copied or cut to the clipboard.
    pub fn masked(mut self) -> Self {
        self.masked = true;
        self
    }

    /// Emits [`InputEvent::PasteText`] instead of inserting raw text when a paste has
    /// at least [`LONG_PASTE_LINES`] lines or [`LONG_PASTE_CHARS`] characters.
    #[allow(dead_code)]
    pub fn intercept_long_paste(mut self) -> Self {
        self.intercept_long_paste = true;
        self
    }

    /// The text as drawn: the content, or `*`s for it when masked.
    fn shown(&self) -> String {
        if self.masked { "*".repeat(self.content.len()) } else { self.content.clone() }
    }

    pub fn placeholder(mut self, text: impl Into<SharedString>) -> Self {
        self.placeholder = text.into();
        self
    }

    /// Swaps the gray text shown while empty, e.g. for an inline suggestion. Doesn't
    /// notify: callers change it from their own render, before this input draws.
    pub fn set_placeholder(&mut self, text: impl Into<SharedString>) {
        self.placeholder = text.into();
    }

    pub fn placeholder_text(&self) -> &SharedString {
        &self.placeholder
    }

    pub fn text(&self) -> &str {
        &self.content
    }

    pub fn set_text(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.content = text.into();
        self.pasted = false;
        let end = self.content.len();
        self.selected_range = end..end;
        self.selection_reversed = false;
        self.marked_range = None;
        self.goal_x = None;
        cx.emit(InputEvent::Changed);
        cx.notify();
    }

    pub fn is_focused(&self, window: &Window) -> bool {
        self.focus_handle.is_focused(window)
    }

    fn left(&mut self, _: &Left, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.previous_boundary(self.cursor_offset()), window, cx);
        } else {
            self.move_to(self.selected_range.start, window, cx)
        }
    }

    fn right(&mut self, _: &Right, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.next_boundary(self.selected_range.end), window, cx);
        } else {
            self.move_to(self.selected_range.end, window, cx)
        }
    }

    fn up(&mut self, _: &Up, window: &mut Window, cx: &mut Context<Self>) {
        match self.vertical(-1) {
            Some((offset, x)) => {
                self.move_to(offset, window, cx);
                self.goal_x = Some(x);
            }
            None => cx.propagate(),
        }
    }

    fn down(&mut self, _: &Down, window: &mut Window, cx: &mut Context<Self>) {
        match self.vertical(1) {
            Some((offset, x)) => {
                self.move_to(offset, window, cx);
                self.goal_x = Some(x);
            }
            None => cx.propagate(),
        }
    }

    fn select_up(&mut self, _: &SelectUp, window: &mut Window, cx: &mut Context<Self>) {
        let (offset, x) = self.vertical(-1).unwrap_or((0, px(0.)));
        self.select_to(offset, window, cx);
        self.goal_x = Some(x);
    }

    fn select_down(&mut self, _: &SelectDown, window: &mut Window, cx: &mut Context<Self>) {
        let (offset, x) = self.vertical(1).unwrap_or((self.content.len(), px(0.)));
        self.select_to(offset, window, cx);
        self.goal_x = Some(x);
    }

    /// The offset one visual line up or down from the cursor, or `None` at the
    /// first or last line, so the key can go to whoever's listening outside.
    fn vertical(&self, dir: i32) -> Option<(usize, Pixels)> {
        if !self.multiline {
            return None;
        }
        let layout = self.last_layout.as_ref()?;
        let p = layout.position_for_offset(self.cursor_offset());
        let x = self.goal_x.unwrap_or(p.x);
        let y = p.y + layout.line_height * (dir as f32) + layout.line_height / 2.;
        let total = layout.line_top(layout.lines.len());
        if y < px(0.) || y >= total {
            return None;
        }
        Some((layout.offset_for_position(point(x, y)), x))
    }

    fn word_left(&mut self, _: &WordLeft, window: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.previous_word(self.cursor_offset()), window, cx);
    }

    fn word_right(&mut self, _: &WordRight, window: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.next_word(self.cursor_offset()), window, cx);
    }

    fn select_left(&mut self, _: &SelectLeft, window: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor_offset()), window, cx);
    }

    fn select_right(&mut self, _: &SelectRight, window: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor_offset()), window, cx);
    }

    fn select_word_left(&mut self, _: &SelectWordLeft, window: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_word(self.cursor_offset()), window, cx);
    }

    fn select_word_right(&mut self, _: &SelectWordRight, window: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_word(self.cursor_offset()), window, cx);
    }

    fn select_all(&mut self, _: &SelectAll, window: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, window, cx);
        self.select_to(self.content.len(), window, cx)
    }

    fn line_start(&self, offset: usize) -> usize {
        self.content[..offset].rfind('\n').map_or(0, |i| i + 1)
    }

    fn line_end(&self, offset: usize) -> usize {
        self.content[offset..].find('\n').map_or(self.content.len(), |i| offset + i)
    }

    fn home(&mut self, _: &Home, window: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.line_start(self.cursor_offset()), window, cx);
    }

    fn end(&mut self, _: &End, window: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.line_end(self.cursor_offset()), window, cx);
    }

    fn select_home(&mut self, _: &SelectHome, window: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.line_start(self.cursor_offset()), window, cx);
    }

    fn select_end(&mut self, _: &SelectEnd, window: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.line_end(self.cursor_offset()), window, cx);
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            match self.atomic_at(self.cursor_offset(), true) {
                Some(chip) => self.selected_range = chip,
                None => self.select_to(self.previous_boundary(self.cursor_offset()), window, cx),
            }
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn backspace_word(&mut self, _: &BackspaceWord, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.previous_word(self.cursor_offset()), window, cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            match self.atomic_at(self.cursor_offset() + 1, false) {
                Some(chip) => self.selected_range = chip,
                None => self.select_to(self.next_boundary(self.cursor_offset()), window, cx),
            }
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn delete_word(&mut self, _: &DeleteWord, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.next_word(self.cursor_offset()), window, cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn enter(&mut self, _: &Enter, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(InputEvent::Submit);
    }

    fn newline(&mut self, _: &Newline, window: &mut Window, cx: &mut Context<Self>) {
        if self.multiline {
            self.replace_text_in_range(None, "\n", window, cx);
        }
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.is_selecting = true;
        let offset = self.index_for_mouse_position(event.position);
        if !event.modifiers.shift
            && let Some(chip) = self.atomic_at(offset, false)
        {
            self.is_selecting = false;
            cx.emit(InputEvent::MarkClicked(chip));
            return;
        }
        if event.click_count >= 2 {
            let (start, end) = self.word_at(offset);
            self.move_to(start, window, cx);
            self.select_to(end, window, cx);
        } else if event.modifiers.shift {
            self.select_to(offset, window, cx);
        } else {
            self.move_to(offset, window, cx)
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _window: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_selecting {
            self.select_to(self.index_for_mouse_position(event.position), window, cx);
        }
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = cx.read_from_clipboard() else { return };
        let images: Vec<Image> = item
            .entries()
            .iter()
            .filter_map(|e| match e {
                ClipboardEntry::Image(i) => Some(i.clone()),
                _ => None,
            })
            .collect();
        if !images.is_empty() {
            cx.emit(InputEvent::PasteImages(images));
            return;
        }
        if let Some(text) = item.text() {
            let text = text.replace("\r\n", "\n");
            self.pasted = true;
            if self.fold_pastes && self.multiline && kynex_core::claude::input::paste::folds(&text) {
                cx.emit(InputEvent::PasteFolded(text));
                return;
            }
            let text = if self.multiline { text } else { text.replace('\n', " ") };
            if self.intercept_long_paste && is_long_paste(&text) {
                cx.emit(InputEvent::PasteText(text));
                return;
            }
            self.replace_text_in_range(None, &text, window, cx);
        }
    }

    /// With nothing selected here, Ctrl+C goes on to the views around the
    /// field, e.g. to copy text selected in the transcript.
    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            return cx.propagate();
        }
        if self.masked {
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(self.content[self.selected_range.clone()].to_string()));
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() && !self.masked {
            cx.write_to_clipboard(ClipboardItem::new_string(self.content[self.selected_range.clone()].to_string()));
            self.replace_text_in_range(None, "", window, cx)
        }
    }

    fn move_to(&mut self, offset: usize, window: &Window, cx: &mut Context<Self>) {
        self.selected_range = offset..offset;
        self.selection_reversed = false;
        self.goal_x = None;
        window.notify_text_selection_changed(&self.focus_handle);
        cx.notify()
    }

    pub fn cursor_offset(&self) -> usize {
        if self.selection_reversed { self.selected_range.start } else { self.selected_range.end }
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        let (Some(bounds), Some(layout)) = (self.last_bounds.as_ref(), self.last_layout.as_ref()) else {
            return 0;
        };
        if self.content.is_empty() {
            return 0;
        }
        if position.y < bounds.top() {
            return 0;
        }
        if position.y > bounds.bottom() {
            return self.content.len();
        }
        layout.offset_for_position(position - bounds.origin)
    }

    fn select_to(&mut self, offset: usize, window: &Window, cx: &mut Context<Self>) {
        if self.selection_reversed {
            self.selected_range.start = offset
        } else {
            self.selected_range.end = offset
        };
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        self.goal_x = None;
        window.notify_text_selection_changed(&self.focus_handle);
        cx.notify()
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8_offset = 0;
        let mut utf16_count = 0;
        for ch in self.content.chars() {
            if utf16_count >= offset {
                break;
            }
            utf16_count += ch.len_utf16();
            utf8_offset += ch.len_utf8();
        }
        utf8_offset
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf16_offset = 0;
        let mut utf8_count = 0;
        for ch in self.content.chars() {
            if utf8_count >= offset {
                break;
            }
            utf8_count += ch.len_utf8();
            utf16_offset += ch.len_utf16();
        }
        utf16_offset
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range_utf16: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range_utf16.start)..self.offset_from_utf16(range_utf16.end)
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.content.grapheme_indices(true).rev().find_map(|(idx, _)| (idx < offset).then_some(idx)).unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .find_map(|(idx, _)| (idx > offset).then_some(idx))
            .unwrap_or(self.content.len())
    }

    fn previous_word(&self, offset: usize) -> usize {
        self.content
            .split_word_bound_indices()
            .rev()
            .find(|(idx, w)| *idx < offset && !w.trim().is_empty())
            .map_or(0, |(idx, _)| idx)
    }

    fn next_word(&self, offset: usize) -> usize {
        self.content
            .split_word_bound_indices()
            .find(|(idx, w)| idx + w.len() > offset && !w.trim().is_empty())
            .map_or(self.content.len(), |(idx, w)| idx + w.len())
    }

    fn word_at(&self, offset: usize) -> (usize, usize) {
        self.content
            .split_word_bound_indices()
            .find(|(idx, w)| offset >= *idx && offset < idx + w.len())
            .map_or((offset, offset), |(idx, w)| (idx, idx + w.len()))
    }
}

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.shown()[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection { range: self.range_to_utf16(&self.selected_range), reversed: self.selection_reversed })
    }

    fn set_selected_text_range(&mut self, range_utf16: Range<usize>, window: &mut Window, cx: &mut Context<Self>) {
        self.selected_range = self.range_from_utf16(&range_utf16);
        self.selection_reversed = false;
        window.notify_text_selection_changed(&self.focus_handle);
        cx.notify();
    }

    fn text_length_utf16(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> Option<usize> {
        Some(self.content.encode_utf16().count())
    }

    fn marked_text_range(&self, _window: &mut Window, _cx: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked_range.as_ref().map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.marked_range = None;
        window.notify_text_input_changed(&self.focus_handle);
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        let new_text =
            if self.multiline { new_text.replace("\r\n", "\n") } else { new_text.replace(['\r', '\n'], " ") };

        self.content.replace_range(range.clone(), &new_text);
        self.selected_range = range.start + new_text.len()..range.start + new_text.len();
        self.selection_reversed = false;
        self.marked_range.take();
        self.goal_x = None;
        window.notify_text_input_changed(&self.focus_handle);
        cx.emit(InputEvent::Changed);
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());

        self.content.replace_range(range.clone(), new_text);
        self.marked_range = (!new_text.is_empty()).then(|| range.start..range.start + new_text.len());
        let insertion_start_utf16 = self.offset_to_utf16(range.start);
        let inserted_length_utf16 = new_text.encode_utf16().count();
        self.selected_range = new_selected_range_utf16
            .map(|selected| {
                self.range_from_utf16(
                    &(insertion_start_utf16 + selected.start.min(inserted_length_utf16)
                        ..insertion_start_utf16 + selected.end.min(inserted_length_utf16)),
                )
            })
            .unwrap_or_else(|| range.start + new_text.len()..range.start + new_text.len());
        self.selection_reversed = false;
        window.notify_text_input_changed(&self.focus_handle);
        cx.emit(InputEvent::Changed);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let layout = self.last_layout.as_ref()?;
        let range = self.range_from_utf16(&range_utf16);
        let start = layout.position_for_offset(range.start);
        let end = layout.position_for_offset(range.end);
        Some(Bounds::from_corners(
            bounds.origin + start,
            bounds.origin + point(end.x.max(start.x), end.y + layout.line_height),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let local = self.last_bounds?.localize(&point)?;
        let layout = self.last_layout.as_ref()?;
        Some(self.offset_to_utf16(layout.offset_for_position(local)))
    }
}

impl Render for TextInput {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .w_full()
            .key_context(CONTEXT)
            .track_focus(&self.focus_handle(cx))
            .cursor(CursorStyle::IBeam)
            .when(self.mono, |d| d.font_family(crate::theme::mono()))
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::backspace_word))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::delete_word))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::down))
            .on_action(cx.listener(Self::word_left))
            .on_action(cx.listener(Self::word_right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_up))
            .on_action(cx.listener(Self::select_down))
            .on_action(cx.listener(Self::select_word_left))
            .on_action(cx.listener(Self::select_word_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::select_home))
            .on_action(cx.listener(Self::select_end))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::enter))
            .on_action(cx.listener(Self::newline))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .child(TextElement { input: cx.entity() })
    }
}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

fn is_long_paste(text: &str) -> bool {
    text.lines().count() >= LONG_PASTE_LINES || text.chars().count() >= LONG_PASTE_CHARS
}

#[cfg(test)]
mod tests;
