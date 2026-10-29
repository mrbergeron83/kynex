//! Workspace actions, their user-editable key bindings, and the handlers that
//! aren't tied to one panel (tabs, Esc, Shift+Tab, Ctrl+1…9).

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};

use gpui::{App, Context, KeyBinding, KeyDownEvent, Window, actions};
use kynex_core::paths::data_dir;
use kynex_core::project::Mode;
use kynex_core::session::Kind;
use kynex_core::toasts::Toast;

use super::KynexApp;
use super::state::Active;
use crate::ui::selectable;

actions!(
    kynex,
    [
        OpenProject,
        NewSession,
        NextSession,
        PrevSession,
        CloseTab,
        NextTab,
        PrevTab,
        ToggleSessions,
        CycleMode,
        Interrupt,
        CopySelection,
        ToggleTerminal,
        SearchHistory,
        StashDraft,
        PickModel,
        ExpandTools,
        EditInEditor,
        Rewind,
        ToggleTimeline,
        PrevTurn,
        NextTurn,
        PageUp,
        PageDown,
        ScrollTop,
        ScrollBottom,
        LineUp,
        LineDown,
        OpenSettings
    ]
);

/// Workspace shortcuts and their defaults, Claude Code's where it has one.
/// `<data dir>/keymap.json` overrides them by name, e.g. `{ "interrupt": "ctrl-c" }`;
/// an empty string unbinds one.
const SHORTCUTS: [(&str, &str); 25] = [
    ("open_project", "secondary-o"),
    ("open_settings", "secondary-,"),
    ("new_session", "secondary-n"),
    // The sessions panel's list, top to bottom. Brackets move around on
    // non-US layouts, so outside macOS it's the browsers' Ctrl+PageDown / Up.
    ("next_session", if cfg!(target_os = "macos") { "cmd-shift-]" } else { "ctrl-pagedown" }),
    ("prev_session", if cfg!(target_os = "macos") { "cmd-shift-[" } else { "ctrl-pageup" }),
    ("close_tab", "secondary-w"),
    ("next_tab", "ctrl-tab"),
    ("prev_tab", "ctrl-shift-tab"),
    ("toggle_sessions", "secondary-b"),
    ("toggle_terminal", "ctrl-`"),
    ("cycle_mode", "shift-tab"),
    ("interrupt", "escape"),
    ("search_history", "ctrl-r"),
    ("stash_draft", "ctrl-s"),
    ("pick_model", "alt-p"),
    // Claude Code's Ctrl+O; Ctrl+O already opens a project outside macOS.
    ("expand_tools", if cfg!(target_os = "macos") { "ctrl-o" } else { "ctrl-shift-o" }),
    ("edit_in_editor", "ctrl-g"),
    // Two Esc presses on an empty composer, told apart from one by timing (see
    // `history.rs`): a key sequence would hold every single Esc back.
    ("rewind", ESC_ESC),
    ("toggle_timeline", "secondary-shift-t"),
    // The timeline's turns, previous and next.
    ("prev_turn", "alt-up"),
    ("next_turn", "alt-down"),
    // Scrolling the transcript, even while typing; Home / End and ↑ / ↓ also
    // do when the message box doesn't have them (see `TRANSCRIPT_KEYS`).
    ("page_up", "pageup"),
    ("page_down", "pagedown"),
    ("scroll_top", "secondary-home"),
    ("scroll_bottom", "secondary-end"),
];

/// Keys that scroll the transcript when nothing focused (the message box, a
/// prompt) uses them, on top of the rebindable ones above.
const TRANSCRIPT_KEYS: [&str; 4] = ["home", "end", "up", "down"];

const ESC_ESC: &str = "escape escape";

/// Whether Esc Esc opens the rewind menu: the default, unless `keymap.json` moved it.
static ESC_ESC_REWINDS: AtomicBool = AtomicBool::new(true);

pub(super) fn esc_esc_rewinds() -> bool {
    ESC_ESC_REWINDS.load(Ordering::Relaxed)
}

/// The keys above a terminal sends to its shell instead (see `terminal::view`).
pub const SHELL_KEYS: [&str; 14] = [
    "ctrl-r",
    "ctrl-s",
    "ctrl-g",
    "ctrl-n",
    "ctrl-w",
    "alt-p",
    "pageup",
    "pagedown",
    "ctrl-home",
    "ctrl-end",
    "home",
    "end",
    "up",
    "down",
];

pub fn bind_keys(cx: &mut App) {
    let path = data_dir().join("keymap.json");
    let user: HashMap<String, String> = match std::fs::read_to_string(&path) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_else(|e| {
            eprintln!("kynex: ignoring {}: {e}", path.display());
            HashMap::new()
        }),
        Err(_) => {
            // Write the defaults out so there's something to edit.
            let defaults: BTreeMap<_, _> = SHORTCUTS.iter().copied().collect();
            if let Ok(s) = serde_json::to_string_pretty(&defaults) {
                let _ = std::fs::create_dir_all(data_dir());
                let _ = std::fs::write(&path, s);
            }
            HashMap::new()
        }
    };
    let c = Some("Workspace");
    let mut bindings = Vec::new();
    for (name, default) in SHORTCUTS {
        let keys = crate::ui::keys(user.get(name).map_or(default, String::as_str));
        if name == "rewind" {
            let esc_esc = keys.split_whitespace().collect::<Vec<_>>().join(" ") == ESC_ESC;
            ESC_ESC_REWINDS.store(esc_esc, Ordering::Relaxed);
            if esc_esc {
                continue;
            }
        }
        if keys.trim().is_empty() {
            continue;
        }
        if let Some(bad) = keys.split_whitespace().find(|k| gpui::Keystroke::parse(k).is_err()) {
            eprintln!("kynex: keymap.json: `{bad}` isn't a key for {name}; using {default}");
        }
        let keys = if keys.split_whitespace().all(|k| gpui::Keystroke::parse(k).is_ok()) {
            keys
        } else {
            crate::ui::keys(default)
        };
        bindings.push(match name {
            "open_project" => KeyBinding::new(keys, OpenProject, c),
            "open_settings" => KeyBinding::new(keys, OpenSettings, c),
            "new_session" => KeyBinding::new(keys, NewSession, c),
            "next_session" => KeyBinding::new(keys, NextSession, c),
            "prev_session" => KeyBinding::new(keys, PrevSession, c),
            "close_tab" => KeyBinding::new(keys, CloseTab, c),
            "next_tab" => KeyBinding::new(keys, NextTab, c),
            "prev_tab" => KeyBinding::new(keys, PrevTab, c),
            "toggle_sessions" => KeyBinding::new(keys, ToggleSessions, c),
            "toggle_terminal" => KeyBinding::new(keys, ToggleTerminal, c),
            "cycle_mode" => KeyBinding::new(keys, CycleMode, c),
            "search_history" => KeyBinding::new(keys, SearchHistory, c),
            "stash_draft" => KeyBinding::new(keys, StashDraft, c),
            "pick_model" => KeyBinding::new(keys, PickModel, c),
            "expand_tools" => KeyBinding::new(keys, ExpandTools, c),
            "edit_in_editor" => KeyBinding::new(keys, EditInEditor, c),
            "rewind" => KeyBinding::new(keys, Rewind, c),
            "toggle_timeline" => KeyBinding::new(keys, ToggleTimeline, c),
            "prev_turn" => KeyBinding::new(keys, PrevTurn, c),
            "next_turn" => KeyBinding::new(keys, NextTurn, c),
            "page_up" => KeyBinding::new(keys, PageUp, c),
            "page_down" => KeyBinding::new(keys, PageDown, c),
            "scroll_top" => KeyBinding::new(keys, ScrollTop, c),
            "scroll_bottom" => KeyBinding::new(keys, ScrollBottom, c),
            _ => KeyBinding::new(keys, Interrupt, c),
        });
    }
    for key in TRANSCRIPT_KEYS {
        bindings.push(match key {
            "home" => KeyBinding::new(key, ScrollTop, c),
            "end" => KeyBinding::new(key, ScrollBottom, c),
            "up" => KeyBinding::new(key, LineUp, c),
            _ => KeyBinding::new(key, LineDown, c),
        });
    }
    // Copies text selected in the transcript; the composer has its own Ctrl+C.
    bindings.push(KeyBinding::new(crate::ui::keys("secondary-c"), CopySelection, c));
    cx.bind_keys(bindings);
}

/// Puts the text selected in the transcript on the clipboard.
pub fn copy_selection(cx: &mut App) {
    match selectable::selected_text(cx) {
        Some(text) => cx.write_to_clipboard(gpui::ClipboardItem::new_string(text)),
        None => cx.propagate(),
    }
}

impl KynexApp {
    pub(super) fn open_project(&mut self, _: &OpenProject, _: &mut Window, cx: &mut Context<Self>) {
        self.pick_project(cx);
    }

    pub(super) fn open_settings_action(&mut self, _: &OpenSettings, _: &mut Window, cx: &mut Context<Self>) {
        self.open_app_settings(cx);
    }

    pub(super) fn new_session_action(&mut self, _: &NewSession, window: &mut Window, cx: &mut Context<Self>) {
        self.new_session(window, cx);
    }

    pub(super) fn next_session(&mut self, _: &NextSession, window: &mut Window, cx: &mut Context<Self>) {
        self.step_session(1, window, cx);
    }

    pub(super) fn prev_session(&mut self, _: &PrevSession, window: &mut Window, cx: &mut Context<Self>) {
        self.step_session(-1, window, cx);
    }

    /// The session `by` rows down the sessions panel (up if negative), in its
    /// order: pinned, ungrouped, then each group; the archive is skipped.
    fn step_session(&mut self, by: isize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(s) = self.active_session() else { return };
        let (kind, cwd, current) = (s.kind, s.cwd.clone(), s.session_id.clone());
        let items = match &self.sessions {
            Some(l) if l.cwd == cwd => l.items.iter().map(|i| i.id.clone()).collect::<Vec<_>>(),
            _ => kynex_core::session::list_sessions(&cwd).into_iter().map(|i| i.id).collect(),
        };
        let arranged = self.groups_for(&cwd).arrange(items.iter().map(String::as_str));
        let order: Vec<String> =
            arranged.pinned.into_iter().chain(arranged.top).chain(arranged.groups.into_iter().flatten()).collect();
        if order.is_empty() {
            return;
        }
        let n = order.len() as isize;
        // A new session sits above the list: down goes to the first, up to the last.
        let next = match current.and_then(|id| order.iter().position(|o| *o == id)) {
            Some(i) => (i as isize + by).rem_euclid(n),
            None if by > 0 => 0,
            None => n - 1,
        };
        self.show_code_view();
        self.switch_session(kind, cwd, order[next as usize].clone());
        self.focus_input(window, cx);
        cx.notify();
    }

    /// Ctrl+W: closes the project tab, like middle-clicking it. If Claude is
    /// working there, the first press only warns; a second within a few
    /// seconds closes it and stops the work.
    pub(super) fn close_tab(&mut self, _: &CloseTab, _: &mut Window, cx: &mut Context<Self>) {
        let Active::Project(i) = self.saved.active else { return };
        let Some(p) = self.saved.projects.get(i) else { return };
        let busy = self.code.get(&p.path).is_some_and(|l| l.sessions.iter().any(|s| s.busy));
        let confirmed = self.confirm_close.take().is_some_and(|(at, when)| at == i && when.elapsed().as_secs() < 4);
        if busy && !confirmed {
            self.confirm_close = Some((i, std::time::Instant::now()));
            self.toast(Toast::warning("Claude is still working here. Press again to close the tab and stop it."));
            return cx.notify();
        }
        self.close_project(i);
        cx.notify();
    }

    fn tab_count(&self) -> usize {
        self.saved.projects.len() + 1
    }

    /// The active tab's position in the title bar: Chat is 0.
    pub(super) fn tab_pos(&self) -> usize {
        match self.saved.active {
            Active::Chat => 0,
            Active::Project(i) => i + 1,
        }
    }

    pub(super) fn jump(&mut self, t: usize, window: &mut Window, cx: &mut Context<Self>) {
        if t < self.tab_count() {
            self.select(if t == 0 { Active::Chat } else { Active::Project(t - 1) }, window, cx);
        }
    }

    pub(super) fn next_tab(&mut self, _: &NextTab, window: &mut Window, cx: &mut Context<Self>) {
        let t = (self.tab_pos() + 1) % self.tab_count();
        self.jump(t, window, cx);
    }

    pub(super) fn prev_tab(&mut self, _: &PrevTab, window: &mut Window, cx: &mut Context<Self>) {
        let n = self.tab_count();
        let t = (self.tab_pos() + n - 1) % n;
        self.jump(t, window, cx);
    }

    pub(super) fn toggle_sessions(&mut self, _: &ToggleSessions, _: &mut Window, cx: &mut Context<Self>) {
        self.saved.sessions_collapsed = !self.saved.sessions_collapsed;
        self.save();
        cx.notify();
    }

    /// Esc by default: closes an open menu or the settings modal first, then
    /// stops a running reply. Otherwise the key goes on to whoever else wants it.
    pub(super) fn interrupt_action(&mut self, _: &Interrupt, _: &mut Window, cx: &mut Context<Self>) {
        if self.menu.take().is_some()
            || self.settings.take().is_some()
            || self.app_settings.take().is_some()
            || self.extensions.take().is_some()
            || self.cancel_rename()
        {
            return cx.notify();
        }
        if self.active_session().is_some_and(|s| s.busy) {
            self.interrupt(cx);
        } else {
            cx.propagate();
        }
    }

    /// Like Shift+Tab in the CLI: the next permission mode, live.
    pub(super) fn cycle_mode(&mut self, _: &CycleMode, _: &mut Window, cx: &mut Context<Self>) {
        let Some(s) = self.session_mut().filter(|s| s.kind == Kind::Code) else { return };
        let next = Mode::ALL.iter().position(|m| *m == s.mode).map_or(0, |i| (i + 1) % Mode::ALL.len());
        s.set_mode(Mode::ALL[next]);
        cx.notify();
    }

    /// Ctrl+R in the composer: search what you've sent.
    pub(super) fn search_history(&mut self, _: &SearchHistory, window: &mut Window, cx: &mut Context<Self>) {
        if !self.composer_focused(window, cx) {
            return cx.propagate();
        }
        self.start_history_search(cx);
    }

    /// Ctrl+S in the composer: stash the draft, or bring it back.
    pub(super) fn stash_action(&mut self, _: &StashDraft, window: &mut Window, cx: &mut Context<Self>) {
        if !self.composer_focused(window, cx) {
            return cx.propagate();
        }
        self.toggle_stash(cx);
    }

    /// Alt+P: the model picker, above the composer; the draft stays.
    pub(super) fn pick_model(&mut self, _: &PickModel, _: &mut Window, cx: &mut Context<Self>) {
        self.open_model_menu(cx);
    }

    /// Ctrl+G in the composer: write the message in your editor.
    pub(super) fn edit_in_editor_action(&mut self, _: &EditInEditor, window: &mut Window, cx: &mut Context<Self>) {
        if !self.composer_focused(window, cx) {
            return cx.propagate();
        }
        self.edit_in_editor(cx);
    }

    /// The rewind menu, from a key `keymap.json` gave it instead of Esc Esc.
    pub(super) fn rewind_action(&mut self, _: &Rewind, _: &mut Window, cx: &mut Context<Self>) {
        self.open_rewind(cx);
    }

    /// Claude Code's Ctrl+O: every tool call in the transcript open, or all closed again.
    pub(super) fn expand_tools(&mut self, _: &ExpandTools, _: &mut Window, cx: &mut Context<Self>) {
        self.expand_tools = !self.expand_tools;
        for th in self.threads.values() {
            th.list.remeasure();
        }
        cx.notify();
    }

    pub(super) fn key_down(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &ev.keystroke;
        // An image over everything closes first (it has the focus, so usually closes itself).
        if k.key == "escape" && self.viewer.is_some() {
            self.close_image(window, cx);
            cx.stop_propagation();
            return;
        }
        if k.key == "escape"
            && (self.menu.is_some()
                || self.settings.is_some()
                || self.app_settings.is_some()
                || self.extensions.is_some()
                || self.about_open
                || self.git_modal.is_some())
        {
            // The topmost one closes: a menu, then settings or About, then the Git modal.
            if self.menu.take().is_none()
                && self.settings.take().is_none()
                && self.app_settings.take().is_none()
                && self.extensions.take().is_none()
                && !std::mem::take(&mut self.about_open)
            {
                self.close_git_modal(cx);
            }
            cx.notify();
            cx.stop_propagation();
            return;
        }
        // Ctrl+1 is Chat, Ctrl+2 the first project, and so on.
        if k.modifiers.secondary(cx)
            && !k.modifiers.shift
            && !k.modifiers.alt
            && let Some(n) = k.key.parse::<usize>().ok().filter(|n| (1..=9).contains(n))
        {
            self.jump(n - 1, window, cx);
            cx.stop_propagation();
        }
    }
}
