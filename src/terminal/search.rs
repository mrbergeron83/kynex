//! Find in a terminal's scrollback and copy mode, both on `TerminalView`.
//!
//! Find (Ctrl+Shift+F, Cmd+F on macOS) is a small bar over the terminal's top right with the
//! query, "3 of 12" and previous / next; the matches are tinted in the grid. Copy mode
//! (`prefix y`) puts a cursor on the scrollback: its keys never reach the shell. The searching
//! and the cursor are `kynex_core::terminal`'s; this is the keys and the bar.

use gpui::{
    AnyElement, ClipboardItem, Context, Entity, Focusable, Keystroke, MouseButton, Subscription, Window, div,
    prelude::*,
};
use kynex_core::terminal::{CopyMotion, SelectBy, Terminal};

use super::view::{OpenFind, SendKey, TerminalView};
use crate::theme::theme;
use crate::ui::input::{self, InputEvent, TextInput};
use crate::ui::motion::{self, fade_in};
use crate::ui::{Icon, Tooltip, icon, icon_button, px};

pub(super) struct FindBar {
    input: Entity<TextInput>,
    /// The query the matches are of.
    query: String,
    count: usize,
    /// The current match, counting from 1.
    current: usize,
    _sub: Subscription,
}

impl TerminalView {
    /// Ctrl+Shift+F: opens the bar, or puts the keyboard back in it.
    pub(super) fn open_find(&mut self, _: &OpenFind, window: &mut Window, cx: &mut Context<Self>) {
        self.open_find_bar(window, cx);
    }

    fn open_find_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.terminal.is_none() {
            return;
        }
        let input = match &self.find {
            Some(bar) => bar.input.clone(),
            None => {
                let input = cx.new(|cx| TextInput::new(cx).placeholder("Find in terminal"));
                let sub = cx.subscribe(&input, |this, _, ev: &InputEvent, cx| {
                    if matches!(ev, InputEvent::Changed) {
                        this.run_find(cx);
                    }
                });
                self.find =
                    Some(FindBar { input: input.clone(), query: String::new(), count: 0, current: 0, _sub: sub });
                input
            }
        };
        input.read(cx).focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.find = None;
        if let Some(t) = &self.terminal {
            t.find_clear();
        }
        self.focus.focus(window, cx);
        cx.notify();
    }

    /// Searches for what the bar says, at its newest match.
    fn run_find(&mut self, cx: &mut Context<Self>) {
        let (Some(t), Some(bar)) = (&self.terminal, &mut self.find) else { return };
        let query = bar.input.read(cx).text().to_string();
        if query.is_empty() {
            t.find_clear();
            bar.count = 0;
        } else {
            bar.count = t.find(&query);
        }
        bar.current = bar.count;
        bar.query = query;
        cx.notify();
    }

    fn step_find(&mut self, forward: bool, cx: &mut Context<Self>) {
        let (Some(t), Some(bar)) = (&self.terminal, &mut self.find) else { return };
        if let Some((current, count)) = t.find_step(forward) {
            (bar.current, bar.count) = (current, count);
            cx.notify();
        }
    }

    /// The bar, while it's open.
    pub(super) fn render_find(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let bar = self.find.as_ref()?;
        let t = theme(cx).clone();
        let input = bar.input.clone();
        let count = match (bar.count, bar.query.is_empty()) {
            (_, true) => String::new(),
            (0, false) => "No results".into(),
            (n, _) => format!("{} of {n}", bar.current),
        };
        let none = bar.count == 0;
        let el = div()
            .absolute()
            .top_2()
            .right_3()
            .flex()
            .items_center()
            .gap_1()
            .h(px(30.))
            .pl_2()
            .pr_1()
            .rounded_lg()
            .border_1()
            .border_color(t.border)
            .bg(t.elevated)
            .shadow_lg()
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .capture_action(cx.listener(|this, _: &input::Enter, _, cx| {
                this.step_find(true, cx);
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(|this, _: &input::Newline, _, cx| {
                this.step_find(false, cx);
                cx.stop_propagation();
            }))
            // The terminal's own keys (Esc goes to the shell otherwise) stay out of the field.
            .capture_action(cx.listener(|this, a: &SendKey, window, cx| {
                if a.0 == "escape" {
                    this.close_find(window, cx);
                }
                cx.stop_propagation();
            }))
            .child(icon(Icon::Search, t.text_muted).size_3p5().flex_none())
            .child(div().w(px(200.)).text_sm().child(input))
            .child(
                div()
                    .flex_none()
                    .min_w(px(64.))
                    .text_xs()
                    .text_color(if none { t.text_placeholder } else { t.text_muted })
                    .child(count),
            )
            .child(
                icon_button("term-find-prev", Icon::ArrowUp, cx)
                    .tooltip(Tooltip::with_meta("Previous match", "Shift+Enter"))
                    .on_click(cx.listener(|this, _, _, cx| this.step_find(false, cx))),
            )
            .child(
                icon_button("term-find-next", Icon::ArrowDown, cx)
                    .tooltip(Tooltip::with_meta("Next match", "Enter"))
                    .on_click(cx.listener(|this, _, _, cx| this.step_find(true, cx))),
            )
            .child(
                icon_button("term-find-close", Icon::Close, cx)
                    .tooltip(Tooltip::with_meta("Close", "Esc"))
                    .on_click(cx.listener(|this, _, window, cx| this.close_find(window, cx))),
            );
        Some(fade_in("term-find-bar", el, motion::QUICK).into_any_element())
    }

    pub(super) fn in_copy_mode(&self) -> bool {
        self.terminal.as_ref().is_some_and(Terminal::copy_mode)
    }

    /// `prefix y`: in or out of copy mode.
    pub(super) fn copy_mode_action(&mut self, _: &crate::app::CopyMode, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(t) = &self.terminal {
            t.toggle_copy_mode();
            cx.notify();
        }
    }

    /// A key in copy mode, vi-style.
    pub(super) fn copy_key(&mut self, ks: &Keystroke, window: &mut Window, cx: &mut Context<Self>) {
        let Some(t) = &self.terminal else { return };
        let m = ks.modifiers;
        // Named keys by name, characters as typed (`G`, `$`).
        let name = if ks.key.chars().count() > 1 { ks.key.as_str() } else { ks.key_char.as_deref().unwrap_or(&ks.key) };
        let motion = if m.control {
            match ks.key.as_str() {
                "u" => Some(CopyMotion::HalfPageUp),
                "d" => Some(CopyMotion::HalfPageDown),
                _ => None,
            }
        } else {
            match name {
                "h" | "left" => Some(CopyMotion::Left),
                "l" | "right" => Some(CopyMotion::Right),
                "k" | "up" => Some(CopyMotion::Up),
                "j" | "down" => Some(CopyMotion::Down),
                "w" => Some(CopyMotion::WordForward),
                "b" => Some(CopyMotion::WordBack),
                "e" => Some(CopyMotion::WordEnd),
                "0" | "home" => Some(CopyMotion::LineStart),
                "$" | "end" => Some(CopyMotion::LineEnd),
                "^" => Some(CopyMotion::FirstNonBlank),
                "g" => Some(CopyMotion::Top),
                "G" => Some(CopyMotion::Bottom),
                "pageup" => Some(CopyMotion::HalfPageUp),
                "pagedown" => Some(CopyMotion::HalfPageDown),
                _ => None,
            }
        };
        if let Some(motion) = motion {
            t.copy_motion(motion);
            return cx.notify();
        }
        let yank = |t: &Terminal, cx: &mut Context<Self>| {
            if let Some(text) = t.copy_yank() {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
        };
        match (m.control, name) {
            (false, "v") => t.copy_select(SelectBy::Cell),
            (false, "V") => t.copy_select(SelectBy::Line),
            (false, "y" | "enter") | (true, "c") => yank(t, cx),
            (false, "escape" | "q") => t.toggle_copy_mode(),
            (false, "/") => return self.open_find_bar(window, cx),
            (false, "n") => return self.step_find(true, cx),
            (false, "N") => return self.step_find(false, cx),
            _ => {}
        }
        cx.notify();
    }

    /// The COPY badge, bottom right.
    pub(super) fn render_copy_badge(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = theme(cx).clone();
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
            .text_color(t.accent)
            .child("COPY")
    }
}
