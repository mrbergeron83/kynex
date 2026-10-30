//! `KynexApp`, the root view. This file holds its state, startup and the
//! top-level layout; each part of the window is an `impl KynexApp` block in
//! its own file:
//!
//! | file                | what                                                 |
//! |---------------------|------------------------------------------------------|
//! | `state.rs`          | `Saved`: what persists to `state.json`               |
//! | `keys.rs`           | actions, `keymap.json`, tab / Esc / mode shortcuts   |
//! | `workspace.rs`      | tabs, projects, and which session is shown           |
//! | `title_bar.rs`      | title bar and project tabs                           |
//! | `footer.rs`         | status footer: Kynex's CPU / memory, meters, update  |
//! | `status.rs`         | the footer's usage-limits meter and update pill      |
//! | `toolbar.rs`        | Code / Git switch, path, cost, open-in buttons       |
//! | `sessions_panel.rs` | open and past sessions                               |
//! | `session_groups.rs` | session groups and the archive, their menus          |
//! | `session_header.rs` | the session's title, branch / worktree, turns, cost  |
//! | `terminal_dock.rs`  | the project's terminal panel under the conversation  |
//! | `tasks.rs`          | background tasks: running strip, ended rows          |
//! | `toasts.rs`         | the toast stack: task endings, updates, errors       |
//! | `titles.rs`         | naming sessions Claude Code didn't name              |
//! | `transcript.rs`     | the conversation list and how each item draws        |
//! | `tool_rows.rs`      | tool calls with their results, runs of calls folded  |
//! | `worktrees.rs`      | the project's worktrees, and which one Claude is in  |
//! | `git_modal.rs`      | a worktree's Git view in a modal, from the header    |
//! | `composer.rs`       | message box, attachments, pickers, send / stop       |
//! | `drafts.rs`         | each session's unsent message, kept across switches  |
//! | `queue.rs`          | messages queued while Claude works, Send now         |
//! | `handoff.rs`        | "waiting for you in the browser" banner, Continue    |
//! | `attention.rs`      | OS notifications and the icon's "needs you" dot     |
//! | `background.rs`     | close to tray / Dock, keep awake, `claude` started early |
//! | `history.rs`        | ↑ / ↓ history, Ctrl+R search, stash, Esc Esc, editor |
//! | `rewind.rs`         | rewind to an earlier message: conversation and files |
//! | `timeline.rs`       | the session's turns and steps beside the transcript  |
//! | `slash.rs`          | the slash-command popup, and built-ins Kynex runs   |
//! | `prompts.rs`        | permission / plan / question prompts                 |
//! | `menus.rs`          | popup menus                                          |
//! | `leaving.rs`        | menus, modals and panels animating out as they close |
//! | `settings.rs`       | the project's Claude settings modal                  |
//! | `app_settings.rs`   | the app's Settings modal: CLI, shell, appearance     |
//! | `setup.rs`          | CLI install / sign-in screen and update strip        |
//! | `app_update.rs`     | Kynex updating itself: checks, the update toast      |
//! | `about.rs`          | the About Kynex modal: version, paths, updates       |
//! | `extensions.rs`     | the Extensions modal: MCP servers, plugins           |
//! | `extensions_add.rs` | its Add server panel: browser preset, your own       |
//! | `appearance.rs`     | each project's colour: title bar, tab, composer      |
//! | `models.rs`         | the model list                                       |
//! | `viewer.rs`         | images opened over the window, thumbnails            |

mod about;
mod app_settings;
mod app_update;
mod appearance;
mod attention;
mod background;
mod composer;
mod drafts;
mod extensions;
mod extensions_add;
mod footer;
mod git_modal;
mod handoff;
mod history;
mod keys;
mod leaving;
mod menus;
mod models;
mod prompts;
mod queue;
mod rewind;
mod session_groups;
mod session_header;
mod sessions_panel;
mod settings;
mod setup;
mod slash;
mod state;
mod status;
mod tasks;
mod terminal_dock;
mod timeline;
mod title_bar;
mod titles;
mod toasts;
mod tool_rows;
mod toolbar;
mod transcript;
mod viewer;
mod workspace;
mod worktrees;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use gpui::{
    App, Context, Entity, FocusHandle, Focusable, MouseButton, MouseMoveEvent, Window, WindowBounds, div, prelude::*,
};
use kynex_core::cli::UpdateState;
use kynex_core::live::{Key, Live};
use kynex_core::paths::chat_dir;
use kynex_core::session::{self, Attachment, Kind, Session};

use drafts::Draft;
use keys::{CopySelection, copy_selection};
pub use keys::{SHELL_KEYS, ToggleTerminal, bind_keys};
use menus::OpenMenu;
use prompts::ActivePrompt;
use session_groups::Organize;
use sessions_panel::{ListRead, SessionList};
use settings::SettingsView;
use setup::{CliCheck, Setup};
use slash::Slash;
pub use state::Saved;
use state::{Active, View, WindowState};
use transcript::{Scroll, Thread};
use worktrees::Worktrees;

use crate::git::GitView;
use crate::terminal::TerminalPanel;
use crate::theme::{theme, ui_font};
use crate::ui::image_viewer::ImageViewer;
use crate::ui::input::{InputEvent, TextInput};
use crate::ui::motion::{self, fade_in};
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
    /// Each open project's colour settings, read once.
    appearances: HashMap<PathBuf, kynex_core::project::Appearance>,
    /// Each open project's Claude settings (mode, effort, folders), read once and
    /// kept in step with every save, so no render reads the file.
    claude_settings: HashMap<PathBuf, kynex_core::project::ClaudeSettings>,
    /// Each project's terminal panel, created the first time it's opened.
    terminals: HashMap<PathBuf, Entity<TerminalPanel>>,
    threads: HashMap<Key, Thread>,
    /// Each session's timeline turns, walked again only where items changed.
    timelines: HashMap<Key, kynex_core::timeline::Turns>,
    /// Tool calls and results opened up in the transcript.
    expanded: HashSet<(Key, usize)>,
    /// Runs of tool calls opened up, by their first call's index.
    open_runs: HashSet<(Key, usize)>,
    /// Ctrl+O: every tool call in the transcript shown open.
    expand_tools: bool,
    /// Each project's sent messages and stash, loaded when first needed.
    histories: HashMap<PathBuf, kynex_core::history::History>,
    history_search: Option<history::HistorySearch>,
    /// The transcript row lit up after a jump from the timeline.
    flash: Option<timeline::Flash>,
    /// Each session's timeline turns opened or folded.
    folds: HashMap<Key, kynex_core::timeline::Fold>,
    /// The rewind popup (Esc Esc on an empty composer), while open.
    rewind: Option<rewind::RewindMenu>,
    /// When Esc last went to a draft, for Esc Esc.
    last_esc: Option<std::time::Instant>,
    composer: Entity<TextInput>,
    new_rule: Entity<TextInput>,
    attachments: Vec<Attachment>,
    /// Unsent messages of the sessions not shown, by draft key; and the shown
    /// session's uid and draft key, whose message is in the composer.
    drafts: HashMap<String, Draft>,
    draft_for: Option<(Key, String)>,
    slash: Slash,
    sessions: Option<SessionList>,
    /// The sessions list being read in the background, for this.
    sessions_reading: Option<ListRead>,
    /// The sessions panel's title filter, while it's open.
    session_filter: Entity<TextInput>,
    filtering_sessions: bool,
    /// Groups and the archive in the sessions panel.
    organize: Organize,
    /// Session id whose delete button was clicked once and is awaiting confirmation.
    confirm_delete: Option<String>,
    /// Project tab Ctrl+W was pressed on while Claude worked in it, and when:
    /// a second press soon after closes it.
    confirm_close: Option<(usize, std::time::Instant)>,
    /// The session being named right now, and every one tried this run.
    naming: Option<String>,
    naming_tried: HashSet<String>,
    /// The project settings being edited; the modal is open while this is set.
    settings: Option<SettingsView>,
    /// The app's own Settings modal, while open.
    app_settings: Option<app_settings::AppSettingsView>,
    /// The Extensions modal: MCP servers and plugins in the active folder.
    extensions: Option<extensions::ExtensionsView>,
    /// Projects whose rules changed mid-reply; their process restarts once it's idle.
    restart_when_idle: HashSet<PathBuf>,
    prompt: Option<ActivePrompt>,
    menu: Option<OpenMenu>,
    /// What's animating out after it closed: menus, modals, panels.
    exits: leaving::Exits,
    resizing_sessions: bool,
    resizing_terminal: bool,
    /// Whether a newer Claude Code is out, or being installed.
    cli_update: UpdateState,
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
    toast_hover: bool,
    toasts_paused: bool,
    toasts_ticking: bool,
    /// The conversation's free area last frame (between its header and the
    /// composer), which the toasts stay clear of; `None` in other views.
    toast_room: std::rc::Rc<std::cell::Cell<Option<gpui::Bounds<gpui::Pixels>>>>,
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
        Self::watch_cli_updates(cx);
        Self::watch_app_updates(cx);
        Self::watch_usage(cx);
        Self::watch_resources(cx);

        let mut chat = match saved.chat_session_id.clone() {
            Some(id) => Session::resume(Kind::Chat, chat_dir(), id, wake.clone()),
            None => Session::new(Kind::Chat, chat_dir()),
        };
        chat.model = saved.model.clone();
        chat.wake = wake.clone();
        let chat = Live::of(chat);

        let composer = cx.new(|cx| TextInput::new(cx).multiline());
        cx.subscribe_in(&composer, window, |this, _, ev, window, cx| match ev {
            InputEvent::Submit => this.send(window, cx),
            InputEvent::PasteImages(images) => {
                for i in images {
                    this.attach_bytes(&i.bytes, cx);
                }
            }
            InputEvent::Changed => this.warm_shown(),
        })
        .detach();
        // Focusing the message box means a message is coming: start Claude now.
        let composer_focus = composer.read(cx).focus_handle(cx);
        cx.on_focus(&composer_focus, window, |this, _, _| this.warm_shown()).detach();
        let login_code = cx.new(|cx| TextInput::new(cx).mono().placeholder("Paste the code here"));
        cx.subscribe(&login_code, |this, _, ev, cx| {
            if let InputEvent::Submit = ev {
                this.submit_login_code(cx);
            }
        })
        .detach();
        cx.spawn(async move |this, cx| this.update(cx, |this, cx| this.check_setup(cx))).detach();
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
            if this.saved.theme == crate::theme::Theme::System {
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
            this.stash_draft(cx);
            this.save();
            async {}
        })
        .detach();

        composer.read(cx).focus_handle(cx).focus(window, cx);
        Self {
            saved,
            code: HashMap::new(),
            chat,
            git: HashMap::new(),
            git_modal: None,
            worktrees: HashMap::new(),
            terminals: HashMap::new(),
            appearances: HashMap::new(),
            claude_settings: HashMap::new(),
            threads: HashMap::new(),
            timelines: HashMap::new(),
            expanded: HashSet::new(),
            open_runs: HashSet::new(),
            expand_tools: false,
            histories: HashMap::new(),
            history_search: None,
            flash: None,
            folds: HashMap::new(),
            rewind: None,
            last_esc: None,
            composer,
            new_rule,
            attachments: Vec::new(),
            drafts: HashMap::new(),
            draft_for: None,
            slash: Slash::default(),
            sessions: None,
            sessions_reading: None,
            session_filter,
            filtering_sessions: false,
            organize: Organize::new(group_name),
            confirm_delete: None,
            confirm_close: None,
            naming: None,
            naming_tried: HashSet::new(),
            settings: None,
            app_settings: None,
            extensions: None,
            restart_when_idle: HashSet::new(),
            prompt: None,
            menu: None,
            exits: Default::default(),
            resizing_sessions: false,
            resizing_terminal: false,
            cli_update: UpdateState::Idle,
            app_update: UpdateState::Idle,
            update_checked: None,
            update_checking: false,
            usage: kynex_core::usage::Limits::load(),
            toasts: Default::default(),
            toast_hover: false,
            toasts_paused: false,
            toasts_ticking: false,
            toast_room: Default::default(),
            resources: None,
            clocks_ticking: false,
            wheel: SmoothWheel::default(),
            viewer: None,
            wants: HashMap::new(),
            badged: false,
            awake: None,
            awake_failed: false,
            tray: None,
            native_window: attention::native_handle(window),
            window_active: window.is_window_active(),
            logged: Default::default(),
            about_open: false,
            claude_version: None,
            cli_check: CliCheck::default(),
            setup: Setup::Checking,
            login_code,
            focus: cx.focus_handle(),
            wake,
        }
    }

    fn save(&self) {
        self.saved.write();
    }

    fn poll(&mut self, cx: &mut Context<Self>) {
        // A session that starts wanting you (turn done, prompt up) while Kynex is
        // in the background flashes the taskbar / bounces the Dock icon.
        let mut called = false;
        let mut reports = Vec::new();
        let mut limits: Option<kynex_core::usage::Limits> = None;
        // Folders whose files may have just changed: a turn ended or a tool call finished.
        let mut touched = HashSet::new();
        // Background tasks that just ended, for a toast if their session isn't shown.
        let mut ended = Vec::new();
        // The shown session finishing a turn scrolls to its end; others keep their place.
        let shown = self.active_key();
        let mut shown_ended = false;
        // Sessions whose history came in from their log, and how many items went in front.
        let mut histories = Vec::new();
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
            for item in s.items.get(seen..).into_iter().flatten() {
                if let session::Item::Task(n) = item {
                    ended.push((s.uid, n.clone()));
                }
            }
            if let Some(init) = s.take_init() {
                reports.push((s.cwd.clone(), init));
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
        if let Some(limits) = limits {
            self.limits_reported(limits, cx);
        }
        // What a session reported as it started: its folder's commands, the models.
        for (cwd, init) in reports {
            self.commands_reported(cwd, init.commands, cx);
            models::reported(init.models, cx);
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
        self.restart_idle();
        self.sync_session_ids();
        self.sync_attention(cx);
        self.sync_awake();
        cx.notify();
    }

    /// Restart the projects whose settings changed, once they're idle.
    fn restart_idle(&mut self) {
        self.restart_when_idle.retain(|path| {
            let Some(live) = self.code.get_mut(path) else { return false };
            // A restart would also stop background tasks; wait for those too.
            for s in live.sessions.iter_mut().filter(|s| !s.busy && !s.has_tasks()) {
                s.kill();
            }
            live.busy() || live.sessions.iter().any(Session::has_tasks)
        });
    }
}

impl Focusable for KynexApp {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for KynexApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The UI zoom: everything sized in rems scales with it.
        window.set_rem_size(gpui::px(16.) * self.saved.zoom);
        let t = theme(cx).clone();
        if let Active::Project(i) = self.saved.active
            && let Some(p) = self.saved.projects.get_mut(i).filter(|p| p.view == View::Settings)
        {
            p.view = View::Code;
        }
        let view = self.active_project().map(|p| p.view);
        // Create (or resume) the project's session as soon as its tab is shown,
        // so a restored transcript is visible before the first message.
        // Showing it counts as seeing it finish, while Kynex is in front.
        let active = window.is_window_active();
        if view != Some(View::Git)
            && let Some(s) = self.session_mut()
            && active
        {
            s.finished = false;
        }
        self.sync_attention(cx);
        self.sync_awake();
        self.sync_draft(cx);
        self.sync_prompt(window, cx);
        self.sync_toasts(window, cx);
        self.load_project_settings();
        // Shortcuts (Esc, Ctrl+B…) only reach the workspace through the focused
        // element, so never leave the window with nothing focused.
        if window.focused(cx).is_none() {
            self.focus.focus(window, cx);
        }

        // Toasts sit in the corner picked in Settings, the same in every view;
        // the conversation's own bars and composer are measured to stay clear of.
        let toasts = self.render_toasts(window, cx);
        let in_conversation = matches!(self.setup, Setup::Checking | Setup::Ready) && view != Some(View::Git);
        if !in_conversation {
            self.toast_room.set(None);
        }
        let main = if let Some(setup) = self.render_setup(cx) {
            setup
        } else if view == Some(View::Git) {
            let path = self.active_project().map(|p| p.path.clone()).unwrap();
            let git = self.git.entry(path.clone()).or_insert_with(|| cx.new(|cx| GitView::new(path, cx))).clone();
            // The sessions panel stays, so the toolbar (and its Code / Git switch)
            // doesn't move when the view changes.
            div()
                .flex()
                .flex_1()
                .min_h_0()
                .children(self.render_sessions_column(window, cx))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .bg(t.editor)
                        .child(self.render_toolbar(cx))
                        // Fades in when you switch to it, as the transcript does.
                        .child(fade_in(
                            ("git-view-in", git.entity_id().as_u64() as usize),
                            div().flex_1().min_h_0().child(git),
                            motion::NORMAL,
                        )),
                )
                .into_any_element()
        } else {
            div()
                .flex()
                .flex_1()
                .min_h_0()
                .children(self.render_sessions_column(window, cx))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .bg(t.editor)
                        .child(self.render_toolbar(cx))
                        .children(self.render_session_header(cx))
                        // The timeline runs the full height beside the transcript
                        // and the composer under it, down to the terminal dock.
                        .child(
                            div()
                                .flex()
                                .flex_1()
                                .min_h_0()
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .flex_1()
                                        .min_w_0()
                                        .child(
                                            div()
                                                .relative()
                                                .flex()
                                                .flex_col()
                                                .flex_1()
                                                .min_h_0()
                                                .child(self.render_thread(window, cx))
                                                .child(self.toast_room_probe()),
                                        )
                                        .child(self.render_composer(window, cx)),
                                )
                                .children(self.render_timeline(cx)),
                        )
                        .children(self.render_terminal_dock(window, cx)),
                )
                .into_any_element()
        };

        let menu = self.draw_leaving(|s| &mut s.menu, |e| &mut e.menu, window, cx, |s, l, _, cx| s.render_menu(l, cx));
        let settings = self.draw_leaving(|s| &mut s.settings, |e| &mut e.settings, window, cx, Self::render_settings);
        let app_settings =
            self.draw_leaving(|s| &mut s.app_settings, |e| &mut e.app_settings, window, cx, Self::render_app_settings);
        let extensions =
            self.draw_leaving(|s| &mut s.extensions, |e| &mut e.extensions, window, cx, Self::render_extensions);
        let about = self.render_about(window, cx);
        let git_modal =
            self.draw_leaving(|s| &mut s.git_modal, |e| &mut e.git_modal, window, cx, Self::render_git_modal);
        let viewer =
            self.draw_leaving(|s| &mut s.viewer, |e| &mut e.viewer, window, cx, |s, l, _, _| s.render_image_viewer(l));
        div()
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
            .on_action(cx.listener(Self::open_project))
            .on_action(cx.listener(Self::open_settings_action))
            .on_action(cx.listener(|_, _: &keys::Quit, _, cx| cx.quit()))
            .on_action(cx.listener(|this, _: &keys::ZoomIn, _, cx| this.zoom_by(1, cx)))
            .on_action(cx.listener(|this, _: &keys::ZoomOut, _, cx| this.zoom_by(-1, cx)))
            .on_action(cx.listener(|this, _: &keys::ZoomReset, _, cx| this.zoom_by(0, cx)))
            .on_action(cx.listener(Self::new_session_action))
            .on_action(cx.listener(Self::next_session))
            .on_action(cx.listener(Self::prev_session))
            .on_action(cx.listener(Self::close_tab))
            .on_action(cx.listener(Self::next_tab))
            .on_action(cx.listener(Self::prev_tab))
            .on_action(cx.listener(Self::toggle_sessions))
            .on_action(cx.listener(Self::cycle_mode))
            .on_action(cx.listener(Self::interrupt_action))
            .on_action(cx.listener(Self::toggle_terminal))
            .on_action(cx.listener(Self::search_history))
            .on_action(cx.listener(Self::stash_action))
            .on_action(cx.listener(Self::pick_model))
            .on_action(cx.listener(Self::expand_tools))
            .on_action(cx.listener(Self::edit_in_editor_action))
            .on_action(cx.listener(Self::rewind_action))
            .on_action(cx.listener(Self::toggle_timeline))
            .on_action(cx.listener(Self::prev_turn))
            .on_action(cx.listener(Self::next_turn))
            .on_action(cx.listener(|this, _: &keys::PageUp, _, cx| this.scroll_keys(Scroll::Page(-1.), cx)))
            .on_action(cx.listener(|this, _: &keys::PageDown, _, cx| this.scroll_keys(Scroll::Page(1.), cx)))
            .on_action(cx.listener(|this, _: &keys::LineUp, _, cx| this.scroll_keys(Scroll::Line(-1.), cx)))
            .on_action(cx.listener(|this, _: &keys::LineDown, _, cx| this.scroll_keys(Scroll::Line(1.), cx)))
            .on_action(cx.listener(|this, _: &keys::ScrollTop, _, cx| this.scroll_keys(Scroll::Top, cx)))
            .on_action(cx.listener(|this, _: &keys::ScrollBottom, _, cx| this.scroll_keys(Scroll::Bottom, cx)))
            .on_action(cx.listener(|_, _: &CopySelection, _, cx| copy_selection(cx)))
            // The composer's Ctrl+C, passed on when nothing is selected in it.
            .on_action(cx.listener(|_, _: &crate::ui::input::Copy, _, cx| copy_selection(cx)))
            .on_key_down(cx.listener(Self::key_down))
            .on_drop(cx.listener(Self::on_drop))
            .when(self.resizing_sessions, |d| {
                d.cursor_col_resize()
                    .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                        this.saved.sessions_width = (f32::from(ev.position.x) / this.saved.zoom).clamp(180., 480.);
                        cx.notify();
                    }))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.resizing_sessions = false;
                            this.save();
                            cx.notify();
                        }),
                    )
            })
            .when(self.resizing_terminal, |d| {
                d.cursor_row_resize()
                    .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, window, cx| {
                        this.drag_terminal_edge(f32::from(ev.position.y), window);
                        cx.notify();
                    }))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.resizing_terminal = false;
                            this.save();
                            cx.notify();
                        }),
                    )
            })
            .child(self.wheel.listener())
            .child(self.render_title_bar(window, cx))
            .child(main)
            .when(self.saved.footer, |d| d.child(self.render_footer(cx)))
            .children(toasts)
            .children(git_modal)
            .children(settings)
            .children(app_settings)
            .children(extensions)
            .children(about)
            .children(menu)
            .children(viewer)
    }
}
