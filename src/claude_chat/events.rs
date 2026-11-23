//! Event reducer consuming `ClaudeEvent` batches from `ClaudeClient` and
//! progressive `history::LoadedChunk`s from disk.

use std::time::Instant;

use gpui::{AppContext, Context, ListOffset};
use kynex_core::agent_panes::AgentState;
use kynex_core::claude::composer::CommandItem;
use kynex_core::claude::diff::diff_from_tool_input;
use kynex_core::claude::history::{HistoryMeta, LoadedChunk, TodoItem};
use kynex_core::claude::markdown;
use kynex_core::claude::{ClaudeEvent, InitInfo, PermissionKind, PermissionMode, PermissionRequest, TurnResult, costs};
use serde_json::Value;

use super::ClaudeChatView;
use super::media;
use super::state::*;

impl ClaudeChatView {
    /// Drains all queued `ClaudeClient` events and progressive history chunks.
    pub(super) fn drain_all(&mut self, cx: &mut Context<Self>) {
        let mut changed = false;

        // Drain history chunks first so initial replay lands cleanly before live deltas.
        let mut chunks = Vec::new();
        if let Some(rx) = &self.history_rx {
            while let Ok(chunk) = rx.try_recv() {
                chunks.push(chunk);
            }
        }
        for chunk in chunks {
            self.apply_history_chunk(chunk, cx);
            changed = true;
        }

        // Drain live subprocess events.
        let events = self.client.as_ref().map(|c| c.drain_events()).unwrap_or_default();
        for ev in events {
            self.apply_event(ev, cx);
            changed = true;
        }

        if changed {
            if self.find.open && !self.find.query.is_empty() {
                self.recompute_find_matches();
            }
            cx.notify();
        }
    }

    pub(super) fn apply_event(&mut self, ev: ClaudeEvent, cx: &mut Context<Self>) {
        match ev {
            ClaudeEvent::Initialized(info) => self.on_initialized(info, cx),
            ClaudeEvent::SystemInit(sys) => {
                self.ultrathink = sys.ultrathink_available;
                if !sys.session_id.is_empty() {
                    self.adopt_session_id(sys.session_id, cx);
                }
                if let Some(model) = sys.model
                    && !model.is_empty()
                {
                    self.active_model = Some(model);
                }
                if let Some(mode) = sys.permission_mode {
                    self.permission_mode = mode;
                }
                self.ready = true;
            }
            ClaudeEvent::StatusUpdate { status, permission_mode } => {
                if let Some(mode) = permission_mode {
                    self.permission_mode = mode;
                }
                if let Some(st) = status
                    && !st.is_empty()
                    && self.turn_active
                {
                    self.set_status(AgentState::Working, st, cx);
                }
            }
            ClaudeEvent::CompactBoundary { trigger, pre_tokens } => {
                self.finalize_streaming_rows();
                let mut detail = String::from("Conversation compacted");
                if let Some(t) = trigger.filter(|s| !s.is_empty()) {
                    detail.push_str(&format!(" ({t})"));
                }
                if let Some(tok) = pre_tokens {
                    detail.push_str(&format!(" · {} tokens before", format_tokens(tok)));
                }
                self.push_row(LogRow::CompactionDivider { detail });
            }
            ClaudeEvent::UserReplayed { uuid, text } => {
                // Attach uuid to the last optimistic UserPrompt if it matches; otherwise skip noise.
                if let Some(LogRow::UserPrompt { uuid: slot, .. }) =
                    self.rows.iter_mut().rev().find(|r| matches!(r, LogRow::UserPrompt { uuid: None, .. }))
                {
                    *slot = Some(uuid);
                } else if !text.trim().is_empty() && !self.has_recent_user_text(&text) {
                    let id = self.next_id();
                    let file_chips = extract_at_chips(&text);
                    self.push_row(LogRow::UserPrompt {
                        id,
                        uuid: Some(uuid),
                        text,
                        images: Vec::new(),
                        file_chips,
                        paste_labels: Vec::new(),
                    });
                }
            }
            ClaudeEvent::ThoughtDelta(delta) => {
                if delta.is_empty() {
                    return;
                }
                self.set_status(AgentState::Working, "Thinking…", cx);
                if let Some(LogRow::Thought { text, streaming: true, .. }) = self.rows.last_mut() {
                    text.push_str(&delta);
                    let idx = self.rows.len() - 1;
                    self.remeasure_row(idx);
                } else {
                    self.finalize_streaming_rows();
                    let id = self.next_id();
                    self.push_row(LogRow::Thought {
                        id,
                        text: delta,
                        started_at: Some(Instant::now()),
                        duration_ms: None,
                        expanded: false,
                        streaming: true,
                    });
                }
            }
            ClaudeEvent::TextDelta(delta) => {
                if delta.is_empty() {
                    return;
                }
                self.set_status(AgentState::Working, "Responding…", cx);
                if let Some(LogRow::Prose { raw, blocks, plain, streaming: true, .. }) = self.rows.last_mut() {
                    raw.push_str(&delta);
                    *blocks = markdown::parse(raw);
                    *plain = markdown::extract_plain_text(blocks);
                    let idx = self.rows.len() - 1;
                    self.remeasure_row(idx);
                } else {
                    self.finalize_streaming_rows();
                    let blocks = markdown::parse(&delta);
                    let plain = markdown::extract_plain_text(&blocks);
                    let id = self.next_id();
                    self.push_row(LogRow::Prose { id, raw: delta, blocks, plain, streaming: true });
                }
            }
            ClaudeEvent::ToolInputDelta { id, name, partial_json } => {
                // Keep status live while tool JSON streams.
                if !name.is_empty() {
                    self.set_status(AgentState::Working, format!("Preparing {name}…"), cx);
                }
                if let Some(entry) = self.streaming_tool_json.get_mut(&id) {
                    entry.push_str(&partial_json);
                } else {
                    self.streaming_tool_json.insert(id, partial_json);
                }
            }
            ClaudeEvent::ToolCallStart { id, name, input } => {
                self.finalize_streaming_rows();
                self.set_status(AgentState::Working, format!("Running {name}…"), cx);
                self.upsert_tool_call(id, name, input, false);
            }
            ClaudeEvent::ToolCallUpdate { id, name, input } => {
                self.streaming_tool_json.remove(&id);
                self.upsert_tool_call(id, name, input, true);
            }
            ClaudeEvent::ToolResult { tool_use_id, content, is_error, images } => {
                self.apply_tool_result(&tool_use_id, content, is_error, images, cx);
            }
            ClaudeEvent::MessageUsage { model, usage } => {
                let ctx_tok = usage.context_input_tokens();
                if ctx_tok > 0 {
                    self.context_used = ctx_tok;
                }
                self.last_usage = usage;
                if let Some(m) = model
                    && !m.is_empty()
                {
                    self.active_model = Some(m);
                }
            }
            ClaudeEvent::SubagentEvent(sub) => {
                apply_subagent_msg(&self.config.workspace, &mut self.subagents, &sub.parent_tool_use_id, sub.message);
            }
            ClaudeEvent::TaskStarted { task_id, tool_use_id, description, task_type } => {
                let key = tool_use_id.unwrap_or_else(|| task_id.clone());
                let label = if !description.trim().is_empty() {
                    truncate_chars(description.trim(), 64)
                } else {
                    task_type.unwrap_or_else(|| "Subagent".into())
                };
                if let Some(track) = self.subagents.iter_mut().find(|s| s.tool_use_id == key) {
                    track.task_id = Some(task_id);
                    if track.label == "Subagent" {
                        track.label = label;
                    }
                    track.running = true;
                    track.status = "Running".into();
                } else {
                    self.subagents.push(SubagentTrack {
                        tool_use_id: key,
                        task_id: Some(task_id),
                        label,
                        last_tool: None,
                        summary: None,
                        running: true,
                        status: "Running".into(),
                        rows: Vec::new(),
                    });
                }
            }
            ClaudeEvent::TaskProgress(prog) => {
                let track = self.subagents.iter_mut().find(|s| {
                    s.task_id.as_deref() == Some(prog.task_id.as_str())
                        || prog.tool_use_id.as_deref().is_some_and(|id| s.tool_use_id == id)
                });
                if let Some(t) = track {
                    if let Some(tool) = prog.last_tool_name {
                        t.last_tool = Some(tool);
                    }
                    if let Some(sum) = prog.summary {
                        t.summary = Some(sum);
                    }
                    if let Some(desc) = prog.description
                        && t.label == "Subagent"
                    {
                        t.label = truncate_chars(&desc, 64);
                    }
                }
            }
            ClaudeEvent::TaskNotification { task_id, tool_use_id, status, summary } => {
                let track = self.subagents.iter_mut().find(|s| {
                    s.task_id.as_deref() == Some(task_id.as_str())
                        || tool_use_id.as_deref().is_some_and(|id| s.tool_use_id == id)
                });
                if let Some(t) = track {
                    t.running = false;
                    t.status = status;
                    if let Some(sum) = summary {
                        t.summary = Some(sum);
                    }
                }
            }
            ClaudeEvent::PermissionRequested(req) => self.on_permission_requested(req, cx),
            ClaudeEvent::PromptSuggestion(suggestion) => {
                let trimmed = suggestion.trim().to_string();
                self.prompt_suggestion = (!trimmed.is_empty()).then_some(trimmed);
            }
            ClaudeEvent::RateLimit(rl) => {
                let msg = rl.message.unwrap_or_else(|| {
                    let mut s = format!("Rate limit: {}", rl.status);
                    if let Some(r) = rl.resets_at {
                        s.push_str(&format!(" (resets {r})"));
                    }
                    s
                });
                self.push_row(LogRow::Notice { text: msg, is_error: false });
            }
            ClaudeEvent::McpStatus(servers) => {
                let summary = if servers.is_empty() {
                    "No MCP servers configured.".to_string()
                } else {
                    servers.iter().map(|s| format!("{}: {}", s.name, s.status)).collect::<Vec<_>>().join(" · ")
                };
                self.push_row(LogRow::Notice { text: format!("MCP · {summary}"), is_error: false });
            }
            ClaudeEvent::TurnFinished(result) => self.on_turn_finished(result, cx),
            ClaudeEvent::ControlAck { .. } => {}
            ClaudeEvent::ControlError { error, .. } => {
                self.push_row(LogRow::Notice { text: format!("Control error: {error}"), is_error: true });
            }
            ClaudeEvent::Error(err) => {
                self.push_row(LogRow::Notice { text: err, is_error: true });
            }
            ClaudeEvent::Exited { code, stderr } => {
                self.turn_active = false;
                self.exited = true;
                self.pending_card = None;
                self.queued_cards.clear();
                self.finalize_streaming_rows();
                self.settle_subagents(false);
                let msg = match code {
                    Some(0) | None if stderr.trim().is_empty() => "Claude process ended.".to_string(),
                    Some(c) if stderr.trim().is_empty() => format!("Claude exited with status {c}."),
                    Some(c) => format!("Claude exited ({c}): {}", truncate_chars(stderr.trim(), 240)),
                    None => format!("Claude stopped: {}", truncate_chars(stderr.trim(), 240)),
                };
                self.push_row(LogRow::Notice { text: msg, is_error: code.is_some_and(|c| c != 0) });
                self.set_status(AgentState::Idle, "Stopped", cx);
            }
        }
    }

    fn on_initialized(&mut self, info: InitInfo, cx: &mut Context<Self>) {
        if let Some(mode) = info.current_permission_mode {
            self.permission_mode = mode;
        }
        if let Some(fast) = &info.fast_mode_state {
            self.fast_mode = matches!(fast.as_str(), "on" | "enabled" | "true");
        }
        if self.active_model.is_none()
            && let Some(first) = info.models.first()
        {
            self.active_model = Some(first.value.clone());
        }
        // Build `/` slash command list: CLI commands + built-in Kynex `/fork` & `/clear`.
        let mut cmds: Vec<CommandItem> = info
            .commands
            .iter()
            .map(|c| CommandItem {
                name: c.name.trim_start_matches('/').to_string(),
                description: c.description.clone(),
                argument_hint: c.argument_hint.clone(),
                aliases: c.aliases.clone(),
            })
            .collect();
        if !cmds.iter().any(|c| c.name == "fork") {
            cmds.push(CommandItem {
                name: "fork".into(),
                description: "Fork this session into a new side-by-side pane".into(),
                argument_hint: None,
                aliases: vec!["branch".into()],
            });
        }
        if !cmds.iter().any(|c| c.name == "clear") {
            cmds.push(CommandItem {
                name: "clear".into(),
                description: "Clear the visible transcript".into(),
                argument_hint: None,
                aliases: vec!["reset".into()],
            });
        }
        self.slash_commands = cmds;
        self.init_info = Some(info);
        self.ready = true;
        // Start in Auto where the model allows it. Only from the plain default, so a
        // `defaultMode` the user set in their Claude settings still wins.
        if self.permission_mode == PermissionMode::Default && self.active_model_supports_auto() {
            self.set_permission_mode(PermissionMode::Auto, cx);
        }
        if !self.turn_active {
            self.set_status(AgentState::Idle, "Ready", cx);
        }
    }

    fn on_permission_requested(&mut self, req: PermissionRequest, cx: &mut Context<Self>) {
        let card = match req.kind {
            PermissionKind::ToolApproval {
                tool_name,
                input,
                permission_suggestions,
                decision_reason,
                blocked_path,
                description,
                ..
            } => {
                let diff_preview = diff_from_tool_input(&tool_name, &input);
                PendingCard::Permission {
                    req_id: req.request_id,
                    tool_name,
                    input,
                    suggestions: permission_suggestions,
                    why: decision_reason,
                    blocked_path,
                    description,
                    diff_preview,
                    show_comment: false,
                }
            }
            PermissionKind::AskUserQuestion { questions, .. } => {
                let n = questions.len();
                PendingCard::AskUserQuestion {
                    req_id: req.request_id,
                    questions,
                    active_idx: 0,
                    selected: vec![Vec::new(); n],
                    other_active: vec![false; n],
                    other_text: vec![String::new(); n],
                }
            }
            PermissionKind::ExitPlanMode { plan_text, plan_path, .. } => {
                let text = plan_text.unwrap_or_else(|| "Claude is ready to exit plan mode.".into());
                let plan_blocks = markdown::parse(&text);
                PendingCard::ExitPlanMode { req_id: req.request_id, plan_blocks, plan_path, show_feedback: false }
            }
        };

        let detail = match &card {
            PendingCard::Permission { tool_name, .. } => format!("Approve {tool_name}?"),
            PendingCard::AskUserQuestion { .. } => "Question from Claude".into(),
            PendingCard::ExitPlanMode { .. } => "Review plan".into(),
        };

        if self.pending_card.is_none() {
            self.pending_card = Some(card);
        } else {
            self.queued_cards.push_back(card);
        }
        self.set_status(AgentState::NeedsYou, detail, cx);
    }

    fn on_turn_finished(&mut self, result: TurnResult, cx: &mut Context<Self>) {
        self.turn_active = false;
        self.turn_ended = true;
        self.finalize_streaming_rows();
        // Foreground subagents can't outlive their turn (after an interrupt no tool
        // result arrives); background tasks end with their own notification.
        self.settle_subagents(true);

        if let Some(sid) = result.session_id.clone().filter(|s| !s.is_empty()) {
            self.adopt_session_id(sid, cx);
        }
        if let Some(win) = result.context_window
            && win > 0
        {
            self.context_window = Some(win);
        }
        let ctx_tok = result.usage.context_input_tokens();
        if ctx_tok > 0 {
            self.context_used = ctx_tok;
        }
        if !result.usage.is_empty() {
            self.last_usage = result.usage;
        }

        // Persist incremental USD delta for Anthropic sessions so resumed sessions keep cumulative spend.
        let mut turn_cost = None;
        if !self.is_openrouter() && result.total_cost_usd.is_finite() {
            let delta = (result.total_cost_usd - self.process_cost_usd).max(0.0);
            self.process_cost_usd = result.total_cost_usd.max(self.process_cost_usd);
            self.cumulative_cost_usd += delta;
            turn_cost = Some(delta);
            if let Some(sid) = self.session_id.clone() {
                cx.background_spawn(async move {
                    let _ = costs::add_delta(&sid, delta);
                })
                .detach();
            }
        }

        if result.is_error {
            let msg = if !result.errors.is_empty() {
                result.errors.join("\n")
            } else {
                result.result_text.unwrap_or_else(|| "Turn failed.".into())
            };
            self.push_row(LogRow::Notice { text: msg, is_error: true });
        }
        self.push_turn_end(result.duration_ms, turn_cost, result.is_error);

        cx.emit(ClaudeChatEvent::TurnFinished);

        // If user queued messages while Claude was busy, dispatch the next one immediately.
        if let Some(next) = self.take_queue_batch() {
            self.dispatch_queued(next, cx);
        } else {
            self.set_status(AgentState::Done, "Done", cx);
        }
    }

    /// Ends the turn in the transcript with its recap: tools used since the last
    /// message sent, and the files edited.
    fn push_turn_end(&mut self, duration_ms: u64, cost_usd: Option<f64>, is_error: bool) {
        let from = self.rows.iter().rposition(LogRow::is_user_turn).map_or(0, |i| i + 1);
        let mut tools = 0;
        let mut files: Vec<String> = Vec::new();
        for row in &self.rows[from..] {
            match row {
                LogRow::ToolCall { .. } => tools += 1,
                LogRow::EditRow { path, is_error: false, .. } => {
                    tools += 1;
                    if !files.contains(path) {
                        files.push(path.clone());
                    }
                }
                LogRow::EditRow { .. } => tools += 1,
                _ => {}
            }
        }
        self.turn_started = None;
        self.push_row(LogRow::TurnEnd { duration_ms, tools, cost_usd, is_error, files });
    }

    pub(super) fn adopt_session_id(&mut self, sid: String, cx: &mut Context<Self>) {
        if self.session_id.as_deref() == Some(sid.as_str()) {
            return;
        }
        self.session_id = Some(sid.clone());
        cx.emit(ClaudeChatEvent::SessionId(sid.clone()));
        if !self.is_openrouter() && !self.cost_loaded {
            self.cost_loaded = true;
            cx.spawn(async move |this, cx| {
                let loaded = cx.background_spawn(async move { costs::load(&sid) }).await;
                this.update(cx, |this, cx| {
                    this.cumulative_cost_usd = loaded.total_usd + this.process_cost_usd;
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
    }

    fn upsert_tool_call(&mut self, tool_use_id: String, name: String, input: Value, is_update: bool) {
        if is_todo_tool(&name) {
            apply_todo_input(&mut self.plan_steps, &name, &input);
        }
        if is_subagent_tool(&name) {
            let label = input
                .get("description")
                .or_else(|| input.get("subagent_type"))
                .or_else(|| input.get("prompt"))
                .and_then(Value::as_str)
                .map(|s| truncate_chars(s.lines().next().unwrap_or(s).trim(), 64))
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "Subagent".into());
            if let Some(t) = self.subagents.iter_mut().find(|s| s.tool_use_id == tool_use_id) {
                t.label = label;
            } else {
                self.subagents.push(SubagentTrack {
                    tool_use_id: tool_use_id.clone(),
                    task_id: None,
                    label,
                    last_tool: None,
                    summary: None,
                    running: true,
                    status: "Running".into(),
                    rows: Vec::new(),
                });
            }
        }

        // 2. Edit / MultiEdit / Write compact 1-line rows with collapsible inline diff
        if is_edit_tool(&name) {
            let path = extract_file_path(&input).unwrap_or_else(|| name.clone());
            let diff = diff_from_tool_input(&name, &input).unwrap_or_default();
            for (idx, row) in self.rows.iter_mut().enumerate().rev() {
                if let LogRow::EditRow { tool_use_id: tid, path: p, diff: d, .. } = row
                    && *tid == tool_use_id
                {
                    if !path.is_empty() {
                        *p = path;
                    }
                    if !diff.is_empty() || is_update {
                        *d = diff;
                    }
                    self.remeasure_row(idx);
                    return;
                }
            }
            let id = self.next_id();
            self.push_row(LogRow::EditRow {
                id,
                tool_use_id,
                tool_name: name,
                path,
                diff,
                expanded: false,
                done: false,
                is_error: false,
                error_text: None,
            });
            return;
        }

        // 3. Skip separate noisy ToolCall row for pure TodoWrite bookkeeping (shown in Plan strip).
        if is_todo_tool(&name) {
            return;
        }

        // 4. Bash / PowerShell / MCP / generic tools
        let summary = tool_summary(&self.config.workspace, &name, &input);
        let command = extract_command(&input);
        let input_preview = format_input_preview(&input);
        for (idx, row) in self.rows.iter_mut().enumerate().rev() {
            if let LogRow::ToolCall { tool_use_id: tid, summary: s, command: c, input_preview: ip, .. } = row
                && *tid == tool_use_id
            {
                *s = summary;
                if command.is_some() {
                    *c = command;
                }
                if !input_preview.is_empty() {
                    *ip = input_preview;
                }
                self.remeasure_row(idx);
                return;
            }
        }
        let id = self.next_id();
        self.push_row(LogRow::ToolCall {
            id,
            tool_use_id,
            name,
            summary,
            command,
            input_preview,
            output: String::new(),
            images: Vec::new(),
            started_at: Some(Instant::now()),
            duration_ms: None,
            done: false,
            is_error: false,
            expanded: false,
        });
    }

    fn apply_tool_result(
        &mut self,
        tool_use_id: &str,
        content: String,
        is_error: bool,
        images: Vec<kynex_core::claude::ToolResultImage>,
        cx: &mut Context<Self>,
    ) {
        // Mark subagent complete if this result belongs to an Agent/Task call.
        if let Some(sub) = self.subagents.iter_mut().find(|s| s.tool_use_id == tool_use_id) {
            sub.running = false;
            sub.status = if is_error { "Failed".into() } else { "Done".into() };
            if sub.summary.is_none() && !content.trim().is_empty() {
                sub.summary = Some(truncate_chars(content.trim(), 160));
            }
        }

        // Decode any inline tool-result images off the UI thread.
        let mut row_images = Vec::new();
        for (idx, img) in images.into_iter().enumerate() {
            let img_id = self.next_id();
            let label = format!("Tool image #{}", idx + 1);
            row_images.push(RowImage { key: ImageKey::Inline(img_id), logged_ref: None, label: label.clone() });
            let b64 = img.base64;
            cx.spawn(async move |this, cx| {
                let decoded = cx.background_spawn(async move { media::decode_inline_base64(&b64, label) }).await;
                if let Some(cached) = decoded {
                    this.update(cx, |this, cx| {
                        this.image_cache.insert_inline(img_id, cached);
                        cx.notify();
                    })
                    .ok();
                }
            })
            .detach();
        }

        for (idx, row) in self.rows.iter_mut().enumerate().rev() {
            match row {
                LogRow::EditRow { tool_use_id: tid, done, is_error: err, error_text, .. } if tid == tool_use_id => {
                    *done = true;
                    *err = is_error;
                    if is_error && !content.trim().is_empty() {
                        *error_text = Some(truncate_chars(content.trim(), 240));
                    }
                    self.remeasure_row(idx);
                    return;
                }
                LogRow::ToolCall {
                    tool_use_id: tid,
                    output,
                    images: imgs,
                    started_at,
                    duration_ms,
                    done,
                    is_error: err,
                    ..
                } if tid == tool_use_id => {
                    *output = content;
                    imgs.extend(row_images);
                    *done = true;
                    *err = is_error;
                    if let Some(start) = started_at.take() {
                        *duration_ms = Some(start.elapsed().as_millis() as u64);
                    }
                    self.remeasure_row(idx);
                    return;
                }
                _ => {}
            }
        }
    }

    pub(super) fn apply_history_chunk(&mut self, chunk: LoadedChunk, cx: &mut Context<Self>) {
        match chunk {
            LoadedChunk::Recent { items, meta, has_older } => {
                self.apply_history_meta(meta);
                self.prepend_history_items(items, true, cx);
                self.loading_older_history = has_older;
            }
            LoadedChunk::Older { items } => {
                self.prepend_history_items(items, false, cx);
                self.loading_older_history = false;
            }
            LoadedChunk::Done => {
                self.history_loading = false;
                self.loading_older_history = false;
                self.history_rx = None;
            }
        }
    }

    fn prepend_history_items(
        &mut self,
        items: Vec<kynex_core::claude::history::HistoryItem>,
        is_recent: bool,
        cx: &mut Context<Self>,
    ) {
        let mut rows = Vec::new();
        let mut seq = self.next_seq;
        history_items_to_rows(&self.config.workspace, items, &mut rows, &mut self.subagents, &mut || {
            let id = seq;
            seq += 1;
            id
        });
        self.next_seq = seq;
        if rows.is_empty() {
            return;
        }
        let old_len = self.rows.len();
        let old_items = self.items.len();
        let top = self.list_state.logical_scroll_top();
        let existing = std::mem::take(&mut self.rows);
        self.rows = rows;
        self.rows.extend(existing);
        self.relayout_prepended();
        let added = self.items.len().saturating_sub(old_items);
        if is_recent && old_len == 0 {
            self.scroll_to_bottom();
        } else if !is_recent && !self.stick_to_bottom {
            self.list_state.scroll_to(ListOffset { item_ix: top.item_ix + added, offset_in_item: top.offset_in_item });
        }
        self.update_title_from_first_prompt(cx);
    }

    fn apply_history_meta(&mut self, meta: HistoryMeta) {
        if meta.context_tokens > 0 && self.context_used == 0 {
            self.context_used = meta.context_tokens;
        }
        if self.active_model.is_none() {
            self.active_model = meta.model;
        }
        if !meta.latest_todos.is_empty() && self.plan_steps.is_empty() {
            self.plan_steps = meta
                .latest_todos
                .into_iter()
                .map(|t: TodoItem| PlanStep { content: t.content, status: t.status, active_form: t.active_form })
                .collect();
        }
    }

    pub(super) fn set_status(&mut self, state: AgentState, detail: impl Into<String>, cx: &mut Context<Self>) {
        let detail = detail.into();
        if self.state != state || self.status_detail != detail {
            self.state = state;
            self.status_detail = detail.clone();
            cx.emit(ClaudeChatEvent::StatusChanged { state, detail });
        }
    }

    pub(super) fn update_title_from_first_prompt(&mut self, cx: &mut Context<Self>) {
        if self.title.is_some() {
            return;
        }
        let first = self.rows.iter().find_map(|r| match r {
            LogRow::UserPrompt { text, .. } if !text.trim().is_empty() => Some(text.trim()),
            _ => None,
        });
        if let Some(line) = first.and_then(|t| t.lines().next()) {
            let title = truncate_chars(line.trim(), 60);
            if !title.is_empty() {
                self.title = Some(title.clone());
                cx.emit(ClaudeChatEvent::TitleChanged(title));
            }
        }
    }

    fn has_recent_user_text(&self, text: &str) -> bool {
        let trimmed = text.trim();
        self.rows.iter().rev().take(4).any(|r| matches!(r, LogRow::UserPrompt { text: t, .. } if t.trim() == trimmed))
    }
}
