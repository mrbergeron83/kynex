//! Workspace actions, their user-editable key bindings, and the handlers that
//! aren't tied to one panel (tabs, Esc, Ctrl+1…9).

use std::collections::{BTreeMap, HashMap};
use std::sync::OnceLock;

use gpui::{App, Context, KeyBinding, KeyDownEvent, Window, actions};
use kynex_core::layout::Direction;
use kynex_core::paths::data_dir;
use kynex_core::toasts::Toast;

use super::KynexApp;

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
        Interrupt,
        ToggleTerminal,
        ToggleGit,
        ToggleAgents,
        OpenSettings,
        Quit,
        ZoomIn,
        ZoomOut,
        ZoomReset,
        CommandPalette,
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
        PickWorkspace,
        NewWorkspace,
        CloseWorkspace,
        KeyboardSettings,
        PickProject,
        NextNeedsYou,
        ClosePaneOrTab,
        UndoLayout,
        RedoLayout,
        ReopenPane,
        LayoutSnapshots,
        CopyMode
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

/// Open a file linked in text in the file viewer (`line` is 1-based).
#[derive(Clone, PartialEq, Debug, gpui::Action)]
#[action(namespace = kynex, no_json)]
pub struct OpenFileLink {
    pub path: std::path::PathBuf,
    pub line: Option<u32>,
}

/// Run the custom command of this name (`kynex_core::user_commands`).
#[derive(Clone, PartialEq, Debug, gpui::Action)]
#[action(namespace = kynex, no_json)]
pub struct RunCommand(pub String);

/// The sessions list's own keys, while it has the focus.
const LIST_KEYS: [(&str, &str); 6] = [
    ("up", "up"),
    ("down", "down"),
    ("enter", "open"),
    ("f2", "rename"),
    ("delete", "archive"),
    ("shift-delete", "delete"),
];

/// Workspace shortcuts and their defaults. `<data dir>/keymap.json` overrides them by name;
/// an empty string unbinds one.
const SHORTCUTS: [(&str, &str); 25] = [
    ("command_palette", "secondary-k"),
    ("open_project", "secondary-o"),
    ("open_settings", "secondary-,"),
    ("new_session", "secondary-n"),
    ("next_session", if cfg!(target_os = "macos") { "cmd-shift-]" } else { "ctrl-pagedown" }),
    ("prev_session", if cfg!(target_os = "macos") { "cmd-shift-[" } else { "ctrl-pageup" }),
    ("next_session_too", if cfg!(target_os = "macos") { "" } else { "ctrl-shift-]" }),
    ("prev_session_too", if cfg!(target_os = "macos") { "" } else { "ctrl-shift-[" }),
    ("next_workspace", "secondary-alt-pagedown"),
    ("prev_workspace", "secondary-alt-pageup"),
    ("close_tab", "secondary-w"),
    ("next_tab", "ctrl-tab"),
    ("prev_tab", "ctrl-shift-tab"),
    ("move_tab_left", "ctrl-shift-pageup"),
    ("move_tab_right", "ctrl-shift-pagedown"),
    ("toggle_sessions", "prefix b"),
    ("toggle_terminal", "ctrl-`"),
    ("interrupt", "escape"),
    ("toggle_git_pane", "secondary-shift-g"),
    ("toggle_agents", "secondary-shift-j"),
    ("next_needs_you", "secondary-shift-a"),
    ("quit", "secondary-q"),
    ("zoom_in", "secondary-="),
    ("zoom_out", "secondary--"),
    ("zoom_reset", "secondary-0"),
];

/// The prefix key, as `keymap.json` can change it: `prefix` in a shortcut stands for it.
const DEFAULT_PREFIX: &str = "ctrl-b";

/// Shortcuts that start with the prefix key (tmux style).
const PREFIXED: &[(&str, &str)] = &[
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
    ("resize_panes", "prefix r"),
    ("pick_workspace", "prefix w"),
    ("new_workspace", "prefix shift-n"),
    ("close_workspace", "prefix shift-d"),
    ("go_to", "prefix g"),
    ("keyboard_shortcuts", "prefix ?"),
    ("next_project", "prefix ]"),
    ("prev_project", "prefix ["),
    ("pick_project", "prefix shift-p"),
    ("undo_layout", "prefix u"),
    ("redo_layout", "prefix shift-u"),
    ("reopen_pane", "prefix shift-r"),
    ("layout_snapshots", "prefix s"),
    ("copy_mode", "prefix y"),
];

/// Title bar places taken before the projects (none — Kynex is pure project tabs).
pub(super) const PROJECTS_AT: usize = 0;

static PREFIX: OnceLock<&'static str> = OnceLock::new();
static IN_EFFECT: OnceLock<Vec<(&'static str, String)>> = OnceLock::new();

pub fn in_effect() -> &'static [(&'static str, String)] {
    IN_EFFECT.get().map_or(&[], Vec::as_slice)
}

pub fn prefix_key() -> &'static str {
    PREFIX.get().copied().unwrap_or(DEFAULT_PREFIX)
}

fn resolve(keys: &str, prefix: &str) -> &'static str {
    let words: Vec<&str> = keys.split_whitespace().map(|k| if k == "prefix" { prefix } else { k }).collect();
    crate::ui::keys(&words.join(" "))
}

fn is_prefix_alone(keys: &str, prefix: &str) -> bool {
    let words: Vec<&str> = keys.split_whitespace().collect();
    words.len() == 1 && crate::ui::keys(words[0]).eq_ignore_ascii_case(prefix)
}

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
        "pick_workspace" => KeyBinding::new(keys, PickWorkspace, c),
        "new_workspace" => KeyBinding::new(keys, NewWorkspace, c),
        "close_workspace" => KeyBinding::new(keys, CloseWorkspace, c),
        "go_to" => KeyBinding::new(keys, CommandPalette, c),
        "keyboard_shortcuts" => KeyBinding::new(keys, KeyboardSettings, c),
        "next_project" => KeyBinding::new(keys, NextTab, c),
        "prev_project" => KeyBinding::new(keys, PrevTab, c),
        "pick_project" => KeyBinding::new(keys, PickProject, c),
        "undo_layout" => KeyBinding::new(keys, UndoLayout, c),
        "redo_layout" => KeyBinding::new(keys, RedoLayout, c),
        "reopen_pane" => KeyBinding::new(keys, ReopenPane, c),
        "copy_mode" => KeyBinding::new(keys, CopyMode, c),
        "layout_snapshots" => KeyBinding::new(keys, LayoutSnapshots, c),
        _ => {
            let (what, rest) = name.split_once('_')?;
            let rest = rest.strip_suffix("_arrow").unwrap_or(rest);
            match what {
                "focus" => KeyBinding::new(keys, FocusPane(dir(rest)), c),
                "swap" => KeyBinding::new(keys, SwapPane(dir(rest)), c),
                _ => return None,
            }
        }
    })
}

/// Keys a terminal/CLI pane sends directly to its PTY instead of letting Workspace take them.
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
            let mut defaults: BTreeMap<_, _> = SHORTCUTS.iter().chain(PREFIXED).copied().collect();
            defaults.insert("prefix", DEFAULT_PREFIX);
            if let Ok(s) = serde_json::to_string_pretty(&defaults) {
                let _ = std::fs::create_dir_all(data_dir());
                let _ = std::fs::write(&path, s);
            }
            HashMap::new()
        }
    };
    let mut user = user;
    for (old, new) in [("next_context", "next_workspace"), ("prev_context", "prev_workspace")] {
        if let Some(keys) = user.remove(old) {
            user.entry(new.to_string()).or_insert(keys);
        }
    }
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
    let mut close_keys = None;
    let mut in_effect = Vec::new();
    for &(name, default) in SHORTCUTS.iter().chain(PREFIXED) {
        let default = resolve(default, prefix);
        let user_keys = user.get(name).map(String::as_str).filter(|k| {
            let old = name == "toggle_sessions"
                && (k.trim() == "secondary-b" || (cfg!(target_os = "macos") && k.trim() == "cmd-b"));
            let alone = is_prefix_alone(k, prefix);
            if alone && !old {
                eprintln!("kynex: keymap.json: {name} can't be the prefix key alone; using {default}");
            }
            !old && !alone
        });
        let keys = user_keys.map_or(default, |k| resolve(k, prefix));
        if keys.trim().is_empty() {
            in_effect.push((name, String::new()));
            continue;
        }
        if let Some(bad) = keys.split_whitespace().find(|k| gpui::Keystroke::parse(k).is_err()) {
            eprintln!("kynex: keymap.json: `{bad}` isn't a key for {name}; using {default}");
        }
        let keys = if keys.split_whitespace().all(|k| gpui::Keystroke::parse(k).is_ok()) { keys } else { default };
        in_effect.push((name, keys.to_string()));
        if name == "close_tab" {
            close_keys = Some(keys);
        }
        if let Some(binding) = prefixed_binding(name, keys) {
            bindings.push(binding);
            continue;
        }
        bindings.push(match name {
            "command_palette" => KeyBinding::new(keys, CommandPalette, c),
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
            "toggle_git_pane" => KeyBinding::new(keys, ToggleGit, c),
            "toggle_agents" => KeyBinding::new(keys, ToggleAgents, c),
            "next_needs_you" => KeyBinding::new(keys, NextNeedsYou, c),
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
    let _ = IN_EFFECT.set(in_effect);
    cx.bind_keys(bindings);
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

    fn step_session(&mut self, by: isize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(cwd) = self.workspace_cwd() else { return };
        let Some(list) = self.sessions.get(&cwd) else {
            let dir = cwd.clone();
            cx.spawn_in(window, async move |this, cx| {
                let list = async move { kynex_core::sessions::list_sessions(&dir) };
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

    fn step_through(&mut self, items: Vec<String>, by: isize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(cwd) = self.workspace_cwd() else { return };
        let current_agent = self.focused_agent_id();
        let current_sid = self.focused_session_id();
        let fresh: Vec<String> = self
            .live_agents_in(&cwd)
            .into_iter()
            .rev()
            .filter(|(_, sid, _, _)| sid.as_ref().is_none_or(|s| !items.contains(s)))
            .map(|(id, sid, _, _)| sid.unwrap_or_else(|| super::session_rows::new_key(id)))
            .collect();
        let arranged = self.groups_for(&cwd).arrange(items.iter().map(String::as_str));
        let order: Vec<String> = arranged
            .pinned
            .into_iter()
            .chain(fresh)
            .chain(arranged.top)
            .chain(arranged.groups.into_iter().flatten())
            .collect();
        if order.is_empty() {
            return;
        }
        let current =
            current_sid.filter(|s| order.contains(s)).or_else(|| current_agent.map(super::session_rows::new_key));
        let n = order.len() as isize;
        let next = match current.and_then(|id| order.iter().position(|o| *o == id)) {
            Some(i) => (i as isize + by).rem_euclid(n),
            None if by > 0 => 0,
            None => n - 1,
        };
        let target = &order[next as usize];
        if let Some(agent_id) = super::session_rows::key_agent(target) {
            self.go_to_agent(agent_id, window, cx);
        } else {
            self.switch_session(cwd, target.clone(), window, cx);
        }
        self.focus_input(window, cx);
        cx.notify();
    }

    pub(super) fn close_tab(&mut self, _: &CloseTab, _: &mut Window, cx: &mut Context<Self>) {
        let Some(i) = self.saved.active_project else { return };
        let Some(p) = self.saved.projects.get(i) else { return };
        let busy = self.project_has_working_agent(&p.path);
        let confirmed = self.confirm_close.take().is_some_and(|(at, when)| at == i && when.elapsed().as_secs() < 4);
        if busy && !confirmed {
            self.confirm_close = Some((i, std::time::Instant::now()));
            self.toast(Toast::warning("An agent is still working here. Press again to close the tab."));
            return cx.notify();
        }
        self.close_project(i, cx);
        cx.notify();
    }

    fn tab_count(&self) -> usize {
        self.saved.projects.len()
    }

    pub(super) fn tab_pos(&self) -> Option<usize> {
        self.saved.active_project
    }

    pub(super) fn jump(&mut self, t: usize, window: &mut Window, cx: &mut Context<Self>) {
        if t < self.saved.projects.len() {
            self.select_project(t, window, cx);
        }
    }

    pub(super) fn next_tab(&mut self, _: &NextTab, window: &mut Window, cx: &mut Context<Self>) {
        let n = self.tab_count();
        if n == 0 {
            return;
        }
        let t = (self.tab_pos().unwrap_or(0) + 1) % n;
        self.jump(t, window, cx);
    }

    pub(super) fn prev_tab(&mut self, _: &PrevTab, window: &mut Window, cx: &mut Context<Self>) {
        let n = self.tab_count();
        if n == 0 {
            return;
        }
        let t = (self.tab_pos().unwrap_or(0) + n - 1) % n;
        self.jump(t, window, cx);
    }

    pub(super) fn next_workspace(&mut self, _: &NextWorkspace, _: &mut Window, cx: &mut Context<Self>) {
        self.step_workspace(1, cx);
    }

    pub(super) fn prev_workspace(&mut self, _: &PrevWorkspace, _: &mut Window, cx: &mut Context<Self>) {
        self.step_workspace(-1, cx);
    }

    pub(super) fn toggle_sessions(&mut self, _: &ToggleSessions, window: &mut Window, cx: &mut Context<Self>) {
        self.toggle_side_panes(window, cx);
    }

    /// Esc by default: closes open overlays/menus/popups first; otherwise propagates
    /// so interactive CLIs receive Escape untouched.
    pub(super) fn interrupt_action(&mut self, _: &Interrupt, _: &mut Window, cx: &mut Context<Self>) {
        if self.menu.take().is_some()
            || self.palette.take().is_some()
            || self.app_settings.take().is_some()
            || std::mem::take(&mut self.about_open)
            || self.cancel_rename()
            || self.cancel_new_workspace()
            || self.stop_picking()
        {
            return cx.notify();
        }
        cx.propagate();
    }

    pub(super) fn key_down(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &ev.keystroke;
        if k.key == "escape" && self.git_file.is_some() {
            self.close_git_file_modal(cx);
            self.focus_input(window, cx);
            cx.stop_propagation();
            return;
        }
        if k.key == "escape" && (self.menu.is_some() || self.app_settings.is_some() || self.about_open) {
            if self.menu.take().is_none() && self.app_settings.take().is_none() {
                self.about_open = false;
            }
            cx.notify();
            cx.stop_propagation();
            return;
        }
        if k.key == "escape" && self.stop_picking() {
            cx.notify();
            cx.stop_propagation();
            return;
        }
        // Ctrl+1..9 jumps to project tab 0..8.
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

#[cfg(test)]
mod tests;
