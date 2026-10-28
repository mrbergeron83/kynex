//! One terminal: a shell process, its keyboard and mouse input, and redraws
//! whenever it prints.

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle as MouseCursor, EntityInputHandler, EventEmitter, FocusHandle,
    Focusable, KeyBinding, KeyDownEvent, Keystroke, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, Point,
    ScrollDelta, ScrollEvent, Size as PxSize, UTF16Selection, Window, div, prelude::*,
};
use kynex_core::terminal::{self, Key, Palette, SelectBy, ShellSpec, Size, Snapshot, Terminal};

use super::element::{TerminalElement, background};
use crate::theme::theme;

const CONTEXT: &str = "Terminal";

/// A key the workspace would otherwise take (Esc stops Claude, Shift+Tab cycles
/// the mode…), sent to the shell instead while a terminal has focus.
#[derive(Clone, PartialEq, Debug, gpui::Action)]
#[action(namespace = terminal, no_json)]
pub struct SendKey(pub &'static str);

pub fn bind_keys(cx: &mut App) {
    let c = Some(CONTEXT);
    let keys = ["escape", "shift-tab", "ctrl-b", "ctrl-o", "ctrl-c", "ctrl-v", "ctrl-shift-c", "ctrl-shift-v"];
    // The workspace's Cmd+C copies conversation text; in a shell it copies the selection.
    let mac: &[&'static str] = if cfg!(target_os = "macos") { &["cmd-c", "cmd-v"] } else { &[] };
    let keys = keys.into_iter().chain(mac.iter().copied()).chain(crate::app::SHELL_KEYS);
    cx.bind_keys(keys.map(|k| KeyBinding::new(k, SendKey(k), c)));
}

pub enum TerminalEvent {
    /// The title or running state changed; the tab strip redraws.
    Changed,
}

impl EventEmitter<TerminalEvent> for TerminalView {}

pub struct TerminalView {
    terminal: Option<Terminal>,
    error: Option<String>,
    pub shell: ShellSpec,
    cwd: PathBuf,
    focus: FocusHandle,
    /// Where the grid was last drawn and its cell size, to map the mouse to cells.
    pub(super) layout: Option<(Point<Pixels>, PxSize<Pixels>)>,
    /// The cursor's (line, column) in the last drawn frame.
    cursor_cell: Option<(usize, usize)>,
    selecting: bool,
    /// Wheel movement not yet a whole line.
    scroll_rest: f32,
    /// A character just sent from its key press: Windows repeats it through
    /// the text input path, which must not send it twice.
    swallow: Option<String>,
    was_exited: bool,
}

impl TerminalView {
    pub fn new(shell: ShellSpec, cwd: PathBuf, cx: &mut Context<Self>) -> Self {
        let mut view = Self {
            terminal: None,
            error: None,
            shell,
            cwd,
            focus: cx.focus_handle(),
            layout: None,
            cursor_cell: None,
            selecting: false,
            scroll_rest: 0.,
            swallow: None,
            was_exited: false,
        };
        view.start(cx);
        view
    }

    /// Starts (or restarts) the shell.
    fn start(&mut self, cx: &mut Context<Self>) {
        // Output rings this from the PTY thread; one task redraws, coalescing bursts.
        let (tx, rx) = async_channel::bounded::<()>(1);
        let wake: kynex_core::session::Wake = Arc::new(move || {
            let _ = tx.try_send(());
        });
        cx.spawn(async move |this, cx| {
            while rx.recv().await.is_ok() {
                if this.update(cx, |this, cx| this.on_output(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        let size = self.terminal.as_ref().map_or(Size { columns: 100, lines: 20 }, Terminal::size);
        match Terminal::spawn(self.shell.clone(), &self.cwd, size, wake) {
            Ok(t) => {
                self.terminal = Some(t);
                self.error = None;
            }
            Err(e) => {
                self.terminal = None;
                self.error = Some(e);
            }
        }
        self.was_exited = false;
        cx.emit(TerminalEvent::Changed);
        cx.notify();
    }

    fn on_output(&mut self, cx: &mut Context<Self>) {
        if let Some(t) = &self.terminal {
            if let Some(text) = t.take_clipboard() {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
            if t.exited() != self.was_exited {
                self.was_exited = t.exited();
                cx.emit(TerminalEvent::Changed);
            }
        }
        cx.notify();
    }

    /// The tab's name: which shell it is.
    pub fn label(&self) -> &'static str {
        self.shell.kind.label()
    }

    /// What the shell calls its window (often the folder or running program).
    pub fn title(&self) -> Option<String> {
        self.terminal.as_ref().and_then(Terminal::title).map(|t| t.trim().to_string()).filter(|t| !t.is_empty())
    }

    pub fn exited(&self) -> bool {
        self.terminal.as_ref().is_none_or(Terminal::exited)
    }

    /// Types `command` and runs it, restarting the shell first if it had exited.
    pub fn run(&mut self, command: &str, cx: &mut Context<Self>) {
        if self.exited() {
            self.start(cx);
        }
        if let Some(t) = &self.terminal {
            t.run(command);
        }
    }

    pub(super) fn resize(&mut self, size: Size) {
        if let Some(t) = &mut self.terminal {
            t.resize(size);
        }
    }

    pub(super) fn snapshot(&mut self, palette: &Palette) -> Option<Snapshot> {
        let s = self.terminal.as_ref().map(|t| t.snapshot(palette))?;
        self.cursor_cell = s.cursor.map(|c| (c.line, c.column));
        Some(s)
    }

    fn copy(&self, cx: &mut Context<Self>) -> bool {
        match self.terminal.as_ref().and_then(Terminal::selection_text) {
            Some(text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                if let Some(t) = &self.terminal {
                    t.clear_selection();
                }
                cx.notify();
                true
            }
            None => false,
        }
    }

    fn paste_clipboard(&self, cx: &mut Context<Self>) {
        if let (Some(t), Some(text)) = (&self.terminal, cx.read_from_clipboard().and_then(|c| c.text())) {
            t.paste(&text);
        }
    }

    fn send_key(&mut self, action: &SendKey, _: &mut Window, cx: &mut Context<Self>) {
        const MAC: bool = cfg!(target_os = "macos");
        match action.0 {
            // Like Terminal.app: Cmd+C / Cmd+V, and Ctrl+C / Ctrl+V stay the shell's.
            "cmd-c" => {
                self.copy(cx);
            }
            "cmd-v" => self.paste_clipboard(cx),
            // Like Windows Terminal: Ctrl+C copies a selection, else interrupts.
            "ctrl-c" if !MAC && self.copy(cx) => {}
            "ctrl-shift-c" => {
                self.copy(cx);
            }
            "ctrl-v" if !MAC => self.paste_clipboard(cx),
            "ctrl-shift-v" => self.paste_clipboard(cx),
            key => {
                if let Ok(ks) = Keystroke::parse(key) {
                    self.send_keystroke(&ks, cx);
                }
            }
        }
    }

    fn send_keystroke(&mut self, ks: &Keystroke, cx: &mut Context<Self>) -> bool {
        let Some(t) = &self.terminal else { return false };
        let app_cursor = t.app_cursor();
        let key = Key {
            key: &ks.key,
            text: ks.key_char.as_deref(),
            ctrl: ks.modifiers.control,
            alt: ks.modifiers.alt,
            shift: ks.modifiers.shift,
        };
        match terminal::key_bytes(&key, app_cursor) {
            Some(bytes) => {
                if !key.ctrl {
                    self.swallow = ks.key_char.clone();
                }
                t.clear_selection();
                t.write(bytes);
                cx.notify();
                true
            }
            None => false,
        }
    }

    fn key_down(&mut self, ev: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.swallow = None;
        // Dead keys and IME composition come through the text input path instead.
        if ev.prefer_character_input {
            return;
        }
        // Leave the app's own shortcuts (Ctrl+Tab, Ctrl+1…) to the app.
        let m = ev.keystroke.modifiers;
        if m.platform || (m.control && ev.keystroke.key.parse::<u8>().is_ok()) {
            return;
        }
        if self.exited() && ev.keystroke.key == "enter" {
            self.start(cx);
            cx.stop_propagation();
            return;
        }
        if self.send_keystroke(&ev.keystroke, cx) {
            cx.stop_propagation();
        }
    }

    /// The visible cell under `position`, and whether it's the cell's right half.
    fn cell_at(&self, position: Point<Pixels>) -> Option<(usize, usize, bool)> {
        let (origin, cell) = self.layout?;
        let x = ((position.x - origin.x) / cell.width).max(0.);
        let y = ((position.y - origin.y) / cell.height).max(0.);
        Some((y as usize, x as usize, x.fract() > 0.5))
    }

    fn mouse_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.focus.focus(window, cx);
        let (Some(t), Some((line, column, right))) = (&self.terminal, self.cell_at(ev.position)) else { return };
        let by = match ev.click_count {
            2 => SelectBy::Word,
            n if n >= 3 => SelectBy::Line,
            _ => SelectBy::Cell,
        };
        t.select_start(line, column, right, by);
        self.selecting = true;
        cx.notify();
    }

    fn mouse_move(&mut self, ev: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selecting || ev.pressed_button != Some(MouseButton::Left) {
            self.selecting = false;
            return;
        }
        if let (Some(t), Some((line, column, right))) = (&self.terminal, self.cell_at(ev.position)) {
            t.select_to(line, column, right);
            cx.notify();
        }
    }

    fn mouse_up(&mut self, _: &gpui::MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.selecting = false;
        // A click without a drag selects nothing.
        if let Some(t) = &self.terminal
            && t.selection_text().is_none()
        {
            t.clear_selection();
            cx.notify();
        }
    }

    /// Right click: copy the selection, or paste when there is none.
    fn right_click(&mut self, _: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.focus.focus(window, cx);
        if !self.copy(cx) {
            self.paste_clipboard(cx);
        }
    }

    fn scroll(&mut self, ev: &ScrollEvent, _: &mut Window, cx: &mut Context<Self>) {
        let (Some(t), Some((_, cell))) = (&self.terminal, self.layout) else { return };
        let lines = match ev.delta {
            ScrollDelta::Lines(d) => d.y * 3.,
            ScrollDelta::Pixels(d) => d.y / cell.height,
        } + self.scroll_rest;
        let whole = lines.trunc();
        self.scroll_rest = lines - whole;
        if whole != 0. {
            t.scroll(whole as i32);
            cx.notify();
        }
    }
}

/// Text typed through the platform's input path (IME, dead keys, AltGr): sent
/// to the shell as is. There's no editable text here, so ranges are empty.
impl EntityInputHandler for TerminalView {
    fn text_for_range(
        &mut self,
        _: std::ops::Range<usize>,
        _: &mut Option<std::ops::Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        None
    }

    fn selected_text_range(&mut self, _: bool, _: &mut Window, _: &mut Context<Self>) -> Option<UTF16Selection> {
        Some(UTF16Selection { range: 0..0, reversed: false })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<std::ops::Range<usize>> {
        None
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {}

    fn replace_text_in_range(
        &mut self,
        _: Option<std::ops::Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.swallow.take().as_deref() == Some(text) {
            return;
        }
        if let Some(t) = &self.terminal {
            t.write(text.replace("\r\n", "\r").replace('\n', "\r").into_bytes());
            cx.notify();
        }
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<std::ops::Range<usize>>,
        _: &str,
        _: Option<std::ops::Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) {
    }

    fn bounds_for_range(
        &mut self,
        _: std::ops::Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        // Puts the IME window at the cursor.
        let (origin, cell) = self.layout?;
        let cursor = self.cursor_cell?;
        Some(Bounds::new(origin + gpui::point(cell.width * cursor.1 as f32, cell.height * cursor.0 as f32), cell))
    }

    fn character_index_for_point(&mut self, _: Point<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        None
    }
}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx).clone();
        let focused = self.focus.is_focused(window);
        let exited = self.terminal.as_ref().is_some_and(Terminal::exited);
        div()
            .id("terminal")
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .relative()
            .size_full()
            .bg(background(&t))
            .cursor(MouseCursor::IBeam)
            .on_action(cx.listener(Self::send_key))
            .on_key_down(cx.listener(Self::key_down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::right_click))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_scroll(cx.listener(Self::scroll))
            .child(TerminalElement::new(cx.entity(), self.focus.clone(), focused))
            .when_some(self.error.clone(), |d, e| {
                d.child(div().absolute().top_3().left_3().right_3().text_sm().text_color(t.error).child(e))
            })
            .when(exited, |d| {
                d.child(
                    div()
                        .absolute()
                        .bottom_2()
                        .right_3()
                        .px_2()
                        .py_0p5()
                        .rounded_md()
                        .bg(t.elevated)
                        .border_1()
                        .border_color(t.border)
                        .text_xs()
                        .text_color(t.text_muted)
                        .child("Process exited · press Enter to restart"),
                )
            })
    }
}
