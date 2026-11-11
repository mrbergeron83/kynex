//! `KynexApp`, the root view. This file holds its state, startup and the
//! top-level layout; each part of the window is an `impl KynexApp` block in
//! its own file:
//!
//! | file                | what                                                 |
//! |---------------------|------------------------------------------------------|
//! | `state.rs`          | `Saved`: what persists to `state.json`               |
//! | `keys.rs`           | actions, `keymap.json`, tab / Esc / mode shortcuts   |
//! | `layout.rs`         | Code and Git layouts, overlays, shortcuts, resize drags |
//! | `projects.rs`       | tabs, projects, and which session is shown           |
//! | `title_bar.rs`      | title bar and project tabs                           |
//! | `footer.rs`         | status footer: Kynex's CPU / memory, meters, update  |
//! | `status.rs`         | the footer's usage-limits meter and update pill      |
//! | `pane_actions.rs`  | a pane's header buttons and menus, opening a session beside one |
//! | `pane_keys.rs`     | the prefix keys (Ctrl+B …): panes, pane tabs, workspaces, projects, resize mode |
//! | `pane_tabs.rs`     | the selected workspace's tab bar: new, close, rename, reorder |
//! | `tiles.rs`          | the split panes of a workspace: layouts, focus, drawing |
//! | `custom_themes.rs`  | themes from JSON files in the data dir: loading, watching, applying |
//! | `control.rs`        | the control socket: `knx project / workspace / pane / session / events` |
//! | `control_workspaces.rs` | its `workspace` commands: list, show, make, remove |
//! | `layouts.rs`        | saving each workspace's panes and bringing them back |
//! | `layout_history.rs` | undo / redo of pane changes, reopening a pane, older layouts |
//! | `toolbar.rs`        | Code / Git switch, path, cost, open-in buttons       |
//! | `sessions_panel.rs` | open and past sessions                               |
//! | `session_groups.rs` | session groups and the archive, their menus          |
//! | `session_select.rs` | picking sessions to archive or delete together       |
//! | `session_tidy.rs`   | many sessions at once: menu, tidying, list keys      |
//! | `session_header.rs` | the session's title, branch / worktree, turns, cost  |
//! | `terminal_panes.rs` | terminal panes: Ctrl+`, run buttons, hide and show   |
//! | `tool_panes.rs`     | the Git view and the timeline as panes               |
//! | `custom_commands.rs`| custom commands: keys, popup, pane, background       |
//! | `agent_panes.rs`    | other agent CLIs in panes, their state off the screen |
//! | `tasks.rs`          | background tasks: running strip, ended rows          |
//! | `toasts.rs`         | the toast stack: task endings, updates, errors       |
//! | `titles.rs`         | naming sessions Claude Code didn't name              |
//! | `transcript.rs`     | the conversation list, its scrolling and follow-the-tail |
//! | `transcript_rows.rs` | how each transcript row draws: messages, results, the turn's end |
//! | `find.rs`           | find in the transcript (Ctrl+F)                      |
//! | `tool_rows.rs`      | tool calls with their results, runs of calls folded  |
//! | `worktrees.rs`      | the project's worktrees, and which one Claude is in  |
//! | `workspaces.rs`     | workspaces: listing, selecting, their folders        |
//! | `workspace_list.rs` | the workspaces and their open sessions in the sidebar |
//! | `workspace_new.rs`  | making and removing workspaces, folding worktree tabs |
//! | `git_modal.rs`      | a worktree's Git view in a modal, from the header    |
//! | `scratch_modal.rs`  | the file explorer in a modal: session or project     |
//! | `composer.rs`       | message box, attachments, pickers, send / stop       |
//! | `drafts.rs`         | each session's unsent message, kept across switches  |
//! | `queue.rs`          | messages queued while Claude works, Send now         |
//! | `handoff.rs`        | "waiting for you in the browser" banner, Continue    |
//! | `trust.rs`          | "trust this folder?" strip, as the CLI's dialog       |
//! | `attention.rs`      | OS notifications and the icon's "needs you" dot     |
//! | `background.rs`     | close to tray / Dock, keep awake, `claude` started early, idle sessions sleep |
//! | `history.rs`        | ↑ / ↓ history, Ctrl+R search, stash, Esc Esc, editor |
//! | `rewind.rs`         | rewind to an earlier message: conversation and files |
//! | `timeline.rs`       | the session's turns and steps beside the transcript  |
//! | `slash.rs`          | the slash-command popup, and built-ins Kynex runs   |
//! | `mentions.rs`       | the `@file` mention popup                            |
//! | `prompts.rs`        | permission / plan / question prompts                 |
//! | `menus.rs`          | popup menus                                          |
//! | `palette.rs`        | the command palette (Ctrl+K)                         |
//! | `palette_filters.rs`| its state filters (All, Needs you, Working, Idle, Done) |
//! | `palette_more.rs`   | its rows for pane moves, agent panes, custom commands |
//! | `palette_sessions.rs`| its session rows: per workspace, and open ones not listed yet |
//! | `goto.rs`           | revealing a session in its project, workspace and pane; next that needs you |
//! | `leaving.rs`        | menus, modals and panels animating out as they close |
//! | `settings.rs`       | the project's Claude settings modal                  |
//! | `project_setup.rs`  | its "Setup": trust, mode, ask rules, starter rules   |
//! | `rules_view.rs`     | its "Rules in effect": every rule, a command tester  |
//! | `delegate_view.rs`  | its "Delegate to Haiku": on / off, the rule told      |
//! | `app_settings.rs`   | the app's Settings modal: CLI, shell, appearance     |
//! | `agents_settings.rs` | its Agents: delegating, Kynex's subagents to edit       |
//! | `providers.rs`      | Settings → Providers: Anthropic sign-in, OpenRouter key, cap, My models |
//! | `grok_setup.rs`     | Settings → Providers → Grok: opt in, install, sign in / out |
//! | `setup.rs`          | CLI install / sign-in screen and update strip        |
//! | `provider_setup.rs` | OpenRouter key on launch, new sessions' provider, OpenRouter on the setup screen |
//! | `app_update.rs`     | Kynex updating itself: checks, the update toast      |
//! | `about.rs`          | the About Kynex modal: version, paths, updates       |
//! | `storage.rs`        | Settings → Storage: sessions' disk use, cleanup      |
//! | `storage_list.rs`   | its sessions: filter, sort, go to, files             |
//! | `extensions.rs`     | the Extensions modal: its state, loading, sections   |
//! | `extensions_servers.rs` | MCP servers: status, tools, errors, restart, sign-in, test |
//! | `extensions_add.rs` | its Add server panel: browser preset, your own       |
//! | `extensions_plugins.rs` | plugins: what they add, actions, marketplaces    |
//! | `extensions_catalog.rs` | skills & commands, agents, hooks (read-only)     |
//! | `appearance.rs`     | each project's colour: title bar, tab, composer      |
//! | `models.rs`         | the model list, and the picker's families / versions |
//! | `viewer.rs`         | images opened over the window, thumbnails            |

mod about;
mod agent_panes;
mod agents_settings;
mod app_settings;
mod app_update;
mod appearance;
mod attention;
mod background;
mod composer;
mod control;
mod control_workspaces;
mod custom_commands;
mod custom_themes;
mod delegate_view;
mod drafts;
mod extensions;
mod extensions_add;
mod extensions_catalog;
mod extensions_plugins;
mod extensions_servers;
pub(crate) mod find;
mod footer;
mod git_modal;
mod goto;
mod grok_setup;
mod handoff;
pub(crate) mod history;
mod keys;
mod layout;
mod layout_history;
mod layouts;
mod leaving;
pub(crate) mod mentions;
mod menus;
mod models;
mod palette;
mod palette_filters;
mod palette_more;
mod palette_sessions;
mod pane_actions;
mod pane_keys;
mod pane_tabs;
mod project_setup;
mod projects;
pub(crate) mod prompts;
mod provider_setup;
mod providers;
mod queue;
pub(crate) mod rewind;
mod rules_view;
mod scratch_modal;
mod session_groups;
mod session_header;
mod session_select;
mod session_tidy;
mod sessions_panel;
mod settings;
mod setup;
pub(crate) mod slash;
mod state;
mod status;
mod storage;
mod storage_list;
mod tasks;
mod terminal_panes;
mod tiles;
pub(crate) mod timeline;
mod title_bar;
mod titles;
mod toasts;
mod tool_panes;
mod tool_rows;
mod toolbar;
mod transcript;
mod transcript_rows;
mod trust;
mod viewer;
mod workspace_list;
mod workspace_new;
mod workspaces;
mod worktrees;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use gpui::{App, Context, Entity, FocusHandle, Focusable, Window, WindowBounds, div, prelude::*};
use kynex_core::cli::UpdateState;
use kynex_core::live::{Key, Live};
use kynex_core::paths::chat_dir;
use kynex_core::project::ClaudeSettings;
use kynex_core::session::{self, Kind, Session};

pub use control::OPEN_AT_START;
pub use custom_commands::bind_command_keys;
use drafts::Draft;
pub use keys::{SHELL_KEYS, SendPrefix, ToggleTerminal, bind_keys, prefix_key};
use menus::OpenMenu;
use palette::PaletteView;
use scratch_modal::ScratchModal;
use session_groups::Organize;
use sessions_panel::{ListRead, SessionList};
use settings::SettingsView;
use setup::{CliCheck, Setup};
pub use state::Saved;
use state::{View, WindowState};
use storage::StorageView;
use transcript::Thread;
use worktrees::Worktrees;

use crate::git::GitView;
use crate::pane::SessionPane;
use crate::terminal::TerminalPanel;
use crate::theme::{theme, ui_font};
use crate::ui::image_viewer::ImageViewer;
use crate::ui::input::{InputEvent, TextInput};
use crate::ui::smooth_scroll::SmoothWheel;

pub struct KynexApp {
    saved: Saved,
    code: HashMap<PathBuf, Live>,
    chat: Live,
    git: HashMap<PathBuf, Entity<GitView>>,
    /// The folder whose Git view is open in a modal over the conversation.
    git_modal: Option<PathBuf>,
    /// Each open project's git worktrees, kept fresh in the background.
    worktrees: HashMap<PathBuf, Entity<Worktrees>>,
    /// Each project's saved workspaces (selection, sessions, order), read on first use.
    workspaces: HashMap<PathBuf, kynex_core::workspace::Workspaces>,
    workspace_ui: workspace_new::WorkspaceUi,
    /// Each open project's colour settings, read once.
    appearances: HashMap<PathBuf, kynex_core::project::Appearance>,
    /// Each open project's Claude settings (mode, effort, folders), read once and
    /// kept in step with every save, so no render reads the file.
    claude_settings: HashMap<PathBuf, kynex_core::project::ClaudeSettings>,
    /// Hidden terminals with shells running, by folder, newest last: shown again before
    /// a new one starts.
    terminals: Vec<(PathBuf, Entity<TerminalPanel>)>,
    /// The panel in each terminal pane, of every workspace.
    term_panes: HashMap<kynex_core::layout::PaneId, Entity<TerminalPanel>>,
    /// Command panes from last time whose program is being looked up, so it starts once.
    command_starting: HashSet<kynex_core::layout::PaneId>,
    /// The custom commands of the project in front, for the palette.
    user_commands: Vec<kynex_core::user_commands::UserCommand>,
    /// Where each terminal pane's scrollback from last time is kept, to forget it with the pane.
    /// (workspace, id) by pane.
    term_histories: HashMap<kynex_core::layout::PaneId, (PathBuf, String)>,
    /// The agent CLIs agent panes can run, built-in and the user's.
    agent_rules: Vec<kynex_core::agent_panes::AgentRule>,
    /// What each agent pane's screen last said.
    agent_trackers: HashMap<kynex_core::layout::PaneId, kynex_core::agent_panes::Tracker>,
    /// Agent screens are being read every so often.
    agent_watch: bool,
    /// The agent rules were asked for.
    agent_rules_asked: bool,
    /// A custom command's program over the window, until it exits.
    command_popup: Option<Entity<crate::terminal::TerminalView>>,
    threads: HashMap<Key, Thread>,
    /// Each session's timeline turns, walked again only where items changed.
    timelines: HashMap<Key, kynex_core::timeline::Turns>,
    /// Tool calls and results opened up in the transcript.
    expanded: HashSet<(Key, usize)>,
    /// Runs of tool calls opened up, by their first call's index.
    open_runs: HashSet<(Key, usize)>,
    /// Calls auto mode refused that were allowed from their row since, with
    /// what was allowed ("Edge actions").
    unblocked: HashMap<(Key, usize), String>,
    /// Each project's sent messages and stash, loaded when first needed.
    histories: HashMap<PathBuf, kynex_core::history::History>,
    /// Each session's timeline turns opened or folded.
    folds: HashMap<Key, kynex_core::timeline::Fold>,
    /// A project opened for the first time, whose setup checklist is yet to open.
    setup_for: Option<PathBuf>,
    new_rule: Entity<TextInput>,
    /// The command typed in the settings' "Rules in effect" to try the rules on.
    rule_test: Entity<TextInput>,
    /// The project's own rule for what goes to its subagents (`delegate_view.rs`).
    delegate_rule: Entity<TextInput>,
    /// Unsent messages of the sessions not shown, by draft key; and the shown
    /// session's uid and draft key, whose message is in the composer.
    drafts: HashMap<String, Draft>,
    /// The focused pane: the message box and what goes with it (`src/pane/`). Every
    /// other pane's is parked until it is focused (`tiles.rs`).
    pane: SessionPane,
    pane_id: kynex_core::layout::PaneId,
    next_pane: kynex_core::layout::PaneId,
    parked: HashMap<kynex_core::layout::PaneId, SessionPane>,
    /// Each workspace's panes, by the workspace's folder.
    tiles: HashMap<PathBuf, kynex_core::tiling::Tiling>,
    /// The project each of `tiles` belongs to (where its layout is saved), by workspace folder.
    tile_projects: HashMap<PathBuf, PathBuf>,
    /// The pane tab being renamed, and the field its name is typed in.
    pane_tab_rename: Option<(usize, Entity<TextInput>)>,
    /// Resize mode (`prefix r`): h / j / k / l and the arrows grow the focused pane.
    resize_mode: Option<pane_keys::ResizePlace>,
    /// The row picked with the keys in the workspace / project picker menus.
    menu_pick: usize,
    /// The key just handled by resize mode, so the views under it don't take it too.
    resize_swallow: bool,
    /// Dividers held by the pointer, while the panes are being resized.
    tile_drag: Option<Vec<tiles::Resizing>>,
    /// Where the panes are drawn, as last laid out.
    tile_area: std::rc::Rc<std::cell::Cell<Option<gpui::Bounds<gpui::Pixels>>>>,
    palette: Option<PaletteView>,
    scratch: Option<ScratchModal>,
    /// When each session's files were last opened, for the composer's badge,
    /// and how many scratch files changed since; what that count was taken at.
    scratch_seen: HashMap<String, std::time::SystemTime>,
    scratch_new: HashMap<String, usize>,
    scratch_counted: Option<(String, bool)>,
    /// Settings → Storage's measurements, while that page has been opened.
    storage: Option<StorageView>,
    /// Actions last run from the palette, newest first.
    palette_recent: Vec<String>,
    sessions: Option<SessionList>,
    /// The sessions list being read in the background, for this.
    sessions_reading: Option<ListRead>,
    /// Sessions reads started, and the newest one shown: an older read finishing late isn't.
    sessions_reads: (u64, u64),
    /// The sessions panel's title filter, while it's open.
    session_filter: Entity<TextInput>,
    filtering_sessions: bool,
    /// Groups and the archive in the sessions panel.
    organize: Organize,
    /// The sessions list, for its own keys (↑ / ↓, Enter, F2, Delete).
    sessions_focus: FocusHandle,
    /// Session id whose delete button was clicked once and is awaiting confirmation.
    confirm_delete: Option<String>,
    /// Project tab Ctrl+W was pressed on while Claude worked in it, and when:
    /// a second press soon after closes it.
    confirm_close: Option<(usize, std::time::Instant)>,
    /// A project tab being dragged along the title bar, and where it's shown.
    tab_drag: Option<title_bar::TabDrag>,
    /// Each project tab's (left, right) as last drawn, in the order shown.
    tab_spans: std::rc::Rc<std::cell::RefCell<Vec<(f32, f32)>>>,
    /// The session being named right now, and every one tried this run.
    naming: Option<String>,
    naming_tried: HashSet<String>,
    /// The project settings being edited; the modal is open while this is set.
    settings: Option<SettingsView>,
    /// The app's own Settings modal, while open.
    app_settings: Option<app_settings::AppSettingsView>,
    /// The Extensions modal: MCP servers and plugins in the active folder.
    extensions: Option<extensions::ExtensionsView>,
    menu: Option<OpenMenu>,
    /// What's animating out after it closed: menus, modals, panels.
    exits: leaving::Exits,
    resizing_sessions: bool,
    /// Whether a newer Claude Code is out, or being installed.
    cli_update: UpdateState,
    /// Folders the CLI would ignore settings in until trusted; see `trust.rs`.
    trust: trust::Trust,
    /// Kynex's own update: found, downloading, installed or failed.
    app_update: UpdateState,
    /// When Kynex last asked GitHub for a newer version, and whether it's asking now.
    update_checked: Option<std::time::SystemTime>,
    update_checking: bool,
    /// The account's usage limits, as the CLI last reported them (this run or the last).
    usage: Option<kynex_core::usage::Limits>,
    /// The toasts in the corner, whether the pointer is on them, and whether
    /// their clock is paused (that, or the window is in the background) or running.
    toasts: kynex_core::toasts::Toasts,
    /// Ctrl+G: the message is open in the user's editor; the id of the toast saying so.
    editing_outside: Option<u64>,
    toast_hover: bool,
    toasts_paused: bool,
    toasts_ticking: bool,
    /// What Kynex and its processes use, as last read (`footer.rs`).
    resources: Option<kynex_core::resources::Usage>,
    /// The shown session's clocks (thinking, calls, pulse) are repainted once a second.
    clocks_ticking: bool,
    /// Mouse-wheel turns made smooth for every scroll area.
    wheel: SmoothWheel,
    /// An image opened over the window.
    viewer: Option<Entity<ImageViewer>>,
    /// Why each open session wants you, as of the last check (`attention.rs`).
    wants: HashMap<Key, kynex_core::attention::Why>,
    /// The icon's "needs you" dot is on.
    badged: bool,
    /// Idle sleep held off while sessions work (Keep awake), and whether taking it failed.
    awake: Option<kynex_core::awake::Awake>,
    awake_failed: bool,
    /// The control socket, if this is the Kynex serving it (`control.rs`).
    control: Option<kynex_core::control::server::Server>,
    /// Clients waiting for a session's turn to end, and the one being set up.
    control_waits: Vec<control::Wait>,
    control_wait_for: Option<control::Wait>,
    /// What each session was doing at the last look, for the event listeners.
    control_seen: control::Seen,
    /// Themes read from the data dir's `themes/` folder, the files that couldn't be used, and the watch on it.
    custom_themes: Vec<kynex_core::themes::Custom>,
    theme_errors: Vec<String>,
    _theme_watch: Option<kynex_core::watch::Watch>,
    /// The notification-area icon, while closing the window hides Kynex there (Windows).
    tray: Option<kynex_core::tray::Tray>,
    /// The window's native handle, for the taskbar dot.
    native_window: isize,
    /// Kynex is the app in front: no notifications then, and finished turns count as seen.
    window_active: bool,
    /// Thumbnails of screenshots still in session logs (drawn from `&self`, hence the cell).
    logged: std::cell::RefCell<viewer::LoggedImages>,
    /// The About Kynex modal is open; the installed Claude Code version, once read.
    about_open: bool,
    claude_version: Option<String>,
    cli_check: CliCheck,
    /// Whether the CLI is installed and signed in; until it is, a setup screen
    /// takes the workspace's place.
    setup: Setup,
    /// Where to paste the code the sign-in page shows.
    login_code: Entity<TextInput>,
    /// The OpenRouter key, once loaded from the keychain (`provider_setup.rs`):
    /// with it, new sessions can run on OpenRouter and a user signed out of
    /// Anthropic gets past the setup screen.
    openrouter_key: Option<String>,
    /// The OpenRouter key's credit for the footer, and when it was asked.
    openrouter_credit: Option<(kynex_core::openrouter::KeyInfo, std::time::Instant)>,
    openrouter_credit_asking: bool,
    /// The setup screen's OpenRouter key field, while its check runs, and why it failed.
    setup_key: Entity<TextInput>,
    setup_key_busy: bool,
    setup_key_error: Option<String>,
    focus: FocusHandle,
    wake: session::Wake,
}

impl KynexApp {
    pub fn new(saved: Saved, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let _ = std::fs::create_dir_all(chat_dir());

        // Session reader threads ring this; one task drains every session and repaints.
        let (tx, rx) = async_channel::bounded::<()>(1);
        let wake: session::Wake = Arc::new(move || {
            let _ = tx.try_send(());
        });
        cx.spawn(async move |this, cx| {
            while rx.recv().await.is_ok() {
                if this.update(cx, |this, cx| this.poll(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        // The control socket: requests ring this, and a task answers them with the window at hand.
        let (control_tx, control_rx) = async_channel::bounded::<()>(1);
        let control = match kynex_core::control::server::Server::start(Arc::new(move || {
            let _ = control_tx.try_send(());
        })) {
            Ok(server) => Some(server),
            Err(e) => {
                eprintln!("kynex: no control socket: {e}");
                None
            }
        };
        cx.spawn_in(window, async move |this, cx| {
            while control_rx.recv().await.is_ok() {
                if this.update_in(cx, |this, window, cx| this.serve_control(window, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        if let Some(folder) = OPEN_AT_START.get() {
            let folder = folder.to_string_lossy().into_owned();
            cx.spawn_in(window, async move |this, cx| {
                this.update_in(cx, |this, window, cx| {
                    let _ = this.open_project_at(&folder, window, cx);
                })
                .ok();
            })
            .detach();
        }
        let theme_watch = Self::watch_themes(cx);
        Self::watch_cli_updates(cx);
        Self::watch_app_updates(cx);
        Self::watch_usage(cx);
        Self::watch_resources(cx);
        Self::watch_sleep(cx);

        let mut chat = match saved.chat_session_id.clone() {
            Some(id) => Session::resume(Kind::Chat, chat_dir(), id, wake.clone()),
            None => Session::new(Kind::Chat, chat_dir()),
        };
        chat.model = ClaudeSettings::load(&chat_dir()).model;
        chat.wake = wake.clone();
        let chat = Live::of(chat);

        let composer = Self::new_composer(1, window, cx);
        let login_code = cx.new(|cx| TextInput::new(cx).mono().placeholder("Paste the code here"));
        cx.subscribe(&login_code, |this, _, ev, cx| {
            if let InputEvent::Submit = ev {
                this.submit_login_code(cx);
            }
        })
        .detach();
        cx.spawn(async move |this, cx| this.update(cx, |this, cx| this.check_setup(cx))).detach();
        cx.spawn(async move |this, cx| this.update(cx, |this, cx| this.load_openrouter(cx))).detach();
        cx.spawn(async move |this, cx| this.update(cx, |this, cx| this.fold_worktree_tabs(cx))).detach();
        let setup_key = cx.new(|cx| TextInput::new(cx).mono().masked().placeholder("Paste your OpenRouter key"));
        cx.subscribe(&setup_key, |this, _, ev, cx| {
            if let InputEvent::Submit = ev {
                this.setup_openrouter_key(cx);
            }
        })
        .detach();
        let group_name = cx.new(|cx| TextInput::new(cx).placeholder("Name"));
        cx.subscribe(&group_name, |this, _, ev, cx| {
            if let InputEvent::Submit = ev {
                this.commit_rename(cx);
            }
        })
        .detach();
        // Clicking away keeps the name too; Esc cancels before the focus leaves.
        let name_focus = group_name.read(cx).focus_handle(cx);
        cx.on_blur(&name_focus, window, |this, _, cx| this.commit_rename(cx)).detach();
        let session_filter = cx.new(|cx| TextInput::new(cx).placeholder("Filter sessions"));
        cx.subscribe(&session_filter, |_, _, ev, cx| {
            if let InputEvent::Changed = ev {
                cx.notify();
            }
        })
        .detach();
        let new_rule = cx.new(|cx| TextInput::new(cx).mono().placeholder("Bash(npm test:*)"));
        cx.subscribe(&new_rule, |this, _, ev, cx| {
            if let InputEvent::Submit = ev {
                this.add_rule(false, cx);
            }
        })
        .detach();
        let rule_test = cx.new(|cx| TextInput::new(cx).mono().placeholder("git add -A && git commit -m x && git push"));
        cx.subscribe(&rule_test, |_, _, ev, cx| {
            if let InputEvent::Changed = ev {
                cx.notify();
            }
        })
        .detach();
        let delegate_rule = cx
            .new(|cx| TextInput::new(cx).multiline().placeholder("Empty: the rule made from the agents, shown below"));
        cx.subscribe(&delegate_rule, |this, input, ev, cx| {
            if let InputEvent::Changed = ev {
                let rule = input.read(cx).text().trim().to_string();
                this.edit_settings(cx, |v| v.claude.delegate_rule = rule);
            }
        })
        .detach();

        cx.observe_window_bounds(window, |this, window, _| {
            let bounds = window.window_bounds();
            let b = bounds.get_bounds();
            this.saved.window = Some(WindowState {
                x: b.origin.x.into(),
                y: b.origin.y.into(),
                width: b.size.width.into(),
                height: b.size.height.into(),
                maximized: matches!(bounds, WindowBounds::Maximized(_)),
            });
        })
        .detach();
        // Coming back to Kynex: files may have changed while it was in the background.
        Self::watch_notification_clicks(window, cx);
        Self::watch_close(window, cx);
        cx.spawn(async move |this, cx| this.update(cx, |this, cx| this.sync_tray(cx))).detach();
        // The System theme follows the OS's light / dark setting as it changes.
        cx.observe_window_appearance(window, |this, _, cx| {
            if this.saved.theme == crate::theme::Theme::System && !this.custom_theme_applied() {
                crate::theme::set(this.saved.theme, cx);
                cx.refresh_windows();
            }
        })
        .detach();
        cx.observe_window_activation(window, |this, window, cx| {
            this.window_active = window.is_window_active();
            if window.is_window_active() {
                let folders: Vec<PathBuf> = this.worktrees.keys().cloned().collect();
                this.refresh_worktrees(&folders, cx);
            }
            // Toasts wait while you're away.
            cx.notify();
        })
        .detach();
        cx.on_app_quit(|this, cx| {
            this.stash_all_drafts(cx);
            this.save_scrollback(cx);
            this.save_layouts_now();
            this.save();
            async {}
        })
        .detach();

        Self::watch_resize_keys(cx);
        composer.read(cx).focus_handle(cx).focus(window, cx);
        if let Some(found) = saved.newer_than_us {
            let msg = format!(
                "state.json was saved by a newer Kynex (format v{found}); it was kept as state.json.bak and this session won't overwrite it."
            );
            cx.spawn(async move |this, cx| {
                this.update(cx, |this, cx| {
                    this.toast(kynex_core::toasts::Toast::error(msg));
                    cx.notify();
                })
            })
            .detach();
        }
        Self {
            saved,
            code: HashMap::new(),
            chat,
            git: HashMap::new(),
            git_modal: None,
            worktrees: HashMap::new(),
            workspaces: HashMap::new(),
            workspace_ui: Default::default(),
            terminals: Vec::new(),
            term_panes: HashMap::new(),
            command_starting: HashSet::new(),
            user_commands: Vec::new(),
            command_popup: None,
            agent_rules: Vec::new(),
            term_histories: HashMap::new(),
            agent_trackers: HashMap::new(),
            agent_watch: false,
            agent_rules_asked: false,
            appearances: HashMap::new(),
            claude_settings: HashMap::new(),
            threads: HashMap::new(),
            timelines: HashMap::new(),
            expanded: HashSet::new(),
            open_runs: HashSet::new(),
            unblocked: HashMap::new(),
            histories: HashMap::new(),
            folds: HashMap::new(),
            setup_for: None,
            new_rule,
            rule_test,
            delegate_rule,
            drafts: HashMap::new(),
            pane: SessionPane::new(composer),
            pane_id: 1,
            next_pane: 2,
            parked: HashMap::new(),
            tiles: HashMap::new(),
            tile_projects: HashMap::new(),
            pane_tab_rename: None,
            resize_mode: None,
            menu_pick: 0,
            resize_swallow: false,
            tile_drag: None,
            tile_area: Default::default(),
            palette: None,
            scratch: None,
            scratch_seen: HashMap::new(),
            scratch_new: HashMap::new(),
            scratch_counted: None,
            storage: None,
            palette_recent: Vec::new(),
            sessions: None,
            sessions_reading: None,
            sessions_reads: (0, 0),
            session_filter,
            filtering_sessions: false,
            organize: Organize::new(group_name),
            sessions_focus: cx.focus_handle(),
            confirm_delete: None,
            confirm_close: None,
            tab_drag: None,
            tab_spans: Default::default(),
            naming: None,
            naming_tried: HashSet::new(),
            settings: None,
            app_settings: None,
            extensions: None,
            menu: None,
            exits: Default::default(),
            resizing_sessions: false,
            cli_update: UpdateState::Idle,
            trust: Default::default(),
            app_update: UpdateState::Idle,
            update_checked: None,
            update_checking: false,
            usage: kynex_core::usage::Limits::load(),
            toasts: Default::default(),
            editing_outside: None,
            toast_hover: false,
            toasts_paused: false,
            toasts_ticking: false,
            resources: None,
            clocks_ticking: false,
            wheel: SmoothWheel::default(),
            viewer: None,
            wants: HashMap::new(),
            badged: false,
            awake: None,
            awake_failed: false,
            control,
            control_waits: Vec::new(),
            control_wait_for: None,
            control_seen: HashMap::new(),
            custom_themes: Vec::new(),
            theme_errors: Vec::new(),
            _theme_watch: theme_watch,
            tray: None,
            native_window: attention::native_handle(window),
            window_active: window.is_window_active(),
            logged: Default::default(),
            about_open: false,
            claude_version: None,
            cli_check: CliCheck::default(),
            setup: Setup::Checking,
            login_code,
            openrouter_key: None,
            openrouter_credit: None,
            openrouter_credit_asking: false,
            setup_key,
            setup_key_busy: false,
            setup_key_error: None,
            focus: cx.focus_handle(),
            wake,
        }
    }

    fn save(&self) {
        let _t = kynex_core::trace::span("save state.json");
        self.saved.write();
    }

    fn poll(&mut self, cx: &mut Context<Self>) {
        let _t = kynex_core::trace::span("poll (draining sessions)");
        // A session that starts wanting you (turn done, prompt up) while Kynex is
        // in the background flashes the taskbar / bounces the Dock icon.
        let mut called = false;
        let mut reports = Vec::new();
        let mut grok_lists = Vec::new();
        let mut without_tools = Vec::new();
        let mut limits: Option<kynex_core::usage::Limits> = None;
        // Folders whose files may have just changed: a turn ended or a tool call finished.
        let mut touched = HashSet::new();
        // Background tasks that just ended, for a toast if their session isn't shown.
        let mut ended = Vec::new();
        // The shown session finishing a turn scrolls to its end; others keep their place.
        let shown = self.focused_key();
        let mut shown_ended = false;
        // Sessions whose history came in from their log, and how many items went in front.
        let mut histories = Vec::new();
        let mut openrouter_turn_ended = false;
        for s in self.all_sessions_mut() {
            // Before `seen`: history isn't news (no toasts for old tasks).
            if let Some(n) = s.take_history() {
                histories.push((s.uid, n));
            }
            let before = s.wants_you();
            let (was_busy, seen) = (s.busy, s.items.len());
            s.poll();
            called |= !before && s.wants_you();
            let finished_tool = s
                .items
                .get(seen..)
                .is_some_and(|new| new.iter().any(|i| matches!(i, session::Item::ToolResult { .. })));
            if (was_busy && !s.busy) || finished_tool {
                touched.insert(s.cwd.clone());
            }
            shown_ended |= was_busy && !s.busy && Some(s.uid) == shown;
            openrouter_turn_ended |= was_busy && !s.busy && s.provider().provider().is_openrouter();
            for item in s.items.get(seen..).into_iter().flatten() {
                if let session::Item::Task(n) = item {
                    ended.push((s.uid, n.clone()));
                }
            }
            if let Some(init) = s.take_init() {
                reports.push((s.cwd.clone(), init, s.provider().provider()));
            }
            if let Some(list) = s.take_grok_models() {
                grok_lists.push(list);
            }
            if let Some(model) = s.take_no_tools() {
                without_tools.push((s.uid, model));
            }
            // The account's limits: whichever session reported last is newest.
            if let Some(newer) = s.take_limits() {
                limits = Some(match limits {
                    Some(older) => older.then(newer),
                    None => newer,
                });
            }
        }
        for (key, n) in histories {
            self.history_came(key, n);
        }
        // A turn spent some of it.
        if openrouter_turn_ended {
            self.refresh_openrouter_credit(cx);
        }
        if let Some(limits) = limits {
            self.limits_reported(limits, cx);
        }
        // What a session reported as it started: its folder's commands, the models.
        for (key, model) in without_tools {
            self.model_without_tools(key, &model);
        }
        for list in grok_lists {
            models::grok_reported(list, cx);
        }
        for (cwd, init, provider) in reports {
            self.commands_reported(cwd, init.commands, cx);
            // Claude's model list; an OpenRouter session's models are My models.
            if provider == kynex_core::provider::Provider::Anthropic {
                models::reported(init.models, cx);
            }
        }
        if let Some(key) = shown.filter(|_| shown_ended) {
            self.turn_ended(key);
        }
        self.refresh_worktrees(&touched, cx);
        self.toast_new_tasks(ended);
        if called && let Some(handle) = cx.windows().first().copied() {
            cx.defer(move |cx| {
                handle
                    .update(cx, |_, window, _| {
                        if !window.is_window_active() {
                            window.request_attention();
                        }
                    })
                    .ok();
            });
        }
        self.open_pending_setup(cx);
        self.restart_idle();
        self.sync_session_ids();
        self.save_layouts(cx);
        self.finish_control_waits(cx);
        self.control_events();
        self.sync_attention(cx);
        self.sync_awake();
        cx.notify();
    }

    /// Restarts the sessions running on settings they couldn't take live, once
    /// they're idle: they start again with the next message.
    pub(super) fn restart_idle(&mut self) {
        // A restart would also stop background tasks; wait for those too.
        for s in self.all_sessions_mut().filter(|s| s.stale && !s.busy && !s.has_tasks()) {
            s.kill();
        }
    }
}

impl Focusable for KynexApp {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for KynexApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _t = kynex_core::trace::span("render (building the frame)");
        // The UI zoom: everything sized in rems scales with it.
        window.set_rem_size(gpui::px(16.) * self.saved.zoom);
        let t = theme(cx).clone();
        let view = self.sync_before_render(window, cx);

        // Toasts sit in the corner picked in Settings, the same in every view;
        // the conversation's own bars and composer are measured to stay clear of.
        let toasts = self.render_toasts(window, cx);
        let in_conversation = self.workspace_ready() && view != Some(View::Git);
        if !in_conversation {
            self.pane.toast_room.set(None);
        }
        let main = if let Some(setup) = self.render_setup(cx) {
            setup
        } else if view == Some(View::Git) {
            self.render_git_main(window, cx)
        } else {
            self.render_code_main(window, cx)
        };

        let key = self.focused_key();
        let root = div()
            .id("workspace")
            .key_context("Workspace")
            .track_focus(&self.focus)
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .bg(t.editor)
            .font_family(ui_font())
            .text_color(t.text)
            .text_sm()
            .capture_key_down(cx.listener(Self::swallow_resize_key))
            .on_key_down(cx.listener(Self::key_down))
            .on_drop(cx.listener(Self::on_drop))
            // A dragged project tab follows the pointer anywhere in the window,
            // and lands where it's shown wherever it's let go.
            .on_drag_move(cx.listener(Self::drag_tab))
            .on_drop(cx.listener(Self::drop_tab))
            .child(self.wheel.listener())
            .child(self.render_title_bar(key, window, cx))
            .child(main)
            .when(self.saved.footer, |d| d.child(self.render_footer(key, cx)));
        let root = self.with_actions(self.with_resize_drag(root, cx), cx);
        self.with_overlays(root, toasts, window, cx)
    }
}
