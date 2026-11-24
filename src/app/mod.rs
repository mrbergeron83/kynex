//! `KynexApp`, the root view. Pure-CLI architecture: every workspace tiles
//! native interactive `agent_cli` panes (`agent_cli.rs`), Sessions & Workspaces
//! (`Tree`), Agents, Git, and plain Terminal panes.

mod about;
mod agent_cli;
mod agent_picker;
mod agents_pane;
mod antigravity_setup;
mod app_settings;
mod app_update;
mod appearance;
mod attention;
mod chat_settings;
mod claude_chat;
mod control;
mod control_pane_layout;
mod control_panes;
mod control_sessions;
mod control_workspaces;
mod custom_commands;
mod custom_themes;
mod footer;
mod git_file_modal;
mod git_pane;
mod goto;
mod grok_setup;
pub(crate) mod keys;
mod layout;
mod layout_history;
mod layouts;
mod layouts_named;
mod leaving;
mod menus;
mod palette;
mod palette_filters;
mod palette_more;
mod palette_sessions;
mod pane_actions;
mod pane_keys;
mod pane_move;
mod projects;
mod provider_setup;
mod providers;
mod removed_workspaces;
mod session_groups;
mod session_rows;
mod session_select;
mod session_tidy;
mod session_tree;
mod sessions_panel;
mod setup;
mod state;
mod status;
mod storage;
mod storage_list;
mod terminal_panes;
mod tiles;
mod title_bar;
mod toasts;
mod tool_panes;
mod toolbar;
mod usage_meter;
mod workspace_binding;
mod workspace_list;
mod workspace_new;
mod workspaces;
pub(crate) mod worktrees;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use gpui::{App, Context, Entity, FocusHandle, Focusable, Window, WindowBounds, div, prelude::*};
use kynex_core::cli::UpdateState;
use kynex_core::layout::PaneId;

pub use control::OPEN_AT_START;
pub use custom_commands::bind_command_keys;
pub use keys::{CopyMode, SHELL_KEYS, SendPrefix, ToggleGit, ToggleTerminal, bind_keys, prefix_key};
use menus::OpenMenu;
use palette::PaletteView;
use session_groups::Organize;
use sessions_panel::{Reads, SessionList};
use setup::CliUpdates;
pub use state::Saved;
use state::WindowState;
use storage::StorageView;
use worktrees::Worktrees;

use crate::terminal::{TerminalPanel, TerminalView};
use crate::theme::{theme, ui_font};
use crate::ui::input::{InputEvent, TextInput};
use crate::ui::smooth_scroll::SmoothWheel;

pub struct KynexApp {
    saved: Saved,
    /// A changed file's whole diff or commit detail, in a modal over the window.
    git_file: Option<git_file_modal::GitDiffModal>,
    /// The Git pane of each workspace folder.
    git_panes: HashMap<PathBuf, Entity<crate::git_pane::GitPane>>,
    /// A workspace folder whose Git pane opens once its panes are up.
    want_git_pane: Option<(PathBuf, std::time::Instant)>,
    /// Each open project's git worktrees, kept fresh in the background.
    worktrees: HashMap<PathBuf, Entity<Worktrees>>,
    /// Each project's saved workspaces (selection, sessions, order, layouts), read on first use.
    workspaces: HashMap<PathBuf, kynex_core::workspace::Workspaces>,
    workspace_ui: workspace_new::WorkspaceUi,
    /// Which drifted workspaces were warned about (`workspace_binding.rs`).
    binding: workspace_binding::Binding,
    /// The "Removed workspaces" section: what is unfolded, and what a right-click asked.
    removed_ui: removed_workspaces::RemovedUi,
    /// Each open project's colour settings, read once.
    appearances: HashMap<PathBuf, kynex_core::project::Appearance>,
    /// Hidden plain terminals with shells running, by folder, newest last.
    terminals: Vec<(PathBuf, Entity<TerminalPanel>)>,
    /// The panel in each plain terminal pane, of every workspace.
    term_panes: HashMap<PaneId, Entity<TerminalPanel>>,
    /// Command/shell panes whose startup task is in flight.
    command_starting: HashSet<PaneId>,
    /// The custom commands of the project in front, for the palette.
    user_commands: Vec<kynex_core::user_commands::UserCommand>,
    /// Where each plain terminal pane's scrollback from last time is kept.
    term_histories: HashMap<PaneId, (PathBuf, String)>,
    /// A custom command's program over the window, until it exits.
    command_popup: Option<Entity<TerminalView>>,
    /// Built-in + user agent screen rules (`kynex_core::agent_panes`).
    agent_rules: Vec<kynex_core::agent_panes::AgentRule>,
    agent_rules_asked: bool,
    /// Cached provider catalog (computed off the UI thread for the picker & palette).
    provider_catalog: Vec<kynex_core::agent_cli::ProviderEntry>,
    /// All live `agent_cli` instances across open workspaces (visible in panes OR parked in background).
    agent_instances: HashMap<agent_cli::AgentId, agent_cli::LiveAgent>,
    /// Which [`agent_cli::LiveAgent`] is currently mounted in each non-pending `agent_cli` pane.
    pane_agents: HashMap<PaneId, agent_cli::AgentId>,
    /// All live native Claude Code chat instances across open workspaces (visible OR parked).
    claude_chats: HashMap<agent_cli::AgentId, claude_chat::LiveClaudeChat>,
    /// Which [`claude_chat::LiveClaudeChat`] is currently mounted in each `ClaudeChat` pane.
    pane_claude: HashMap<PaneId, agent_cli::AgentId>,
    /// Inline provider picker state for pending `agent_cli` panes.
    agent_pickers: HashMap<PaneId, agent_picker::AgentPickerState>,
    /// Whether the 500ms agent screen/hook watcher loop is running.
    agent_watch: bool,
    /// Provider id currently being installed from the inline picker (`claude`, `agy`, `grok`).
    agent_installing: Option<String>,
    /// Focus handle for the active inline provider picker.
    picker_focus: FocusHandle,
    next_pane: PaneId,
    /// Each workspace's panes, by the workspace's folder.
    tiles: HashMap<PathBuf, kynex_core::tiling::Tiling>,
    /// The project each of `tiles` belongs to (where its layout is saved), by workspace folder.
    tile_projects: HashMap<PathBuf, PathBuf>,
    /// The workspace whose panes were on screen last: the one you go to takes its shape.
    tiled_last: Option<PathBuf>,
    /// The name being typed for "Save layout as…", in the layouts menu.
    layout_name: Option<Entity<TextInput>>,
    /// Sticky keyboard mode (`prefix r` resize, `prefix n` navigate), until Esc.
    pane_mode: Option<pane_keys::PaneMode>,
    /// The row picked with the keys in the workspace / project picker menus.
    menu_pick: usize,
    /// The key just handled by resize mode, so the views under it don't take it too.
    resize_swallow: bool,
    /// Dividers held by the pointer, while the panes are being resized.
    tile_drag: Option<Vec<tiles::Resizing>>,
    /// Where the panes are drawn, as last laid out.
    tile_area: std::rc::Rc<std::cell::Cell<Option<gpui::Bounds<gpui::Pixels>>>>,
    palette: Option<PaletteView>,
    /// Settings → Storage's measurements, while that page has been opened.
    storage: Option<StorageView>,
    /// Actions last run from the palette, newest first.
    palette_recent: Vec<String>,
    /// Each workspace's past sessions, as last read.
    sessions: HashMap<PathBuf, SessionList>,
    /// The lists being read in the background, and which read is newest.
    session_reads: Reads,
    /// The sessions panel's title filter, while it's open.
    session_filter: Entity<TextInput>,
    filtering_sessions: bool,
    /// Groups and the archive in the sessions panel.
    organize: Organize,
    /// The sessions list, for its own keys (↑ / ↓, Enter, F2, Delete).
    sessions_focus: FocusHandle,
    /// Session id whose delete button was clicked once and is awaiting confirmation.
    confirm_delete: Option<String>,
    /// Project tab Ctrl+W was pressed on while an agent worked in it, and when.
    confirm_close: Option<(usize, std::time::Instant)>,
    /// A project tab being dragged along the title bar, and where it's shown.
    tab_drag: Option<title_bar::TabDrag>,
    /// A pane dragged over another: that pane, and the side it would land on (`None`: swap).
    pane_drop: Option<(PaneId, Option<kynex_core::layout::Direction>)>,
    /// Each project tab's (left, right) as last drawn, in the order shown.
    tab_spans: std::rc::Rc<std::cell::RefCell<Vec<(f32, f32)>>>,
    /// The app's Settings modal, while open.
    app_settings: Option<app_settings::AppSettingsView>,
    menu: Option<OpenMenu>,
    /// What's animating out after it closed: menus, modals.
    exits: leaving::Exits,
    /// Version and update monitoring for installed CLIs (`claude`, `grok`, `agy`).
    cli_updates: CliUpdates,
    /// Kynex's own update: found, downloading, installed or failed.
    app_update: UpdateState,
    /// When Kynex last asked GitHub for a newer version, and whether it's asking now.
    update_checked: Option<std::time::SystemTime>,
    update_checking: bool,
    /// The toasts in the corner.
    toasts: kynex_core::toasts::Toasts,
    toast_hover: bool,
    toasts_paused: bool,
    toasts_ticking: bool,
    /// What Kynex and its processes use, as last read (`footer.rs`).
    resources: Option<kynex_core::resources::Usage>,
    /// Wakes the resources sampler when the footer is shown or the window regains focus.
    resources_wake: async_channel::Sender<()>,
    /// Mouse-wheel turns made smooth for every scroll area.
    wheel: SmoothWheel,
    /// Which `agent_cli` panes currently want attention (`attention.rs`).
    wants: HashMap<PaneId, kynex_core::attention::Alert>,
    /// Idle sleep held off while agents work (Keep awake), and whether taking it failed.
    awake: Option<kynex_core::awake::Awake>,
    awake_failed: bool,
    /// The control socket, if this is the Kynex serving it (`control.rs`).
    control: Option<kynex_core::control::server::Server>,
    /// Clients waiting for a command or agent pane turn to end.
    control_waits: Vec<control::Wait>,
    control_wait_for: Option<control::Wait>,
    /// The last marker a `knx pane run --wait` command was typed with.
    control_marker: u64,
    /// The loop looking at waits on panes is running.
    pane_wait_watch: bool,
    /// Themes read from the data dir's `themes/` folder, errors, and watcher.
    custom_themes: Vec<kynex_core::themes::Custom>,
    theme_errors: Vec<String>,
    _theme_watch: Option<kynex_core::watch::Watch>,
    /// Kynex is the app in front: no background OS notifications then.
    window_active: bool,
    /// The About Kynex modal is open.
    about_open: bool,
    /// The OpenRouter key, once loaded from the OS keychain (`provider_setup.rs`).
    openrouter_key: Option<String>,
    /// The OpenRouter key's credit for the footer, and when it was asked.
    openrouter_credit: Option<(kynex_core::openrouter::KeyInfo, std::time::Instant)>,
    openrouter_credit_asking: bool,
    /// The account's usage limits, as the CLI last reported them, this run or the last (`usage_meter.rs`).
    usage: Option<kynex_core::claude::usage::Limits>,
    focus: FocusHandle,
}

impl KynexApp {
    pub fn new(saved: Saved, window: &mut Window, cx: &mut Context<Self>) -> Self {
        kynex_core::project::set_bypass_allowed(saved.allow_bypass);
        // The control socket: requests (including `knx hook`) ring this.
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
        let resources_wake = Self::watch_resources(cx);
        Self::watch_usage(cx);
        cx.spawn(async move |this, cx| this.update(cx, |this, cx| this.probe_clis(cx))).detach();
        cx.spawn(async move |this, cx| this.update(cx, |this, cx| this.load_openrouter(cx))).detach();
        cx.spawn(async move |this, cx| this.update(cx, |this, cx| this.fold_worktree_tabs(cx))).detach();

        let group_name = cx.new(|cx| TextInput::new(cx).placeholder("Name"));
        cx.subscribe(&group_name, |this, _, ev, cx| {
            if let InputEvent::Submit = ev {
                this.commit_rename(cx);
            }
        })
        .detach();
        let name_focus = group_name.read(cx).focus_handle(cx);
        cx.on_blur(&name_focus, window, |this, _, cx| this.commit_rename(cx)).detach();

        let session_filter = cx.new(|cx| TextInput::new(cx).placeholder("Filter sessions"));
        cx.subscribe(&session_filter, |_, _, ev, cx| {
            if let InputEvent::Changed = ev {
                cx.notify();
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
        Self::watch_notification_clicks(window, cx);
        cx.observe_window_appearance(window, |this, _, cx| {
            if this.saved.theme == crate::theme::Theme::System && !this.custom_theme_applied() {
                crate::theme::set(this.saved.theme, cx);
                cx.refresh_windows();
            }
        })
        .detach();
        crate::ui::motion::set_window_active(window.is_window_active());
        cx.observe_window_activation(window, |this, window, cx| {
            this.window_active = window.is_window_active();
            crate::ui::motion::set_window_active(this.window_active);
            if window.is_window_active() {
                if this.saved.footer {
                    let _ = this.resources_wake.try_send(());
                }
                let folders: Vec<PathBuf> = this.worktrees.keys().cloned().collect();
                this.refresh_worktrees(&folders, cx);
            }
            cx.notify();
        })
        .detach();
        cx.on_app_quit(|this, cx| {
            this.save_scrollback(cx);
            this.save_layouts_now();
            this.save();
            async {}
        })
        .detach();

        Self::watch_pane_mode_keys(cx);
        let focus = cx.focus_handle();
        focus.focus(window, cx);
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
            git_file: None,
            git_panes: HashMap::new(),
            want_git_pane: None,
            worktrees: HashMap::new(),
            workspaces: HashMap::new(),
            workspace_ui: Default::default(),
            binding: Default::default(),
            removed_ui: Default::default(),
            appearances: HashMap::new(),
            terminals: Vec::new(),
            term_panes: HashMap::new(),
            command_starting: HashSet::new(),
            user_commands: Vec::new(),
            term_histories: HashMap::new(),
            command_popup: None,
            agent_rules: Vec::new(),
            agent_rules_asked: false,
            provider_catalog: Vec::new(),
            agent_instances: HashMap::new(),
            pane_agents: HashMap::new(),
            claude_chats: HashMap::new(),
            pane_claude: HashMap::new(),
            agent_pickers: HashMap::new(),
            agent_watch: false,
            agent_installing: None,
            picker_focus: cx.focus_handle(),
            next_pane: 1,
            tiles: HashMap::new(),
            tile_projects: HashMap::new(),
            tiled_last: None,
            layout_name: None,
            pane_mode: None,
            menu_pick: 0,
            resize_swallow: false,
            tile_drag: None,
            tile_area: Default::default(),
            palette: None,
            storage: None,
            palette_recent: Vec::new(),
            sessions: HashMap::new(),
            session_reads: Reads::default(),
            session_filter,
            filtering_sessions: false,
            organize: Organize::new(group_name),
            sessions_focus: cx.focus_handle(),
            confirm_delete: None,
            confirm_close: None,
            tab_drag: None,
            pane_drop: None,
            tab_spans: Default::default(),
            app_settings: None,
            menu: None,
            exits: Default::default(),
            cli_updates: CliUpdates::default(),
            app_update: UpdateState::Idle,
            update_checked: None,
            update_checking: false,
            toasts: Default::default(),
            toast_hover: false,
            toasts_paused: false,
            toasts_ticking: false,
            resources: None,
            resources_wake,
            wheel: SmoothWheel::default(),
            wants: HashMap::new(),
            awake: None,
            awake_failed: false,
            control,
            control_waits: Vec::new(),
            control_wait_for: None,
            control_marker: 0,
            pane_wait_watch: false,
            custom_themes: Vec::new(),
            theme_errors: Vec::new(),
            _theme_watch: theme_watch,
            window_active: window.is_window_active(),
            about_open: false,
            openrouter_key: None,
            openrouter_credit: None,
            openrouter_credit_asking: false,
            usage: kynex_core::claude::usage::Limits::load(),
            focus,
        }
    }

    fn save(&self) {
        let _t = kynex_core::trace::span("save state.json");
        self.saved.write();
    }

    /// Holds off idle sleep while any agent is working (when Keep awake is enabled).
    pub(super) fn sync_awake(&mut self) {
        let busy = self.any_agent_working();
        let want = self.saved.keep_awake.wanted(busy);
        if want && self.awake.is_none() && !self.awake_failed {
            match kynex_core::awake::Awake::hold("Kynex agent working") {
                Ok(a) => self.awake = Some(a),
                Err(e) => {
                    self.awake_failed = true;
                    eprintln!("kynex: keep awake: {e}");
                }
            }
        } else if !want {
            self.awake = None;
            self.awake_failed = false;
        }
    }

    pub(super) fn set_keep_awake(&mut self, k: kynex_core::awake::KeepAwake, cx: &mut Context<Self>) {
        self.saved.keep_awake = k;
        self.awake_failed = false;
        self.sync_awake();
        self.save();
        cx.notify();
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
        window.set_rem_size(gpui::px(16.) * self.saved.zoom);
        let t = theme(cx).clone();
        self.sync_before_render(window, cx);

        let toasts = self.render_toasts(window, cx);
        let main = self.render_code_main(window, cx);

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
            .on_drag_move(cx.listener(Self::drag_tab))
            .on_drop(cx.listener(Self::drop_tab))
            .child(self.wheel.listener())
            .child(self.render_title_bar(window, cx))
            .child(main)
            .when(self.saved.footer, |d| d.child(self.render_footer(cx)));
        let root = self.with_actions(self.with_resize_drag(root, cx), cx);
        self.with_overlays(root, toasts, window, cx)
    }
}
