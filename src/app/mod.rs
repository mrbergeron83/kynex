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
//! | `toolbar.rs`        | Code / Git switch, path, cost, open-in buttons       |
//! | `sessions_panel.rs` | open and past sessions                               |
//! | `session_groups.rs` | session groups and the archive, their menus          |
//! | `session_header.rs` | the session's title, branch / worktree, turns, cost  |
//! | `terminal_dock.rs`  | the project's terminal panel under the conversation  |
//! | `titles.rs`         | naming sessions Claude Code didn't name              |
//! | `transcript.rs`     | the conversation list and how each item draws        |
//! | `tool_rows.rs`      | tool calls with their results, runs of calls folded  |
//! | `worktrees.rs`      | the project's worktrees, and which one Claude is in  |
//! | `git_modal.rs`      | a worktree's Git view in a modal, from the header    |
//! | `composer.rs`       | message box, attachments, pickers, send / stop       |
//! | `drafts.rs`         | each session's unsent message, kept across switches  |
//! | `queue.rs`          | messages queued while Claude works, Send now         |
//! | `history.rs`        | ↑ / ↓ history, Ctrl+R search, stash, Esc Esc, editor |
//! | `rewind.rs`         | rewind to an earlier message: conversation and files |
//! | `timeline.rs`       | the session's turns and steps beside the transcript  |
//! | `slash.rs`          | the slash-command popup, and built-ins Kynex runs   |
//! | `prompts.rs`        | permission / plan / question prompts                 |
//! | `menus.rs`          | popup menus                                          |
//! | `settings.rs`       | the project's Claude settings modal                  |
//! | `setup.rs`          | CLI install / sign-in screen and update strip        |
//! | `app_update.rs`     | Kynex updating itself: checks, the update toast      |
//! | `about.rs`          | the About Kynex modal: version, paths, updates       |
//! | `appearance.rs`     | each project's colour: title bar, tab, composer      |
//! | `models.rs`         | the model list                                       |

mod about;
mod app_update;
mod appearance;
mod composer;
mod drafts;
mod git_modal;
mod history;
mod keys;
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
mod terminal_dock;
mod timeline;
mod title_bar;
mod titles;
mod tool_rows;
mod toolbar;
mod transcript;
mod workspace;
mod worktrees;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use gpui::{
    App, Context, Entity, FocusHandle, Focusable, MouseButton, MouseMoveEvent, Window, WindowBounds, div, prelude::*,
    px,
};
use kynex_core::cli::UpdateState;
use kynex_core::live::{Key, Live};
use kynex_core::paths::chat_dir;
use kynex_core::session::{self, Attachment, Kind, Session};

use drafts::Draft;
use keys::{CopySelection, copy_selection};
pub use keys::{SHELL_KEYS, bind_keys};
use menus::OpenMenu;
use prompts::ActivePrompt;
use session_groups::Organize;
use sessions_panel::SessionList;
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
use crate::ui::input::{InputEvent, TextInput};
use crate::ui::motion::{self, fade_in, pop_in};
use crate::ui::smooth_scroll::SmoothWheel;
use crate::ui::{Icon, icon, icon_button, popover};

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
    /// Each project's terminal panel, created the first time it's opened.
    terminals: HashMap<PathBuf, Entity<TerminalPanel>>,
    threads: HashMap<Key, Thread>,
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
    notice: Option<String>,
    sessions: Option<SessionList>,
    /// Groups and the archive in the sessions panel.
    organize: Organize,
    /// Session id whose delete button was clicked once and is awaiting confirmation.
    confirm_delete: Option<String>,
    /// The session being named right now, and every one tried this run.
    naming: Option<String>,
    naming_tried: HashSet<String>,
    /// The project settings being edited; the modal is open while this is set.
    settings: Option<SettingsView>,
    /// Projects whose rules changed mid-reply; their process restarts once it's idle.
    restart_when_idle: HashSet<PathBuf>,
    prompt: Option<ActivePrompt>,
    menu: Option<OpenMenu>,
    resizing_sessions: bool,
    resizing_terminal: bool,
    /// Whether a newer Claude Code is out, or being installed.
    cli_update: UpdateState,
    /// Kynex's own update: found, downloading, installed or failed.
    app_update: UpdateState,
    /// When Kynex last asked GitHub for a newer version, and whether it's asking now.
    update_checked: Option<std::time::SystemTime>,
    update_checking: bool,
    /// The small "update available" toast under the title bar.
    update_toast: bool,
    /// Mouse-wheel turns made smooth for every scroll area.
    wheel: SmoothWheel,
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

        let mut chat = match saved.chat_session_id.clone() {
            Some(id) => Session::resume(Kind::Chat, chat_dir(), id),
            None => Session::new(Kind::Chat, chat_dir()),
        };
        chat.model = saved.model.clone();
        chat.wake = wake.clone();
        let chat = Live { sessions: vec![chat], active: 0 };

        let composer = cx.new(|cx| TextInput::new(cx).multiline());
        cx.subscribe_in(&composer, window, |this, _, ev, window, cx| match ev {
            InputEvent::Submit => this.send(window, cx),
            InputEvent::PasteImages(images) => {
                for i in images {
                    this.attach_bytes(&i.bytes, cx);
                }
            }
            InputEvent::Changed => {}
        })
        .detach();
        let login_code = cx.new(|cx| TextInput::new(cx).mono().placeholder("Paste the code here"));
        cx.subscribe(&login_code, |this, _, ev, cx| {
            if let InputEvent::Submit = ev {
                this.submit_login_code(cx);
            }
        })
        .detach();
        cx.spawn(async move |this, cx| this.update(cx, |this, cx| this.check_setup(cx))).detach();
        let group_name = cx.new(|cx| TextInput::new(cx).placeholder("Group name"));
        cx.subscribe(&group_name, |this, _, ev, cx| {
            if let InputEvent::Submit = ev {
                this.commit_rename(cx);
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
        cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active() {
                let folders: Vec<PathBuf> = this.worktrees.keys().cloned().collect();
                this.refresh_worktrees(&folders, cx);
            }
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
            threads: HashMap::new(),
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
            notice: None,
            sessions: None,
            organize: Organize::new(group_name),
            confirm_delete: None,
            naming: None,
            naming_tried: HashSet::new(),
            settings: None,
            restart_when_idle: HashSet::new(),
            prompt: None,
            menu: None,
            resizing_sessions: false,
            resizing_terminal: false,
            cli_update: UpdateState::Idle,
            app_update: UpdateState::Idle,
            update_checked: None,
            update_checking: false,
            update_toast: false,
            wheel: SmoothWheel::default(),
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
        // Folders whose files may have just changed: a turn ended or a tool call finished.
        let mut touched = HashSet::new();
        for s in self.all_sessions_mut() {
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
            if let Some(init) = s.take_init() {
                reports.push((s.cwd.clone(), init));
            }
        }
        // What a session reported as it started: its folder's commands, the models.
        for (cwd, init) in reports {
            self.commands_reported(cwd, init.commands, cx);
            models::reported(init.models, cx);
        }
        self.refresh_worktrees(&touched, cx);
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
        cx.notify();
    }

    /// Restart the projects whose settings changed, once they're idle.
    fn restart_idle(&mut self) {
        self.restart_when_idle.retain(|path| {
            let Some(live) = self.code.get_mut(path) else { return false };
            for s in live.sessions.iter_mut().filter(|s| !s.busy) {
                s.kill();
            }
            live.busy()
        });
    }

    fn render_notice(&mut self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let n = self.notice.clone()?;
        let t = theme(cx).clone();
        Some(
            div().absolute().bottom_4().right_4().max_w(px(480.)).child(pop_in(
                "notice-in",
                popover(cx)
                    .flex_row()
                    .items_start()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .child(icon(Icon::Dot, t.warning).mt_0p5())
                    .child(div().flex_1().text_sm().child(n))
                    .child(icon_button("dismiss-notice", Icon::Close, cx).on_click(cx.listener(|this, _, _, cx| {
                        this.notice = None;
                        cx.notify();
                    }))),
                px(6.),
            )),
        )
    }
}

impl Focusable for KynexApp {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for KynexApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx).clone();
        if let Active::Project(i) = self.saved.active
            && let Some(p) = self.saved.projects.get_mut(i).filter(|p| p.view == View::Settings)
        {
            p.view = View::Code;
        }
        let view = self.active_project().map(|p| p.view);
        // Create (or resume) the project's session as soon as its tab is shown,
        // so a restored transcript is visible before the first message.
        // Showing it counts as seeing it finish.
        if view != Some(View::Git)
            && let Some(s) = self.session_mut()
        {
            s.finished = false;
        }
        self.sync_draft(cx);
        self.sync_prompt(window, cx);
        self.load_appearances();
        // Shortcuts (Esc, Ctrl+B…) only reach the workspace through the focused
        // element, so never leave the window with nothing focused.
        if window.focused(cx).is_none() {
            self.focus.focus(window, cx);
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
                .children(self.render_sessions_column(cx))
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
                .children(self.render_sessions_column(cx))
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
                                        .child(self.render_thread(window, cx))
                                        .child(self.render_composer(window, cx)),
                                )
                                .children(self.render_timeline(cx)),
                        )
                        .children(self.render_terminal_dock(window, cx)),
                )
                .into_any_element()
        };

        let menu = self.render_menu(cx);
        let settings = self.render_settings(window, cx);
        let notice = self.render_notice(cx);
        let toast = self.render_update_toast(cx);
        let about = self.render_about(window, cx);
        let git_modal = self.render_git_modal(window, cx);
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
                        this.saved.sessions_width = f32::from(ev.position.x).clamp(180., 480.);
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
            .children(notice)
            .children(toast)
            .children(git_modal)
            .children(settings)
            .children(about)
            .children(menu)
    }
}
