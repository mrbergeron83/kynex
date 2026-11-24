//! The old Claude chat as a pane (`ClaudePane`): a `claude` session driven by
//! `kynex_core::claude::session` (its own process over stream-json), drawn as the old
//! transcript and composer. A drop-in for `ClaudeChatView` behind `KYNEX_OLD_CHAT=1`
//! while it's ported (#545); the app hosts it through the same `ClaudeChatConfig` and
//! `ClaudeChatEvent`.
//!
//! The session's reader threads ring a waker; a task here takes what came in and
//! redraws, so nothing polls while it's idle.

mod composer;
mod history;
mod input_modes;
pub(crate) mod keys;
mod mentions;
mod menus;
mod models;
mod queue;
mod session_header;
mod shell_runs;
mod slash;
mod tasks;
mod thinking;
mod todos;
mod tool_rows;
mod transcript;
mod transcript_rows;
mod viewer;

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use gpui::{App, Context, Entity, EventEmitter, FocusHandle, Focusable, Window, div, prelude::*};
use kynex_core::agent_panes::AgentState;
use kynex_core::claude::column::ColumnWidth;
use kynex_core::claude::input::input_mode::InputMode;
use kynex_core::claude::input::paste::Pastes;
use kynex_core::claude::prompt_history::History;
use kynex_core::claude::session::{self, Attachment, Item, Kind, Session};
use kynex_core::project::{ClaudeSettings, Permissions};
use kynex_core::toasts::Toast;

use crate::claude_chat::{ClaudeChatConfig, ClaudeChatEvent};
use crate::theme::theme;
use crate::ui::completions::Completion;
use crate::ui::image_viewer::ImageViewer;
use crate::ui::input::{InputEvent, TextInput};
use crate::ui::motion::{Leaving, QUICK};
use history::HistorySearch;
use mentions::Mentions;
use slash::Slash;
use transcript::Thread;
use viewer::LoggedImages;

/// The session's id among this run's sessions ([`Session::uid`]): it keys the
/// transcript's selection group and the rows' open/closed state.
pub(crate) type Key = u64;

/// Whether `claude` panes use this pane instead of `ClaudeChatView` (`KYNEX_OLD_CHAT=1`).
pub(crate) fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("KYNEX_OLD_CHAT").is_ok_and(|v| v.trim() == "1"))
}

/// How many characters of the first message make the pane's title.
const TITLE_CHARS: usize = 60;

pub struct ClaudePane {
    config: ClaudeChatConfig,
    focus: FocusHandle,
    pub(crate) session: Session,
    /// The project's Claude settings (model, effort, thinking), as last read.
    claude: ClaudeSettings,
    /// The transcript list, once drawn.
    thread: Option<Thread>,
    /// Rows opened by hand, by item.
    expanded: HashSet<(Key, usize)>,
    /// Runs of tool calls opened by hand, by their first call.
    open_runs: HashSet<(Key, usize)>,
    /// Calls auto mode refused that were allowed from their row, with what was allowed.
    unblocked: HashMap<(Key, usize), String>,
    /// Every tool row open (Ctrl+O).
    expand_tools: bool,
    /// The 1 s repaint for clocks runs.
    clocks_ticking: bool,
    /// The Transcript width setting.
    pub(crate) column: ColumnWidth,
    /// The UI zoom, from the window's rem size: the columns are in sizes at 100%.
    zoom: f32,
    /// The project's colour, by name ([`kynex_core::project::COLOURS`]).
    colour: Option<String>,
    viewer: Option<Entity<ImageViewer>>,
    logged: RefCell<LoggedImages>,
    /// The message box.
    composer: Entity<TextInput>,
    /// Images attached to the message being written.
    attachments: Vec<Attachment>,
    /// The slash-command popup.
    slash: Slash,
    /// The `@file` mention popup.
    mentions: Mentions,
    /// The slash or mention list as last drawn, playing out once it closes.
    slash_exit: Leaving<(Rc<Vec<Completion>>, usize)>,
    /// Ctrl+R over the sent messages.
    history_search: Option<HistorySearch>,
    /// What was sent in this project, for ↑ / ↓ and Ctrl+R; and unsent drafts.
    history: History,
    /// Counts draft edits, so only the last one in a burst is saved.
    draft_gen: u64,
    /// When Esc last went to a draft, for Esc Esc.
    last_esc: Option<Instant>,
    /// The composer's mode: a prompt, or `!` shell, `#` memory, `?` shortcuts.
    input_mode: InputMode,
    /// The composer's text as of its last change, to tell what a change did.
    last_text: String,
    /// Long pastes folded into chips in the composer.
    pastes: Pastes,
    /// The draft is open in an outside editor (Ctrl+G).
    editing_outside: bool,
    /// The composer's menu (model, effort, mode, memory), while open.
    menu: Option<menus::OpenMenu>,
    /// The todo list folded open by hand after its turn.
    todos_open: bool,
    /// What the app was last told.
    state: AgentState,
    detail: String,
    title: Option<String>,
    reported_id: Option<String>,
    /// The name Kynex gave the session, once it has one.
    name: Option<String>,
    /// A turn was running as of the last wake.
    was_busy: bool,
}

impl EventEmitter<ClaudeChatEvent> for ClaudePane {}

impl Focusable for ClaudePane {
    /// The message box: mounting or focusing the pane puts the cursor there.
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.composer.read(cx).focus_handle(cx)
    }
}

impl ClaudePane {
    pub fn new(config: ClaudeChatConfig, window: &mut Window, cx: &mut Context<Self>) -> Self {
        session::set_preview_maker(crate::ui::image_viewer::preview);
        // The reader threads ring this; capacity 1 folds a burst into one wake.
        let (wake_tx, wake_rx) = async_channel::bounded::<()>(1);
        let wake: session::Wake = Arc::new(move || {
            let _ = wake_tx.try_send(());
        });
        let cwd = config.workspace.clone();
        let mut session = if let Some(from) = config.fork_from.clone() {
            Session::fork(Kind::Code, cwd, from, wake)
        } else if let Some(id) = config.resume_session_id.clone() {
            Session::resume(Kind::Code, cwd, id, wake)
        } else {
            let mut s = Session::new(Kind::Code, cwd);
            s.wake = wake;
            s
        };
        let provider = if session.is_new() {
            session.set_provider(config.launch.clone())
        } else {
            session.restore_provider(config.launch.clone())
        };
        if let Err(e) = provider {
            session.items.push(Item::Error(e));
        }
        session.extra_env = config.extra_env.clone();
        let colour = project_colour(session.settings_folder());
        let claude = ClaudeSettings::load(session.settings_folder());

        cx.spawn(async move |this, cx| {
            while wake_rx.recv().await.is_ok() {
                if this.update(cx, |this, cx| this.woke(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        // Thumbnails give their textures back; the session's process ends as it drops.
        cx.on_release(|this, cx| {
            for image in this.logged.borrow_mut().cache.clear() {
                cx.drop_image(image, None);
            }
        })
        .detach();

        let composer = cx.new(|cx| TextInput::new(cx).multiline().fold_pastes());
        cx.subscribe_in(&composer, window, |this, _, ev: &InputEvent, window, cx| match ev {
            InputEvent::Submit => this.send(window, cx),
            InputEvent::PasteImages(images) => {
                for i in images {
                    this.attach_bytes(&i.bytes, cx);
                }
            }
            InputEvent::Changed => {
                this.warm();
                this.composer_changed(cx);
                this.draft_changed(cx);
            }
            InputEvent::PasteFolded(text) | InputEvent::PasteText(text) => this.fold_paste(text.clone(), cx),
            InputEvent::MarkClicked(range) => this.expand_chip(range.clone(), cx),
        })
        .detach();
        // Focusing the message box means a message is coming: Claude starts now.
        let focus = composer.read(cx).focus_handle(cx);
        cx.on_focus(&focus, window, |this, _, _| this.warm()).detach();

        let reported_id = config.resume_session_id.clone();
        let mut pane = Self {
            config,
            focus: cx.focus_handle(),
            session,
            claude,
            thread: None,
            expanded: HashSet::new(),
            open_runs: HashSet::new(),
            unblocked: HashMap::new(),
            expand_tools: false,
            clocks_ticking: false,
            column: ColumnWidth::default(),
            zoom: 1.,
            colour,
            viewer: None,
            logged: RefCell::default(),
            composer,
            attachments: Vec::new(),
            slash: Slash::default(),
            mentions: Mentions::default(),
            slash_exit: Leaving::new(QUICK),
            history_search: None,
            history: History::default(),
            draft_gen: 0,
            last_esc: None,
            input_mode: InputMode::Prompt,
            last_text: String::new(),
            pastes: Pastes::default(),
            editing_outside: false,
            menu: None,
            todos_open: false,
            state: AgentState::Idle,
            detail: "Ready".into(),
            title: None,
            reported_id,
            name: None,
            was_busy: false,
        };
        // A model given with the pane (a restart, a pick kept with it) is this session's own.
        // On OpenRouter the app fills in its default when the pane has none: that's no pin.
        let openrouter_default = pane.config.openrouter.default_model().to_string();
        let openrouter = pane.session.provider().provider().is_openrouter();
        let model = pane.config.model.clone().filter(|m| !m.trim().is_empty());
        let model = model.filter(|m| !(openrouter && *m == openrouter_default));
        if let Some(model) = model.filter(|m| *m != pane.folder_model()) {
            pane.session.own_model = Some(model);
        }
        pane.session.use_model(pane.active_model());
        pane.load_history(cx);
        pane
    }

    /// Runs the session under `dir`'s Kynex settings (its project's, for a worktree).
    pub fn set_settings_dir(&mut self, dir: PathBuf) {
        if self.session.settings_folder() != dir {
            self.colour = project_colour(&dir);
            self.claude = ClaudeSettings::load(&dir);
            self.session.settings_dir = Some(dir);
            if self.session.own_model.is_none() {
                self.session.use_model(self.folder_model());
            }
        }
    }

    /// Project `dir`'s Claude settings changed in another pane: this one follows,
    /// live where the CLI takes them, else from its next start.
    pub fn settings_changed(&mut self, dir: &Path, cx: &mut Context<Self>) {
        if !kynex_core::workspace::same_folder(self.session.settings_folder(), dir) {
            return;
        }
        self.claude = ClaudeSettings::load(dir);
        self.apply_project_settings();
        cx.notify();
    }

    /// Brings the session up to its project's rules and settings.
    fn apply_project_settings(&mut self) {
        let dir = self.session.settings_folder().to_path_buf();
        self.session.apply_settings(&Permissions::load(&dir), &self.claude);
        self.restart_idle();
    }

    /// A session whose settings changed under it starts again once it's idle;
    /// a restart would also stop background tasks, so it waits for those too.
    fn restart_idle(&mut self) {
        let s = &mut self.session;
        if s.stale && !s.busy && !s.has_tasks() {
            s.kill();
        }
    }

    /// Starts the process ahead of the first message, on the model it'll use.
    fn warm(&mut self) {
        if !self.session.is_running() {
            let model = self.active_model();
            self.session.use_model(model);
            self.session.warm();
        }
    }

    pub fn status(&self) -> AgentState {
        self.state
    }

    pub fn session_id(&self) -> Option<&str> {
        self.session.session_id.as_deref()
    }

    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// A message from outside (`knx pane prompt`): sent now, or queued behind the turn.
    pub fn send_external_prompt(&mut self, text: String, cx: &mut Context<Self>) {
        if self.session.busy {
            self.session.queue.push(text, Vec::new());
        } else {
            let model = self.active_model();
            self.session.use_model(model);
            self.session.send(text, Vec::new());
        }
        self.follow_tail(cx);
        self.sync(cx);
        cx.notify();
    }

    /// Stops the turn; anything queued goes once it has ended.
    pub fn cancel_turn(&mut self, cx: &mut Context<Self>) {
        self.session.interrupt();
        cx.notify();
    }

    /// Ends the process; the transcript stays, and the next message resumes it.
    pub fn stop(&mut self, cx: &mut Context<Self>) {
        self.session.kill();
        self.set_status(AgentState::Idle, "Stopped", cx);
        cx.notify();
    }

    /// The session's history or process had news: take it in and redraw.
    fn woke(&mut self, cx: &mut Context<Self>) {
        if let Some(n) = self.session.take_history() {
            self.history_came(self.key(), n);
        }
        self.session.poll();
        // As its process starts, a session reports its folder's commands and the models.
        if let Some(init) = self.session.take_init() {
            let cwd = self.session.cwd.clone();
            self.commands_reported(cwd, init.commands, cx);
            models::reported(init.models, cx);
        }
        self.sync(cx);
        cx.notify();
    }

    /// Tells the app what changed: the session's id, its title, its state, a turn's end.
    fn sync(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.session.session_id.clone()
            && !self.session.is_new()
            && self.reported_id.as_deref() != Some(id.as_str())
        {
            self.reported_id = Some(id.clone());
            self.name = kynex_core::claude::titles::stored(&id);
            self.restore_draft(cx);
            cx.emit(ClaudeChatEvent::SessionId(id));
        }
        if self.name.is_none()
            && let Some(id) = self.session.session_id.as_deref()
        {
            self.name = kynex_core::claude::titles::stored(id);
        }
        if self.title.is_none()
            && let Some(title) = first_line(&self.session.items)
        {
            self.title = Some(title.clone());
            cx.emit(ClaudeChatEvent::TitleChanged(title));
        }
        let busy = self.session.busy;
        let ended = std::mem::replace(&mut self.was_busy, busy) && !busy;
        if self.session.waiting() {
            self.set_status(AgentState::NeedsYou, "Needs you", cx);
        } else if busy {
            let doing = self.doing();
            self.set_status(AgentState::Working, doing, cx);
        } else if ended {
            let finished = matches!(self.session.items.last(), Some(Item::Result { is_error: false, .. }));
            if finished {
                self.set_status(AgentState::Done, "Done", cx);
            } else {
                self.set_status(AgentState::Idle, "Stopped", cx);
            }
            self.turn_ended(self.key());
            self.restart_idle();
            cx.emit(ClaudeChatEvent::TurnFinished);
        } else if self.state != AgentState::Done {
            self.set_status(AgentState::Idle, "Ready", cx);
        }
    }

    fn set_status(&mut self, state: AgentState, detail: impl Into<String>, cx: &mut Context<Self>) {
        let detail = detail.into();
        if self.state != state || self.detail != detail {
            self.state = state;
            self.detail = detail.clone();
            cx.emit(ClaudeChatEvent::StatusChanged { state, detail });
        }
    }

    /// What the turn is doing, in a few words, for the pane's header and the Agents pane.
    fn doing(&self) -> String {
        let turn = self.session.items.iter().rev().take_while(|i| !matches!(i, Item::User { .. }));
        for item in turn {
            match item {
                Item::ToolUse { name, timing, .. } if timing.is_running() => return format!("Running {name}…"),
                Item::Thinking(t) if t.is_live() => return "Thinking…".into(),
                Item::Assistant(_) => return "Responding…".into(),
                _ => {}
            }
        }
        "Working…".into()
    }

    fn key(&self) -> Key {
        self.session.uid
    }

    /// The pane's one session, as the old code (one app, many sessions) asked for it.
    fn session_by_key(&self, _key: Key) -> Option<&Session> {
        Some(&self.session)
    }

    fn session_for(&mut self, _key: Key) -> Option<&mut Session> {
        Some(&mut self.session)
    }

    fn thread_of(&self, _key: Key) -> Option<&Thread> {
        self.thread.as_ref()
    }

    fn thread_mut(&mut self, _key: Key) -> Option<&mut Thread> {
        self.thread.as_mut()
    }

    /// The model it runs on, in words.
    fn model_name(&self) -> String {
        let launch = self.session.provider().with_model(&self.session.model);
        launch.model().filter(|m| !m.is_empty()).unwrap_or("the default model").to_string()
    }

    /// Row `row` lit up after a timeline jump; the timeline comes with stage 4.
    fn flash_row(
        &self,
        _key: Key,
        _row: usize,
        el: gpui::Stateful<gpui::Div>,
        _t: &crate::theme::Palette,
    ) -> gpui::AnyElement {
        el.into_any_element()
    }

    fn toast(&mut self, toast: Toast, cx: &mut Context<Self>) {
        cx.emit(ClaudeChatEvent::Toast(toast));
    }

    fn run_in_terminal(&mut self, command: String, _window: &mut Window, cx: &mut Context<Self>) {
        cx.emit(ClaudeChatEvent::RunInTerminal(command));
    }

    fn open_file_changes(&mut self, cwd: PathBuf, path: PathBuf, cx: &mut Context<Self>) {
        cx.emit(ClaudeChatEvent::OpenFileChanges { cwd, path });
    }
}

/// The name of `project`'s colour, if it has one. Blocks on a small file read.
fn project_colour(project: &Path) -> Option<String> {
    kynex_core::project::Appearance::load(project).colour(project).map(str::to_string)
}

/// The first line of the first message, for the pane's title.
fn first_line(items: &[Item]) -> Option<String> {
    let text = items.iter().find_map(|i| match i {
        Item::User { text, .. } if !text.trim().is_empty() => Some(text.as_str()),
        _ => None,
    })?;
    let line = text.lines().find(|l| !l.trim().is_empty())?.trim();
    let mut title: String = line.chars().take(TITLE_CHARS).collect();
    if line.chars().count() > TITLE_CHARS {
        title.push('…');
    }
    Some(title)
}

impl Render for ClaudePane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.zoom = f32::from(window.rem_size()) / 16.;
        let key = self.key();
        let t = theme(cx).clone();
        let header = self.render_session_header(key, cx);
        let thread = self.render_thread(key, window, cx);
        let composer = self.render_composer(Some(key), window, cx);
        let menu = self.render_menu(cx);
        let viewer = self.render_image_viewer(window);
        div()
            .id("claude-pane")
            .track_focus(&self.focus)
            .key_context("ClaudePane")
            .size_full()
            .flex()
            .flex_col()
            .bg(t.editor)
            .on_drop(cx.listener(Self::on_drop))
            .map(|d| self.bind_actions(d, cx))
            .children(header)
            .child(thread)
            .child(composer)
            .children(menu)
            .children(viewer)
    }
}
