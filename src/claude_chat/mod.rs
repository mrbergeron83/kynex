//! First-class native Claude Code chat & composer view (`ClaudeChatView`).
//!
//! Talks directly to the user's installed `claude` CLI over `kynex_core::claude`
//! (`ClaudeClient`), driven by a zero-polling `async_channel` wake loop (0% idle CPU)
//! and deterministic `cx.drop_image` GPU texture cleanup.

mod cards;
mod composer;
mod composer_popups;
mod events;
mod history_keys;
mod input_modes;
mod keys;
mod markdown_view;
mod media;
mod overlays;
mod runs;
mod state;
mod tool_rows;
mod transcript;

use std::cell::Cell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, FontWeight, ListAlignment, ListState, Window, div,
    prelude::*,
};
use kynex_core::agent_cli;
use kynex_core::agent_panes::AgentState;
use kynex_core::claude::composer::{CommandItem, FileIndex, PasteFold};
use kynex_core::claude::history::{self, LoadedChunk};
use kynex_core::claude::{
    ClaudeClient, EffortLevel, InitInfo, PermissionAnswer, PermissionMode, PromptBlock, SessionStart, SpawnConfig,
    TokenUsage, costs,
};

use state::*;
pub use state::{ClaudeChatConfig, ClaudeChatEvent};

use crate::theme::{mono, theme, ui_font};
use crate::ui::input::{InputEvent, TextInput};
use crate::ui::{Icon, Lamp, Tooltip, ghost_button, icon, icon_button, lamp, px};

/// Standalone Claude Desktop-style gpui view for `claude` & `claude-openrouter` panes.
pub struct ClaudeChatView {
    pub(super) config: ClaudeChatConfig,
    pub(super) focus: FocusHandle,
    pub(super) client: Option<ClaudeClient>,
    pub(super) history_rx: Option<mpsc::Receiver<LoadedChunk>>,
    pub(super) history_loading: bool,
    pub(super) loading_older_history: bool,

    // Session state
    pub(super) ready: bool,
    pub(super) exited: bool,
    pub(super) turn_active: bool,
    pub(super) state: AgentState,
    pub(super) status_detail: String,
    pub(super) session_id: Option<String>,
    pub(super) title: Option<String>,
    pub(super) init_info: Option<InitInfo>,
    pub(super) active_model: Option<String>,
    pub(super) effort: EffortLevel,
    pub(super) permission_mode: PermissionMode,
    pub(super) fast_mode: bool,

    // Usage & cumulative cost
    pub(super) context_used: u64,
    pub(super) context_window: Option<u64>,
    pub(super) last_usage: TokenUsage,
    pub(super) process_cost_usd: f64,
    pub(super) cumulative_cost_usd: f64,
    pub(super) cost_loaded: bool,

    // Transcript
    pub(super) rows: Vec<LogRow>,
    /// What the list draws: rows, and runs of tool calls (`runs.rs`).
    pub(super) items: Vec<kynex_core::claude::runs::ListItem>,
    /// Runs opened to their calls, by their first row's id.
    pub(super) open_runs: HashSet<u64>,
    /// Ctrl+O: every tool call shown open.
    pub(super) expand_tools: bool,
    pub(super) list_state: ListState,
    /// The transcript's usable width last drawn, for the wide tool lane (0 before the first frame).
    pub(super) column_width: Rc<Cell<f32>>,
    pub(super) stick_to_bottom: bool,
    /// Smooth scrolling in progress, and when its last frame ran.
    pub(super) glide: kynex_core::motion::Glide,
    pub(super) last_frame: Option<Instant>,
    /// Trailing a streaming reply (or gliding to the end): follow once there.
    pub(super) follow_after: bool,
    /// A turn just finished: go to its end on the next frame.
    pub(super) turn_ended: bool,
    /// The scroll offset after the last frame, to notice a scroll by hand.
    pub(super) last_offset: f32,
    /// When the current turn started, for the status line's clock.
    pub(super) turn_started: Option<Instant>,
    /// Whether the once-a-second repaint for clocks is running.
    pub(super) clock_ticking: bool,
    pub(super) streaming_tool_json: HashMap<String, String>,
    pub(super) copied_key: Option<String>,
    copied_gen: u64,

    // Plan / Todo strip & Subagents
    pub(super) plan_steps: Vec<PlanStep>,
    pub(super) plan_expanded: bool,
    pub(super) subagents: Vec<SubagentTrack>,
    pub(super) peek_subagent: Option<String>,

    // Interactive cards & queue
    pub(super) pending_card: Option<PendingCard>,
    pub(super) queued_cards: VecDeque<PendingCard>,
    pub(super) queued: VecDeque<QueuedMessage>,
    pub(super) prompt_suggestion: Option<String>,

    // Composer, inputs, popups, find
    pub(super) composer_input: Entity<TextInput>,
    pub(super) card_input: Entity<TextInput>,
    pub(super) find_input: Entity<TextInput>,
    pub(super) find: FindState,
    pub(super) paste_fold: PasteFold,
    pub(super) file_index: FileIndex,
    pub(super) slash_commands: Vec<CommandItem>,
    pub(super) completion: Option<CompletionPopup>,
    /// What's been sent in this project, the stash and per-session drafts.
    pub(super) prompt_history: kynex_core::claude::prompt_history::History,
    /// Bumped per keystroke, so only the last pause saves the draft.
    pub(super) draft_gen: u64,
    /// The last lone Esc, for Esc Esc.
    pub(super) last_esc: Option<Instant>,
    /// The composer's `!` / `#` / `?` mode, and its text before the last change.
    pub(super) input_mode: kynex_core::claude::input::input_mode::InputMode,
    pub(super) last_text: String,
    /// The menu of where a `#` note goes is open.
    pub(super) memory_menu: bool,
    /// Alt+T: thinking on for this session.
    pub(super) thinking_on: bool,
    /// The CLI acts on `ultrathink` with this model (highlight it).
    pub(super) ultrathink: bool,
    /// Ctrl+G: the draft is open in an editor.
    pub(super) editing_outside: bool,
    pub(super) open_popover: Option<OpenPopover>,

    // Media & Lightbox (all RenderImage textures deterministically dropped via cx.drop_image)
    pub(super) attachments: Vec<ComposerAttachment>,
    pub(super) image_cache: media::ImageCache,
    pub(super) lightbox: Option<LightboxImage>,

    next_seq: u64,
}

impl Focusable for ClaudeChatView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        if self.find.open {
            self.find_input.read(cx).focus_handle(cx)
        } else if self.pending_card.is_some() {
            self.focus.clone()
        } else {
            self.composer_input.read(cx).focus_handle(cx)
        }
    }
}

impl EventEmitter<ClaudeChatEvent> for ClaudeChatView {}

impl ClaudeChatView {
    pub fn new(config: ClaudeChatConfig, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();

        // Zero-polling wake channel (capacity 1 coalesces bursts cleanly).
        let (wake_tx, wake_rx) = async_channel::bounded::<()>(1);
        let wake: kynex_core::claude::Wake = Arc::new(move || {
            let _ = wake_tx.try_send(());
        });

        let session_start = if let Some(fork) = &config.fork_from {
            SessionStart::Fork(fork.clone())
        } else if let Some(resume) = &config.resume_session_id {
            SessionStart::Resume(resume.clone())
        } else {
            SessionStart::New
        };

        let spawn_cfg = SpawnConfig {
            cwd: config.workspace.clone(),
            session: session_start,
            launch: config.launch.clone(),
            model: config.model.clone(),
            effort: None,
            permission_mode: None,
            settings_json: None,
            extra_env: config.extra_env.clone(),
            on_wake: Some(Arc::clone(&wake)),
        };

        let mut initial_error: Option<String> = None;
        let client = match ClaudeClient::spawn(spawn_cfg) {
            Ok(c) => Some(c),
            Err(e) => {
                initial_error = Some(e);
                None
            }
        };

        // Progressive on-disk `.jsonl` history load for resumed or forked sessions.
        let history_sid = config.resume_session_id.clone().or_else(|| config.fork_from.clone());
        let (history_rx, history_loading) = if let Some(sid) = history_sid {
            let (tx, rx) = mpsc::channel::<LoadedChunk>();
            history::load_progressive(config.workspace.clone(), sid, tx, Arc::clone(&wake));
            (Some(rx), true)
        } else {
            (None, false)
        };

        let composer_input = cx.new(|cx| {
            TextInput::new(cx).multiline().intercept_long_paste().placeholder(composer::COMPOSER_PLACEHOLDER)
        });
        let card_input = cx.new(|cx| TextInput::new(cx).placeholder("Add note or feedback…"));
        let find_input = cx.new(|cx| TextInput::new(cx).placeholder("Find in transcript…"));

        cx.subscribe_in(&composer_input, _window, |this, _, ev: &InputEvent, window, cx| match ev {
            InputEvent::Changed => {
                this.composer_mode_changed(cx);
                this.sync_composer_marks(cx);
                this.refresh_completion_popup(cx);
                this.draft_changed(cx);
                // Typing takes over from a selection in the transcript (Ctrl+C copies the draft's).
                crate::ui::selectable::clear(cx);
                cx.notify();
            }
            InputEvent::Submit => {
                if this.completion.is_some() {
                    this.accept_completion(window, cx);
                } else {
                    this.submit_composer(false, window, cx);
                }
            }
            InputEvent::PasteImages(imgs) => this.on_paste_images(imgs.clone(), cx),
            InputEvent::PasteText(text) => this.on_long_paste(text.clone(), window, cx),
            InputEvent::MarkClicked(range) => this.on_composer_mark_clicked(range.clone(), window, cx),
        })
        .detach();

        cx.subscribe_in(&find_input, _window, |this, _, ev: &InputEvent, _window, cx| match ev {
            InputEvent::Changed => {
                this.find.query = this.find_input.read(cx).text().to_string();
                this.recompute_find_matches();
                if let Some(&ix) = this.find.matches.first() {
                    this.find.current = 0;
                    this.reveal_row(ix);
                }
                cx.notify();
            }
            InputEvent::Submit => this.step_find(1, cx),
            _ => {}
        })
        .detach();

        cx.subscribe_in(&card_input, _window, |this, _, ev: &InputEvent, window, cx| {
            if matches!(ev, InputEvent::Submit)
                && let Some(card) = this.pending_card.clone()
            {
                match card {
                    PendingCard::AskUserQuestion { req_id, .. } => {
                        this.submit_ask_questions(&req_id, window, cx);
                    }
                    PendingCard::Permission { req_id, .. } => {
                        let note = this.take_card_input(cx);
                        this.answer_permission(
                            &req_id,
                            PermissionAnswer::Deny { message: note, interrupt: false },
                            window,
                            cx,
                        );
                    }
                    PendingCard::ExitPlanMode { req_id, .. } => {
                        let fb = this.take_card_input(cx);
                        this.answer_permission(
                            &req_id,
                            PermissionAnswer::Deny {
                                message: fb.or_else(|| Some("Please revise the plan.".into())),
                                interrupt: false,
                            },
                            window,
                            cx,
                        );
                    }
                }
            }
        })
        .detach();

        // Reactive wake loop: 0% idle CPU, wakes instantly on CLI events or history chunks.
        cx.spawn(async move |this, cx| {
            while wake_rx.recv().await.is_ok() {
                let alive = this.update(cx, |this, cx| this.drain_all(cx));
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();

        // Index workspace files off the UI thread for `@` fuzzy file completion.
        let ws_for_index = config.workspace.clone();
        cx.spawn(async move |this, cx| {
            let idx = cx.background_spawn(async move { FileIndex::build(&ws_for_index) }).await;
            this.update(cx, |this, cx| {
                this.file_index = idx;
                if matches!(this.completion, Some(CompletionPopup::AtFile { .. })) {
                    this.refresh_completion_popup(cx);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();

        // Preload cumulative USD cost for resumed Anthropic sessions.
        let resume_sid = config.resume_session_id.clone();
        let is_or = config.provider == agent_cli::CLAUDE_OPENROUTER;
        let cost_loaded = if !is_or && let Some(sid) = resume_sid.clone() {
            cx.spawn(async move |this, cx| {
                let loaded = cx.background_spawn(async move { costs::load(&sid) }).await;
                this.update(cx, |this, cx| {
                    this.cumulative_cost_usd = loaded.total_usd + this.process_cost_usd;
                    cx.notify();
                })
                .ok();
            })
            .detach();
            true
        } else {
            false
        };

        // Deterministic process & GPU image texture teardown when view entity is released.
        cx.on_release(|this, cx| {
            if let Some(c) = this.client.take() {
                c.close();
            }
            this.release_all_images(cx);
        })
        .detach();

        let list_state = ListState::new(0, ListAlignment::Bottom, gpui::px(320.));
        list_state.set_follow_mode(gpui::FollowMode::Tail);
        let mut view = Self {
            active_model: config.model.clone(),
            session_id: resume_sid,
            config,
            focus,
            client,
            history_rx,
            history_loading,
            loading_older_history: false,
            ready: false,
            exited: false,
            turn_active: false,
            state: AgentState::Idle,
            status_detail: if initial_error.is_some() { "Failed to start".into() } else { "Starting…".into() },
            title: None,
            init_info: None,
            effort: EffortLevel::Medium,
            permission_mode: PermissionMode::Default,
            fast_mode: false,
            context_used: 0,
            context_window: None,
            last_usage: TokenUsage::default(),
            process_cost_usd: 0.0,
            cumulative_cost_usd: 0.0,
            cost_loaded,
            rows: Vec::new(),
            items: Vec::new(),
            open_runs: HashSet::new(),
            expand_tools: false,
            list_state,
            column_width: Rc::new(Cell::new(0.)),
            stick_to_bottom: true,
            glide: kynex_core::motion::Glide::default(),
            last_frame: None,
            follow_after: false,
            turn_ended: false,
            last_offset: 0.,
            turn_started: None,
            clock_ticking: false,
            streaming_tool_json: HashMap::new(),
            copied_key: None,
            copied_gen: 0,
            plan_steps: Vec::new(),
            plan_expanded: false,
            subagents: Vec::new(),
            peek_subagent: None,
            pending_card: None,
            queued_cards: VecDeque::new(),
            queued: VecDeque::new(),
            prompt_suggestion: None,
            composer_input,
            card_input,
            find_input,
            find: FindState::default(),
            paste_fold: PasteFold::new(),
            file_index: FileIndex::default(),
            slash_commands: composer_popups::default_slash_commands(),
            completion: None,
            prompt_history: Default::default(),
            draft_gen: 0,
            last_esc: None,
            input_mode: Default::default(),
            last_text: String::new(),
            memory_menu: false,
            thinking_on: true,
            ultrathink: false,
            editing_outside: false,
            open_popover: None,
            attachments: Vec::new(),
            image_cache: media::ImageCache::default(),
            lightbox: None,
            next_seq: 1,
        };

        view.load_prompt_history(cx);
        if let Some(err) = initial_error {
            view.push_row(LogRow::Notice { text: err, is_error: true });
            view.exited = true;
        }
        view
    }

    // -----------------------------------------------------------------------
    // Public API for `src/app/claude_chat.rs`
    // -----------------------------------------------------------------------

    #[allow(dead_code)]
    pub fn status(&self) -> AgentState {
        self.state
    }

    #[allow(dead_code)]
    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    #[allow(dead_code)]
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    #[allow(dead_code)]
    pub fn provider(&self) -> &str {
        &self.config.provider
    }

    #[allow(dead_code)]
    pub fn model(&self) -> Option<&str> {
        self.active_model.as_deref()
    }

    pub fn send_external_prompt(&mut self, text: String, cx: &mut Context<Self>) {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return;
        }
        let blocks = vec![PromptBlock::Text(trimmed.to_string())];
        let chips = extract_at_chips(trimmed);
        if self.turn_active {
            let id = self.next_id();
            self.queued.push_back(QueuedMessage {
                id,
                display_text: trimmed.to_string(),
                blocks,
                file_chips: chips,
                paste_labels: Vec::new(),
                row_images: Vec::new(),
            });
            cx.notify();
        } else {
            self.send_blocks_now(trimmed.to_string(), blocks, chips, Vec::new(), Vec::new(), false, cx);
        }
    }

    pub fn cancel_turn(&mut self, cx: &mut Context<Self>) {
        if let Some(client) = &self.client {
            let _ = client.interrupt();
        }
        if self.turn_active {
            self.set_status(AgentState::Working, "Stopping…", cx);
        }
        cx.notify();
    }

    pub fn stop(&mut self, cx: &mut Context<Self>) {
        if let Some(c) = self.client.take() {
            c.close();
        }
        self.turn_active = false;
        self.exited = true;
        self.pending_card = None;
        self.queued_cards.clear();
        self.settle_subagents(false);
        self.set_status(AgentState::Idle, "Stopped", cx);
        cx.notify();
    }

    pub fn toggle_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.find.open {
            self.close_find(window, cx);
        } else {
            self.find.open = true;
            self.find_input.read(cx).focus_handle(cx).focus(window, cx);
            cx.notify();
        }
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    pub(super) fn next_id(&mut self) -> u64 {
        let id = self.next_seq;
        self.next_seq += 1;
        id
    }

    pub(super) fn is_openrouter(&self) -> bool {
        self.config.provider == agent_cli::CLAUDE_OPENROUTER
    }

    pub(super) fn request_fork(&mut self, cx: &mut Context<Self>) {
        if let Some(sid) = self.session_id.clone() {
            cx.emit(ClaudeChatEvent::ForkRequested { from_session_id: sid });
        } else {
            self.push_notice("Send a message first before forking this session.", false, cx);
        }
    }

    pub(super) fn clear_transcript(&mut self, cx: &mut Context<Self>) {
        self.rows.clear();
        self.open_runs.clear();
        self.relayout();
        self.plan_steps.clear();
        self.subagents.clear();
        self.peek_subagent = None;
        self.close_lightbox(cx);
        self.image_cache.release_all(cx);
        cx.notify();
    }

    pub(super) fn push_notice(&mut self, text: impl Into<String>, is_error: bool, cx: &mut Context<Self>) {
        self.push_row(LogRow::Notice { text: text.into(), is_error });
        cx.notify();
    }

    pub(super) fn mark_copied(&mut self, key: String, cx: &mut Context<Self>) {
        self.copied_gen += 1;
        let g = self.copied_gen;
        self.copied_key = Some(key);
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(1600)).await;
            this.update(cx, |this, cx| {
                if this.copied_gen == g {
                    this.copied_key = None;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    // -----------------------------------------------------------------------
    // Find & Turn navigation
    // -----------------------------------------------------------------------

    pub(super) fn recompute_find_matches(&mut self) {
        let q = self.find.query.trim().to_ascii_lowercase();
        if q.is_empty() {
            self.find.matches.clear();
            self.find.current = 0;
            return;
        }
        self.find.matches = self
            .rows
            .iter()
            .enumerate()
            .filter_map(|(i, row)| row.searchable_text().to_ascii_lowercase().contains(&q).then_some(i))
            .collect();
        if self.find.current >= self.find.matches.len() {
            self.find.current = 0;
        }
    }

    pub(super) fn step_find(&mut self, delta: isize, cx: &mut Context<Self>) {
        let len = self.find.matches.len();
        if len == 0 {
            return;
        }
        self.find.current = (self.find.current as isize + delta).rem_euclid(len as isize) as usize;
        if let Some(&ix) = self.find.matches.get(self.find.current) {
            self.reveal_row(ix);
        }
        cx.notify();
    }

    pub(super) fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.find.open = false;
        self.find.matches.clear();
        self.composer_input.read(cx).focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = theme(cx).clone();
        let lamp_state = match self.state {
            AgentState::Idle => Lamp::Idle,
            AgentState::Working => Lamp::Working,
            AgentState::NeedsYou => Lamp::NeedsYou,
            AgentState::Done => Lamp::Done,
        };
        let title = self.title.clone().unwrap_or_else(|| {
            if self.is_openrouter() { "Claude Code (OpenRouter)".into() } else { "Claude Code".into() }
        });
        let can_fork = self.session_id.is_some();

        div()
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .h(px(32.))
            .px_3()
            .border_b_1()
            .border_color(t.border_variant)
            .bg(t.panel)
            .font_family(ui_font())
            .child(lamp("claude-hdr-lamp", lamp_state, cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_weight(FontWeight::MEDIUM)
                    .text_xs()
                    .text_color(t.text)
                    .child(title),
            )
            .when(!self.status_detail.is_empty(), |d| {
                d.child(
                    div()
                        .flex_none()
                        .max_w(px(180.))
                        .truncate()
                        .text_xs()
                        .text_color(t.text_muted)
                        .child(self.status_detail.clone()),
                )
            })
            .when(self.loading_older_history, |d| {
                d.child(
                    div()
                        .flex_none()
                        .font_family(mono())
                        .text_size(px(10.5))
                        .text_color(t.text_placeholder)
                        .child("Loading older…"),
                )
            })
            .when(can_fork, |d| {
                d.child(
                    ghost_button("claude-hdr-fork", cx)
                        .h(px(22.))
                        .px_1p5()
                        .text_xs()
                        .text_color(t.text_muted)
                        .tooltip(Tooltip::text("Fork session into a new pane (/fork)"))
                        .child(icon(Icon::GitBranch, t.text_muted).size(px(12.)))
                        .child("Fork")
                        .on_click(cx.listener(|this, _, _, cx| this.request_fork(cx))),
                )
            })
            .child(
                icon_button("claude-hdr-find", Icon::Search, cx)
                    .tooltip(Tooltip::with_meta("Find in transcript", "Ctrl+F"))
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_find(window, cx))),
            )
    }
}

/// Max width of the centered reading & composer column (matches Kynex's `Readable` measure).
pub(super) const CHAT_COLUMN_MAX_WIDTH: f32 = 820.0;

/// Centers `child` inside a bounded-width column that shrinks fluidly on narrow panes.
pub(super) fn centered_column(child: impl IntoElement) -> gpui::Div {
    div().w_full().flex().justify_center().child(div().w_full().max_w(px(CHAT_COLUMN_MAX_WIDTH)).min_w_0().child(child))
}

impl Render for ClaudeChatView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.image_cache.flush_retired(cx);
        // A peek whose track is gone would draw nothing yet still swallow Esc.
        if let Some(id) = &self.peek_subagent
            && !self.subagents.iter().any(|s| &s.tool_use_id == id)
        {
            self.peek_subagent = None;
        }
        let t = theme(cx).clone();

        let find_bar = self.render_find_bar(cx);
        // While a reply streams, its markdown is re-parsed a few times a second, not every frame.
        crate::markdown::set_streaming(self.turn_active);
        self.follow_frame(window);
        let transcript = self.render_transcript(cx);
        let status_line = self.render_status_line(cx);
        let subagent_strip = self.render_subagent_strip(cx);
        let plan_strip = self.render_plan_strip(cx);
        let pending_card = self.render_pending_card(cx);
        let queued_strip = self.render_queued_strip(cx);
        self.sync_composer_ghost(cx);
        let completion = self.render_completion_popup(cx);
        let mode_popup = self.render_mode_popup(cx);
        let popover = self.render_toolbar_popover(cx);
        let composer = self.render_composer(cx);
        let peek_modal = self.render_subagent_peek_modal(cx);
        let lightbox = self.render_lightbox_modal(cx);

        div()
            .id("claude-chat-view")
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(t.editor)
            .font_family(ui_font())
            .text_color(t.text)
            .track_focus(&self.focus)
            .capture_key_down(cx.listener(Self::on_capture_key_down))
            .on_key_down(cx.listener(Self::on_key_down))
            .child(self.render_header(cx))
            .children(find_bar)
            .child(transcript)
            .children(status_line)
            .children(subagent_strip)
            .children(plan_strip)
            .children(pending_card)
            .children(queued_strip)
            .children(completion)
            .children(mode_popup)
            .children(popover)
            .child(composer)
            .children(peek_modal)
            .children(lightbox)
    }
}
