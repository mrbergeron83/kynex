//! The chat pane's actions and their user-editable key bindings (`keymap.json`,
//! read with the app's in `app::keys`), in the "ClaudePane" context: the
//! composer's Claude Code keys, Ctrl+O, and scrolling the transcript.

use std::sync::atomic::{AtomicBool, Ordering};

use gpui::{Context, KeyBinding, Stateful, Window, actions, prelude::*};

use super::ClaudePane;
use super::transcript::Scroll;
use crate::app::keys::Interrupt;

actions!(
    kynex,
    [
        CycleMode,
        ToggleThinking,
        SearchHistory,
        StashDraft,
        PickModel,
        ExpandTools,
        EditInEditor,
        Rewind,
        PageUp,
        PageDown,
        ScrollTop,
        ScrollBottom,
        LineUp,
        LineDown,
        CopySelection
    ]
);

/// The context these keys are bound in: the pane's root.
const CONTEXT: &str = "ClaudePane";

/// The pane's shortcuts and their defaults, Claude Code's where it has one;
/// `keymap.json` overrides them by name like the app's own.
pub(crate) const SHORTCUTS: [(&str, &str); 13] = [
    ("cycle_mode", "shift-tab"),
    // Claude Code's Alt+T: extended thinking on or off.
    ("toggle_thinking", "alt-t"),
    ("search_history", "ctrl-r"),
    ("stash_draft", "ctrl-s"),
    ("pick_model", "alt-p"),
    // Claude Code's Ctrl+O; Ctrl+O already opens a project outside macOS.
    ("expand_tools", if cfg!(target_os = "macos") { "ctrl-o" } else { "ctrl-shift-o" }),
    ("edit_in_editor", "ctrl-g"),
    // Two Esc presses on an empty composer, told apart from one by timing (see
    // `history.rs`): a key sequence would hold every single Esc back.
    ("rewind", ESC_ESC),
    // Scrolling the transcript, even while typing; Home / End and ↑ / ↓ also
    // do when the message box doesn't have them (see `TRANSCRIPT_KEYS`).
    ("page_up", "pageup"),
    ("page_down", "pagedown"),
    ("scroll_top", "secondary-home"),
    ("scroll_bottom", "secondary-end"),
    // Copies text selected in the transcript; the composer has its own Ctrl+C.
    ("copy_selection", "secondary-c"),
];

/// Keys that scroll the transcript when nothing focused (the message box) uses them.
const TRANSCRIPT_KEYS: [&str; 4] = ["home", "end", "up", "down"];

const ESC_ESC: &str = "escape escape";

/// Whether Esc Esc opens the rewind menu: the default, unless `keymap.json` moved it.
static ESC_ESC_REWINDS: AtomicBool = AtomicBool::new(true);

pub(super) fn esc_esc_rewinds() -> bool {
    ESC_ESC_REWINDS.load(Ordering::Relaxed)
}

/// For the app's key loader: `name`'s keys are `keys`. True when that's rewind on
/// Esc Esc, which isn't a binding (two single Escs, timed); it's in effect all the same.
pub(crate) fn esc_esc(name: &str, keys: &str) -> bool {
    if name != "rewind" {
        return false;
    }
    let esc_esc = keys.split_whitespace().collect::<Vec<_>>().join(" ") == ESC_ESC;
    ESC_ESC_REWINDS.store(esc_esc, Ordering::Relaxed);
    esc_esc
}

/// The binding for one of [`SHORTCUTS`], or `None` for a name that isn't the pane's.
pub(crate) fn binding(name: &str, keys: &'static str) -> Option<KeyBinding> {
    let c = Some(CONTEXT);
    Some(match name {
        "cycle_mode" => KeyBinding::new(keys, CycleMode, c),
        "toggle_thinking" => KeyBinding::new(keys, ToggleThinking, c),
        "search_history" => KeyBinding::new(keys, SearchHistory, c),
        "stash_draft" => KeyBinding::new(keys, StashDraft, c),
        "pick_model" => KeyBinding::new(keys, PickModel, c),
        "expand_tools" => KeyBinding::new(keys, ExpandTools, c),
        "edit_in_editor" => KeyBinding::new(keys, EditInEditor, c),
        "rewind" => KeyBinding::new(keys, Rewind, c),
        "page_up" => KeyBinding::new(keys, PageUp, c),
        "page_down" => KeyBinding::new(keys, PageDown, c),
        "scroll_top" => KeyBinding::new(keys, ScrollTop, c),
        "scroll_bottom" => KeyBinding::new(keys, ScrollBottom, c),
        "copy_selection" => KeyBinding::new(keys, CopySelection, c),
        _ => return None,
    })
}

/// The transcript's fixed keys, on top of the rebindable ones.
pub(crate) fn transcript_bindings() -> Vec<KeyBinding> {
    let c = Some(CONTEXT);
    TRANSCRIPT_KEYS
        .into_iter()
        .map(|key| match key {
            "home" => KeyBinding::new(key, ScrollTop, c),
            "end" => KeyBinding::new(key, ScrollBottom, c),
            "up" => KeyBinding::new(key, LineUp, c),
            _ => KeyBinding::new(key, LineDown, c),
        })
        .collect()
}

impl ClaudePane {
    /// The pane's actions, on its root.
    pub(super) fn bind_actions(&self, d: Stateful<gpui::Div>, cx: &mut Context<Self>) -> Stateful<gpui::Div> {
        d.on_action(cx.listener(Self::interrupt_action))
            .on_action(cx.listener(Self::cycle_mode))
            .on_action(cx.listener(|this, _: &ToggleThinking, _, cx| this.toggle_thinking(cx)))
            .on_action(cx.listener(Self::search_history))
            .on_action(cx.listener(Self::stash_action))
            .on_action(cx.listener(|this, _: &PickModel, _, cx| this.open_model_menu(cx)))
            .on_action(cx.listener(Self::edit_in_editor_action))
            .on_action(cx.listener(|this, _: &Rewind, _, cx| this.open_rewind(cx)))
            .on_action(cx.listener(Self::expand_tools_action))
            .on_action(cx.listener(|this, _: &PageUp, _, cx| this.scroll_keys(Scroll::Page(-1.), cx)))
            .on_action(cx.listener(|this, _: &PageDown, _, cx| this.scroll_keys(Scroll::Page(1.), cx)))
            .on_action(cx.listener(|this, _: &ScrollTop, _, cx| this.scroll_keys(Scroll::Top, cx)))
            .on_action(cx.listener(|this, _: &ScrollBottom, _, cx| this.scroll_keys(Scroll::Bottom, cx)))
            .on_action(cx.listener(|this, _: &LineUp, _, cx| this.scroll_keys(Scroll::Line(-1.), cx)))
            .on_action(cx.listener(|this, _: &LineDown, _, cx| this.scroll_keys(Scroll::Line(1.), cx)))
            .on_action(cx.listener(|_, _: &CopySelection, _, cx| match crate::ui::selectable::selected_text(cx) {
                Some(text) => cx.write_to_clipboard(gpui::ClipboardItem::new_string(text)),
                None => cx.propagate(),
            }))
    }

    /// Esc by default: closes the pane's menu first, then stops a running reply.
    /// Otherwise the key goes on to whoever else wants it.
    fn interrupt_action(&mut self, _: &Interrupt, _: &mut Window, cx: &mut Context<Self>) {
        if self.close_menu(cx) {
            return;
        }
        if self.session.busy {
            self.session.interrupt();
            cx.notify();
        } else {
            cx.propagate();
        }
    }

    /// Like Shift+Tab in the CLI: the next permission mode, live.
    fn cycle_mode(&mut self, _: &CycleMode, _: &mut Window, cx: &mut Context<Self>) {
        let s = &mut self.session;
        // Bypass permissions is in the cycle only when Settings allows it.
        let next = s.mode.cycle(s.can_bypass());
        s.set_mode(next);
        cx.notify();
    }

    /// Ctrl+R in the composer: search what you've sent.
    fn search_history(&mut self, _: &SearchHistory, window: &mut Window, cx: &mut Context<Self>) {
        if !self.composer_focused(window, cx) {
            return cx.propagate();
        }
        self.start_history_search(cx);
    }

    /// Ctrl+S in the composer: stash the draft, or bring it back.
    fn stash_action(&mut self, _: &StashDraft, window: &mut Window, cx: &mut Context<Self>) {
        if !self.composer_focused(window, cx) {
            return cx.propagate();
        }
        self.toggle_stash(cx);
    }

    /// Ctrl+G in the composer: write the message in your editor.
    fn edit_in_editor_action(&mut self, _: &EditInEditor, window: &mut Window, cx: &mut Context<Self>) {
        if !self.composer_focused(window, cx) {
            return cx.propagate();
        }
        self.edit_in_editor(cx);
    }

    /// Claude Code's Ctrl+O: every tool call in the transcript open, or all closed again.
    fn expand_tools_action(&mut self, _: &ExpandTools, _: &mut Window, cx: &mut Context<Self>) {
        self.expand_tools = !self.expand_tools;
        if let Some(th) = &self.thread {
            th.list.remeasure();
        }
        cx.notify();
    }
}
