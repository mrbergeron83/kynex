//! Workspace actions, their user-editable key bindings, and the handlers that
//! aren't tied to one panel (tabs, Esc, Shift+Tab, Ctrl+1…9).

use std::collections::{BTreeMap, HashMap};

use gpui::{App, Context, KeyBinding, KeyDownEvent, Window, actions};
use kynex_core::paths::data_dir;
use kynex_core::project::Mode;
use kynex_core::session::Kind;

use super::KynexApp;
use super::state::Active;
use crate::ui::selectable;

actions!(kynex, [OpenProject, NextTab, PrevTab, ToggleSessions, CycleMode, Interrupt, CopySelection, ToggleTerminal]);

/// Workspace shortcuts and their defaults. `<data dir>/keymap.json` overrides
/// them by name, e.g. `{ "interrupt": "ctrl-c" }`; an empty string unbinds one.
const SHORTCUTS: [(&str, &str); 7] = [
    ("open_project", "secondary-o"),
    ("next_tab", "ctrl-tab"),
    ("prev_tab", "ctrl-shift-tab"),
    ("toggle_sessions", "secondary-b"),
    ("toggle_terminal", "ctrl-`"),
    ("cycle_mode", "shift-tab"),
    ("interrupt", "escape"),
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
            "next_tab" => KeyBinding::new(keys, NextTab, c),
            "prev_tab" => KeyBinding::new(keys, PrevTab, c),
            "toggle_sessions" => KeyBinding::new(keys, ToggleSessions, c),
            "toggle_terminal" => KeyBinding::new(keys, ToggleTerminal, c),
            "cycle_mode" => KeyBinding::new(keys, CycleMode, c),
            _ => KeyBinding::new(keys, Interrupt, c),
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
        if self.menu.take().is_some() || self.settings.take().is_some() {
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

    pub(super) fn key_down(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &ev.keystroke;
        if k.key == "escape" && (self.menu.is_some() || self.settings.is_some() || self.about_open) {
            if self.menu.take().is_none() {
                self.settings = None;
                self.about_open = false;
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
