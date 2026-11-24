//! The composer's input features from the CLI: the `!` / `#` / `?` modes (rules in
//! `kynex_core::claude::input::input_mode`), long pastes folded into chips, the
//! `ultrathink` highlight, and the `?` card of shortcuts.

use std::ops::Range;

use crate::ui::px;
use gpui::{AnyElement, Context, Hsla, div, prelude::*};
use kynex_core::claude::input::input_mode::{InputMode, Submit};
use kynex_core::claude::input::thinking;

use super::{ClaudeChatEvent, SettingsPage};
use super::{ClaudePane, Key};
use crate::theme::{Palette, mono, theme};
use crate::ui::input::Mark;
use crate::ui::{Icon, icon, popover};

/// The colour that says which mode the composer is in.
pub(super) fn mode_colour(mode: InputMode, t: &Palette) -> Option<Hsla> {
    match mode {
        InputMode::Prompt => None,
        InputMode::Shell => Some(t.warning),
        InputMode::Memory => Some(t.info),
        InputMode::Shortcuts => Some(t.accent),
    }
}

/// The composer's keys for the `?` card: (keys, what), with a rebindable
/// shortcut's keys read from `keymap.json`'s.
pub(super) fn composer_shortcuts() -> Vec<(String, &'static str)> {
    let show = |keys: &str| kynex_core::shortcuts::display(keys, cfg!(target_os = "macos"));
    let bound = |name: &str, fallback: &str| {
        show(crate::app::keys::in_effect().iter().find(|(n, _)| *n == name).map_or(fallback, |(_, k)| k.as_str()))
    };
    let fixed = |s: &str| s.to_string();
    vec![
        (fixed("!"), "Shell mode: run a command"),
        (fixed("#"), "Memory: add a line to CLAUDE.md"),
        (fixed("/"), "Commands"),
        (fixed("@"), "Mention a file"),
        (fixed("Enter"), "Send"),
        (show("shift-enter"), "New line"),
        (show("ctrl-enter"), "Send now, with the queue"),
        (bound("cycle_mode", "shift-tab"), "Next permission mode"),
        (bound("toggle_thinking", "alt-t"), "Thinking on / off"),
        (bound("pick_model", "alt-p"), "Pick a model"),
        (fixed("↑ / ↓"), "Earlier messages"),
        (bound("search_history", "ctrl-r"), "Search what you've sent"),
        (bound("stash_draft", "ctrl-s"), "Stash the draft"),
        (bound("edit_in_editor", "ctrl-g"), "Edit in your editor"),
        (bound("interrupt", "escape"), "Stop Claude"),
        (fixed("Esc Esc"), "Go back to an earlier message"),
    ]
}

impl ClaudePane {
    /// The composer's text changed: a leading `!`, `#` or `?` switches its mode.
    pub(super) fn composer_changed(&mut self, cx: &mut Context<Self>) {
        let text = self.composer.read(cx).text().to_string();
        let prev = std::mem::replace(&mut self.last_text, text.clone());
        if prev == text {
            return;
        }
        if let Some(switch) = self.input_mode.on_change(&prev, &text, true) {
            self.input_mode = switch.mode;
            self.last_text = switch.text.clone();
            self.composer.update(cx, |c, cx| c.set_text(switch.text, cx));
        }
        cx.notify();
    }

    /// Backspace or Esc in the empty composer leaves its mode; says whether it did.
    pub(super) fn leave_input_mode(&mut self, cx: &mut Context<Self>) -> bool {
        let text = self.composer.read(cx).text().to_string();
        match self.input_mode.on_backspace(&text) {
            Some(mode) => {
                self.input_mode = mode;
                cx.notify();
                true
            }
            None => false,
        }
    }

    /// Back to an empty prompt with no pastes, after a send.
    pub(super) fn reset_input_mode(&mut self) {
        self.input_mode = InputMode::Prompt;
        self.last_text.clear();
        self.pastes.clear();
    }

    /// What Enter does with the composer's text `raw`, with pastes expanded. A
    /// recalled `!ls` in a prompt runs as the command it was; a paste that starts
    /// with `!` or `#` doesn't (the prefix is read before the chips open).
    pub(super) fn take_submit(&self, raw: &str) -> (InputMode, Submit) {
        let (mode, text) = match self.input_mode {
            InputMode::Prompt => match InputMode::Prompt.on_change("", raw.trim_start(), true) {
                Some(s) if s.mode != InputMode::Shortcuts => (s.mode, s.text),
                _ => (InputMode::Prompt, raw.to_string()),
            },
            mode => (mode, raw.to_string()),
        };
        (mode, mode.submit(&self.pastes.expand(&text)))
    }

    /// A long paste: kept, and a chip put in its place.
    pub(super) fn fold_paste(&mut self, text: String, cx: &mut Context<Self>) {
        let chip = self.pastes.fold(text);
        self.composer.update(cx, |c, cx| c.insert(&chip, cx));
    }

    /// A chip clicked: its paste goes back in its place, in full.
    pub(super) fn expand_chip(&mut self, range: Range<usize>, cx: &mut Context<Self>) {
        let text = self.composer.read(cx).text().to_string();
        let Some(chip) = self.pastes.chips(&text).into_iter().find(|c| c.range == range) else { return };
        let Some(paste) = self.pastes.text(chip.id).map(str::to_string) else { return };
        self.composer.update(cx, |c, cx| c.replace(range, &paste, cx));
    }

    /// Colours the composer: chips, and `ultrathink` while the CLI acts on it.
    pub(super) fn mark_composer(&mut self, key: Option<Key>, cx: &mut Context<Self>) {
        let t = theme(cx).clone();
        let text = self.composer.read(cx).text().to_string();
        let mut marks: Vec<Mark> = self
            .pastes
            .chips(&text)
            .into_iter()
            .map(|c| Mark { range: c.range, color: t.accent, background: Some(t.active), atomic: true })
            .collect();
        let ultrathink = key.and_then(|k| self.session_by_key(k)).is_some_and(|s| s.ultrathink.available);
        if ultrathink && self.input_mode == InputMode::Prompt {
            marks.extend(thinking::keyword_ranges(&text).into_iter().map(|range| Mark {
                range,
                color: t.info,
                background: None,
                atomic: false,
            }));
        }
        self.composer.update(cx, |c, cx| c.set_marks(marks, cx));
    }

    /// The mode's label at the field's left: its character and name in its colour.
    pub(super) fn render_mode_badge(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let mode = self.input_mode;
        let t = theme(cx).clone();
        let colour = mode_colour(mode, &t)?;
        let prefix = mode.prefix()?;
        Some(
            div()
                .flex()
                .items_center()
                .gap_1p5()
                .text_xs()
                .text_color(colour)
                .child(div().font_family(mono()).font_weight(gpui::FontWeight::BOLD).child(prefix.to_string()))
                .child(mode.label())
                .child(div().text_color(t.text_placeholder).child("· Backspace to leave"))
                .into_any_element(),
        )
    }

    /// The `?` card: the composer's shortcuts, and the way to all of them.
    pub(super) fn render_shortcuts_card(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx).clone();
        let rows = composer_shortcuts().into_iter().map(|(keys, what)| {
            div()
                .flex()
                .items_center()
                .gap_3()
                .py_0p5()
                .child(div().w(px(110.)).flex_none().font_family(mono()).text_xs().text_color(t.text).child(keys))
                .child(div().text_xs().text_color(t.text_muted).child(what))
        });
        let (left, right): (Vec<_>, Vec<_>) = rows.enumerate().partition(|(n, _)| n % 2 == 0);
        let column =
            |rows: Vec<(usize, gpui::Div)>| div().flex_1().flex().flex_col().children(rows.into_iter().map(|r| r.1));
        let hover = t.hover;
        popover(cx)
            .p_3()
            .gap_2()
            .child(div().flex().gap_6().child(column(left)).child(column(right)))
            .child(
                div()
                    .id("shortcuts-all")
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_1()
                    .rounded_md()
                    .text_xs()
                    .text_color(t.accent)
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover))
                    .child("All shortcuts in Settings → Keyboard")
                    .child(icon(Icon::ChevronRight, t.accent).size_3())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.input_mode = InputMode::Prompt;
                        cx.emit(ClaudeChatEvent::OpenSettings(SettingsPage::Keyboard));
                        cx.notify();
                    })),
            )
            .into_any_element()
    }
}
