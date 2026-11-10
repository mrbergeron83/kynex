//! Workspace actions, their user-editable key bindings, and the handlers that
//! aren't tied to one panel (tabs, Esc, Shift+Tab, Ctrl+1…9).

use std::collections::{BTreeMap, HashMap};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui::{App, Context, KeyBinding, KeyDownEvent, Window, actions};
use kynex_core::layout::Direction;
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
        NextWorkspace,
        PrevWorkspace,
        MoveTabLeft,
        MoveTabRight,
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
        OpenSettings,
        Quit,
        ZoomIn,
        ZoomOut,
        ZoomReset,
        CommandPalette,
        FindInTranscript,
        ListUp,
        ListDown,
        ListOpen,
        ListRename,
        ListArchive,
        ListDelete,
        SendPrefix,
        SplitRight,
        SplitDown,
        ZoomPane,
        ClosePane,
        NextPane,
        ResizeMode,
        NewPaneTab,
        NextPaneTab,
        PrevPaneTab,
        RenamePaneTab,
        ClosePaneTab,
        PickWorkspace,
        NewWorkspace,
        CloseWorkspace,
        KeyboardSettings,
        PickProject,
        ClosePaneOrTab
    ]
);

/// Focus the pane next to the focused one, that way.
#[derive(Clone, PartialEq, Debug, gpui::Action)]
#[action(namespace = kynex, no_json)]
pub struct FocusPane(pub Direction);

/// Swap the focused pane with the one next to it, that way.
#[derive(Clone, PartialEq, Debug, gpui::Action)]
#[action(namespace = kynex, no_json)]
pub struct SwapPane(pub Direction);

/// Show pane tab `n` (counting from 1).
#[derive(Clone, PartialEq, Debug, gpui::Action)]
#[action(namespace = kynex, no_json)]
pub struct JumpPaneTab(pub usize);

/// The sessions list's own keys, while it has the focus.
const LIST_KEYS: [(&str, &str); 6] = [
    ("up", "up"),
    ("down", "down"),
    ("enter", "open"),
    ("f2", "rename"),
    ("delete", "archive"),
    ("shift-delete", "delete"),
];

/// Workspace shortcuts and their defaults, Claude Code's where it has one.
/// `<data dir>/keymap.json` overrides them by name, e.g. `{ "interrupt": "ctrl-c" }`;
/// an empty string unbinds one.
const SHORTCUTS: [(&str, &str); 37] = [
    ("command_palette", "secondary-k"),
    ("find", "secondary-f"),
    ("open_project", "secondary-o"),
    ("open_settings", "secondary-,"),
    ("new_session", "secondary-n"),
    // The sessions panel's list, top to bottom. Brackets move around on
    // non-US layouts, so outside macOS it's the browsers' Ctrl+PageDown / Up.
    ("next_session", if cfg!(target_os = "macos") { "cmd-shift-]" } else { "ctrl-pagedown" }),
    ("prev_session", if cfg!(target_os = "macos") { "cmd-shift-[" } else { "ctrl-pageup" }),
    // Ctrl+Shift+] / [ as well, where they don't already belong to the line above.
    ("next_session_too", if cfg!(target_os = "macos") { "" } else { "ctrl-shift-]" }),
    ("prev_session_too", if cfg!(target_os = "macos") { "" } else { "ctrl-shift-[" }),
    // The project's checkouts (workspaces), as sessions are with Ctrl+PageDown / Up.
    ("next_workspace", "secondary-alt-pagedown"),
    ("prev_workspace", "secondary-alt-pageup"),
    ("close_tab", "secondary-w"),
    ("next_tab", "ctrl-tab"),
    ("prev_tab", "ctrl-shift-tab"),
    // The shown project tab one place left or right, as in browsers.
    ("move_tab_left", "ctrl-shift-pageup"),
    ("move_tab_right", "ctrl-shift-pagedown"),
    ("toggle_sessions", "prefix b"),
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
    // Closing the window may only hide Kynex; this always quits.
    ("quit", "secondary-q"),
    // The whole UI bigger or smaller, as in a browser.
    ("zoom_in", "secondary-="),
    ("zoom_out", "secondary--"),
    ("zoom_reset", "secondary-0"),
];

/// The prefix key, as `keymap.json` can change it: `prefix` in a shortcut stands for it.
const DEFAULT_PREFIX: &str = "ctrl-b";

/// Shortcuts that start with the prefix key (herdr / tmux style), and `secondary-w` in the
/// "Pane" context. The word `prefix` stands for the prefix key; names work as in `SHORTCUTS`.
const PREFIXED: &[(&str, &str)] = &[
    // Twice, the prefix key itself goes on to the shell in a terminal.
    ("send_prefix", "prefix prefix"),
    ("split_right", "prefix v"),
    ("split_down", "prefix -"),
    ("focus_left", "prefix h"),
    ("focus_down", "prefix j"),
    ("focus_up", "prefix k"),
    ("focus_right", "prefix l"),
    ("focus_left_arrow", "prefix left"),
    ("focus_down_arrow", "prefix down"),
    ("focus_up_arrow", "prefix up"),
    ("focus_right_arrow", "prefix right"),
    ("swap_left", "prefix shift-h"),
    ("swap_down", "prefix shift-j"),
    ("swap_up", "prefix shift-k"),
    ("swap_right", "prefix shift-l"),
    ("swap_left_arrow", "prefix shift-left"),
    ("swap_down_arrow", "prefix shift-down"),
    ("swap_up_arrow", "prefix shift-up"),
    ("swap_right_arrow", "prefix shift-right"),
    ("next_pane", "prefix tab"),
    ("zoom_pane", "prefix z"),
    ("close_pane", "prefix x"),
    // h / j / k / l or the arrows grow the pane; Esc or Enter is done.
    ("resize_panes", "prefix r"),
    ("new_pane_tab", "prefix c"),
    ("next_pane_tab", "prefix n"),
    ("prev_pane_tab", "prefix p"),
    ("pane_tab_1", "prefix 1"),
    ("pane_tab_2", "prefix 2"),
    ("pane_tab_3", "prefix 3"),
    ("pane_tab_4", "prefix 4"),
    ("pane_tab_5", "prefix 5"),
    ("pane_tab_6", "prefix 6"),
    ("pane_tab_7", "prefix 7"),
    ("pane_tab_8", "prefix 8"),
    ("pane_tab_9", "prefix 9"),
    ("rename_pane_tab", "prefix shift-t"),
    ("close_pane_tab", "prefix shift-x"),
    ("pick_workspace", "prefix w"),
    ("new_workspace", "prefix shift-n"),
    ("close_workspace", "prefix shift-d"),
    ("go_to", "prefix g"),
    ("keyboard_shortcuts", "prefix ?"),
    ("next_project", "prefix ]"),
    ("prev_project", "prefix ["),
    ("pick_project", "prefix shift-p"),
];

/// Keys that scroll the transcript when nothing focused (the message box, a
/// prompt) uses them, on top of the rebindable ones above.
const TRANSCRIPT_KEYS: [&str; 4] = ["home", "end", "up", "down"];

const ESC_ESC: &str = "escape escape";

/// Title bar places taken before the projects: Chat.
pub(super) const PROJECTS_AT: usize = 1;

static PREFIX: OnceLock<&'static str> = OnceLock::new();

/// The prefix key, as a keystroke `Keystroke::parse` takes (`ctrl-b` by default).
pub fn prefix_key() -> &'static str {
    PREFIX.get().copied().unwrap_or(DEFAULT_PREFIX)
}

/// `keys` with each `prefix` in it replaced by `prefix`, and `secondary` by the OS's key.
fn resolve(keys: &str, prefix: &str) -> &'static str {
    let words: Vec<&str> = keys.split_whitespace().map(|k| if k == "prefix" { prefix } else { k }).collect();
    crate::ui::keys(&words.join(" "))
}

/// Whether `keys` is the prefix key alone: it would make every prefixed shortcut wait.
fn is_prefix_alone(keys: &str, prefix: &str) -> bool {
    let words: Vec<&str> = keys.split_whitespace().collect();
    words.len() == 1 && crate::ui::keys(words[0]).eq_ignore_ascii_case(prefix)
}

/// The binding for a `PREFIXED` shortcut (in the "Workspace" context, or "Pane").
fn prefixed_binding(name: &str, keys: &'static str) -> Option<KeyBinding> {
    use Direction::{Down, Left, Right, Up};
    let c = Some("Workspace");
    let dir = |n: &str| match n {
        "left" => Left,
        "down" => Down,
        "up" => Up,
        _ => Right,
    };
    Some(match name {
        "send_prefix" => KeyBinding::new(keys, SendPrefix, c),
        "split_right" => KeyBinding::new(keys, SplitRight, c),
        "split_down" => KeyBinding::new(keys, SplitDown, c),
        "next_pane" => KeyBinding::new(keys, NextPane, c),
        "zoom_pane" => KeyBinding::new(keys, ZoomPane, c),
        "close_pane" => KeyBinding::new(keys, ClosePane, c),
        "resize_panes" => KeyBinding::new(keys, ResizeMode, c),
        "new_pane_tab" => KeyBinding::new(keys, NewPaneTab, c),
        "next_pane_tab" => KeyBinding::new(keys, NextPaneTab, c),
        "prev_pane_tab" => KeyBinding::new(keys, PrevPaneTab, c),
        "rename_pane_tab" => KeyBinding::new(keys, RenamePaneTab, c),
        "close_pane_tab" => KeyBinding::new(keys, ClosePaneTab, c),
        "pick_workspace" => KeyBinding::new(keys, PickWorkspace, c),
        "new_workspace" => KeyBinding::new(keys, NewWorkspace, c),
        "close_workspace" => KeyBinding::new(keys, CloseWorkspace, c),
        "go_to" => KeyBinding::new(keys, CommandPalette, c),
        "keyboard_shortcuts" => KeyBinding::new(keys, KeyboardSettings, c),
        "next_project" => KeyBinding::new(keys, NextTab, c),
        "prev_project" => KeyBinding::new(keys, PrevTab, c),
        "pick_project" => KeyBinding::new(keys, PickProject, c),
        _ => {
            let (what, rest) = name.split_once('_')?;
            let rest = rest.strip_suffix("_arrow").unwrap_or(rest);
            match what {
                "focus" => KeyBinding::new(keys, FocusPane(dir(rest)), c),
                "swap" => KeyBinding::new(keys, SwapPane(dir(rest)), c),
                "pane" => {
                    let n = rest.strip_prefix("tab_")?.parse().ok()?;
                    KeyBinding::new(keys, JumpPaneTab(n), c)
                }
                _ => return None,
            }
        }
    })
}

/// Whether Esc Esc opens the rewind menu: the default, unless `keymap.json` moved it.
static ESC_ESC_REWINDS: AtomicBool = AtomicBool::new(true);

pub(super) fn esc_esc_rewinds() -> bool {
    ESC_ESC_REWINDS.load(Ordering::Relaxed)
}

/// The keys above a terminal sends to its shell instead (see `terminal::view`).
pub const SHELL_KEYS: [&str; 16] = [
    "ctrl-k",
    "ctrl-f",
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
            let mut defaults: BTreeMap<_, _> = SHORTCUTS.iter().chain(PREFIXED).copied().collect();
            defaults.insert("prefix", DEFAULT_PREFIX);
            if let Ok(s) = serde_json::to_string_pretty(&defaults) {
                let _ = std::fs::create_dir_all(data_dir());
                let _ = std::fs::write(&path, s);
            }
            HashMap::new()
        }
    };
    // Workspaces were "contexts": a keymap.json written then still binds them.
    let mut user = user;
    for (old, new) in [("next_context", "next_workspace"), ("prev_context", "prev_workspace")] {
        if let Some(keys) = user.remove(old) {
            user.entry(new.to_string()).or_insert(keys);
        }
    }
    // The prefix is one keystroke; anything else is ignored.
    let prefix = user
        .get("prefix")
        .map(|p| crate::ui::keys(p))
        .filter(|p| {
            let ok = p.split_whitespace().count() == 1 && gpui::Keystroke::parse(p).is_ok();
            if !ok {
                eprintln!("kynex: keymap.json: `{p}` isn't one key for prefix; using {DEFAULT_PREFIX}");
            }
            ok
        })
        .unwrap_or(DEFAULT_PREFIX);
    let _ = PREFIX.set(prefix);
    let c = Some("Workspace");
    let mut bindings = Vec::new();
    // Ctrl+W in a pane closes the pane while the tab has several: on whatever keys `close_tab` has.
    let mut close_keys = None;
    for &(name, default) in SHORTCUTS.iter().chain(PREFIXED) {
        let default = resolve(default, prefix);
        // The old default for the sessions column (written out on first run) would make
        // every prefix wait; so would any key that is the prefix alone.
        let user_keys = user.get(name).map(String::as_str).filter(|k| {
            // Only what Kynex wrote itself on first run (`secondary-b`), or the same typed as `cmd-b`.
            let old = name == "toggle_sessions"
                && (k.trim() == "secondary-b" || (cfg!(target_os = "macos") && k.trim() == "cmd-b"));
            let alone = is_prefix_alone(k, prefix);
            if alone && !old {
                eprintln!("kynex: keymap.json: {name} can't be the prefix key alone; using {default}");
            }
            !old && !alone
        });
        let keys = user_keys.map_or(default, |k| resolve(k, prefix));
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
        let keys = if keys.split_whitespace().all(|k| gpui::Keystroke::parse(k).is_ok()) { keys } else { default };
        if name == "close_tab" {
            close_keys = Some(keys);
        }
        if let Some(binding) = prefixed_binding(name, keys) {
            bindings.push(binding);
            continue;
        }
        bindings.push(match name {
            "command_palette" => KeyBinding::new(keys, CommandPalette, c),
            "find" => KeyBinding::new(keys, FindInTranscript, c),
            "open_project" => KeyBinding::new(keys, OpenProject, c),
            "open_settings" => KeyBinding::new(keys, OpenSettings, c),
            "new_session" => KeyBinding::new(keys, NewSession, c),
            "next_session" | "next_session_too" => KeyBinding::new(keys, NextSession, c),
            "prev_session" | "prev_session_too" => KeyBinding::new(keys, PrevSession, c),
            "next_workspace" => KeyBinding::new(keys, NextWorkspace, c),
            "prev_workspace" => KeyBinding::new(keys, PrevWorkspace, c),
            "close_tab" => KeyBinding::new(keys, CloseTab, c),
            "next_tab" => KeyBinding::new(keys, NextTab, c),
            "prev_tab" => KeyBinding::new(keys, PrevTab, c),
            "move_tab_left" => KeyBinding::new(keys, MoveTabLeft, c),
            "move_tab_right" => KeyBinding::new(keys, MoveTabRight, c),
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
            "quit" => KeyBinding::new(keys, Quit, c),
            "zoom_in" => KeyBinding::new(keys, ZoomIn, c),
            "zoom_out" => KeyBinding::new(keys, ZoomOut, c),
            "zoom_reset" => KeyBinding::new(keys, ZoomReset, c),
            _ => KeyBinding::new(keys, Interrupt, c),
        });
    }
    if let Some(keys) = close_keys {
        bindings.push(KeyBinding::new(keys, ClosePaneOrTab, Some("Pane")));
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
    // The sessions list's keys: a deeper context, so they win over the transcript's ↑ / ↓.
    let list = Some("SessionList");
    for (key, what) in LIST_KEYS {
        bindings.push(match what {
            "up" => KeyBinding::new(key, ListUp, list),
            "down" => KeyBinding::new(key, ListDown, list),
            "open" => KeyBinding::new(key, ListOpen, list),
            "rename" => KeyBinding::new(key, ListRename, list),
            "archive" => KeyBinding::new(key, ListArchive, list),
            _ => KeyBinding::new(key, ListDelete, list),
        });
    }
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
        let Some(s) = self.focused_key().and_then(|k| self.session_by_key(k)) else { return };
        let cwd = s.cwd.clone();
        let Some(list) = self.sessions.as_ref().filter(|l| l.read.cwd == cwd) else {
            // The panel is collapsed, so its list isn't read: read it off the UI thread first.
            let dir = cwd.clone();
            cx.spawn_in(window, async move |this, cx| {
                let list = async move { kynex_core::session::list_sessions(&dir) };
                let items = gpui::AppContext::background_spawn(cx, list).await;
                let ids = items.into_iter().map(|i| i.id).collect();
                this.update_in(cx, |this, window, cx| this.step_through(ids, by, window, cx)).ok();
            })
            .detach();
            return;
        };
        let items: Vec<String> = list.items.iter().map(|i| i.id.clone()).collect();
        self.step_through(items, by, window, cx);
    }

    /// [`Self::step_session`] in the folder's sessions `items`, newest first.
    fn step_through(&mut self, items: Vec<String>, by: isize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(s) = self.focused_key().and_then(|k| self.session_by_key(k)) else { return };
        let (kind, cwd, current) = (s.kind, s.cwd.clone(), s.session_id.clone());
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
        self.saved.projects.len() + PROJECTS_AT
    }

    /// The active tab's position in the title bar: Chat is 0, then the projects.
    pub(super) fn tab_pos(&self) -> usize {
        match self.saved.active {
            Active::Chat => 0,
            Active::Project(i) => i + PROJECTS_AT,
        }
    }

    pub(super) fn jump(&mut self, t: usize, window: &mut Window, cx: &mut Context<Self>) {
        if t >= self.tab_count() {
            return;
        }
        if t < PROJECTS_AT {
            self.select(Active::Chat, window, cx);
            self.focus_input(window, cx);
        } else {
            self.select(Active::Project(t - PROJECTS_AT), window, cx);
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

    pub(super) fn next_workspace(&mut self, _: &NextWorkspace, _: &mut Window, cx: &mut Context<Self>) {
        self.step_workspace(1, cx);
    }

    pub(super) fn prev_workspace(&mut self, _: &PrevWorkspace, _: &mut Window, cx: &mut Context<Self>) {
        self.step_workspace(-1, cx);
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
            || self.palette.take().is_some()
            || self.close_scratch(cx)
            || self.settings.take().is_some()
            || self.app_settings.take().is_some()
            || self.extensions.take().is_some()
            || self.cancel_rename()
            || self.cancel_tab_rename(cx)
            || self.cancel_new_workspace()
        {
            return cx.notify();
        }
        if self.focused_key().and_then(|k| self.session_by_key(k)).is_some_and(|s| s.busy) {
            self.interrupt(cx);
        } else {
            cx.propagate();
        }
    }

    /// Like Shift+Tab in the CLI: the next permission mode, live.
    pub(super) fn cycle_mode(&mut self, _: &CycleMode, _: &mut Window, cx: &mut Context<Self>) {
        let key = self.composer_key();
        let Some(s) = key.and_then(|k| self.session_for(k)).filter(|s| s.kind == Kind::Code) else {
            return;
        };
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
        self.pane.expand_tools = !self.pane.expand_tools;
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
        // The scratch explorer: its preview, then the modal.
        if k.key == "escape" && self.menu.is_none() && self.close_scratch(cx) {
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
        // Picking sessions in the panel stops.
        if k.key == "escape" && self.stop_picking() {
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
