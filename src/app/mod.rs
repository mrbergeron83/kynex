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
//! | `session_header.rs` | the session's title, branch / worktree, turns, cost  |
//! | `terminal_dock.rs`  | the project's terminal panel under the conversation  |
//! | `titles.rs`         | naming sessions Claude Code didn't name              |
//! | `transcript.rs`     | the conversation list and how each item draws        |
//! | `worktrees.rs`      | the project's worktrees, and which one Claude is in  |
//! | `composer.rs`       | message box, attachments, pickers, send / stop       |
//! | `prompts.rs`        | permission / plan / question prompts                 |
//! | `menus.rs`          | popup menus                                          |
//! | `settings.rs`       | the project's Claude settings modal                  |
//! | `setup.rs`          | CLI install / sign-in screen and update strip        |
//! | `app_update.rs`     | Kynex updating itself from its GitHub releases       |
//! | `models.rs`         | the model list                                       |

mod app_update;
mod composer;
mod keys;
mod menus;
mod models;
mod prompts;
mod session_header;
mod sessions_panel;
mod settings;
mod setup;
mod state;
mod terminal_dock;
mod title_bar;
mod titles;
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

pub use keys::bind_keys;
use keys::{CopySelection, copy_selection};
use menus::OpenMenu;
use prompts::ActivePrompt;
use sessions_panel::SessionList;
use settings::SettingsView;
use setup::Setup;
pub use state::Saved;
use state::{Active, View, WindowState};
use transcript::Thread;
use worktrees::Worktrees;

use crate::git::GitView;
use crate::terminal::TerminalPanel;
use crate::theme::{theme, ui_font};
use crate::ui::input::{InputEvent, TextInput};
use crate::ui::{Icon, icon, icon_button, popover};

pub struct KynexApp {
    saved: Saved,
    code: HashMap<PathBuf, Live>,
    chat: Live,
    git: HashMap<PathBuf, Entity<GitView>>,
    /// Each open project's git worktrees, kept fresh in the background.
    worktrees: HashMap<PathBuf, Entity<Worktrees>>,
    /// Each project's terminal panel, created the first time it's opened.
    terminals: HashMap<PathBuf, Entity<TerminalPanel>>,
    threads: HashMap<Key, Thread>,
    /// Tool calls and results opened up in the transcript.
    expanded: HashSet<(Key, usize)>,
    composer: Entity<TextInput>,
    new_rule: Entity<TextInput>,
    attachments: Vec<Attachment>,
    notice: Option<String>,
    sessions: Option<SessionList>,
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
    /// Kynex's own update, shown in the same strip style as the CLI's.
    app_update: UpdateState,
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
        cx.on_app_quit(|this, _| {
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
            worktrees: HashMap::new(),
            terminals: HashMap::new(),
            threads: HashMap::new(),
            expanded: HashSet::new(),
            composer,
            new_rule,
            attachments: Vec::new(),
            notice: None,
            sessions: None,
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
        for s in self.all_sessions_mut() {
            let before = s.wants_you();
            s.poll();
            called |= !before && s.wants_you();
        }
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
            div().absolute().bottom_4().right_4().max_w(px(480.)).child(
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
            ),
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
        self.sync_prompt(window, cx);
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
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .bg(t.editor)
                .child(self.render_toolbar(cx))
                .child(div().flex_1().min_h_0().child(git))
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
                        .child(self.render_thread(cx))
                        .child(self.render_composer(window, cx))
                        .children(self.render_terminal_dock(window, cx)),
                )
                .into_any_element()
        };

        let menu = self.render_menu(cx);
        let settings = self.render_settings(window, cx);
        let notice = self.render_notice(cx);
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
            .child(self.render_title_bar(window, cx))
            .child(main)
            .children(notice)
            .children(settings)
            .children(menu)
    }
}
