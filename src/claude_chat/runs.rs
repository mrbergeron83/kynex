//! The transcript list's plumbing: rows stay flat in `rows`, and the drawn list
//! items come from `kynex_core::claude::runs::group`, which gathers consecutive
//! tool calls into one run under a summary ("Edited app.rs ×3, ran 2 commands").
//! A run opens to its calls; folded while Claude is still in it, the latest
//! call shows under the summary.

use std::ops::Range;
use std::path::Path;
use std::time::Instant;

use gpui::{AnyElement, Context, FollowMode, ListOffset, SharedString, Window, div, prelude::*};
use kynex_core::claude::runs::{self, ListItem, RowKind};

use super::state::*;
use super::tool_rows::{disclosure_row, stat};
use super::{CHAT_COLUMN_MAX_WIDTH, ClaudeChatView, centered_column};
use crate::theme::theme;
use crate::ui::{px, spinner};

impl ClaudeChatView {
    /// Regroups the rows into list items and tells the list what changed: the
    /// items from the first that differs on. Rows mostly grow at the end, so
    /// this is usually the last item or two.
    pub(super) fn relayout(&mut self) {
        let kinds: Vec<RowKind> = self.rows.iter().map(row_kind).collect();
        let items = runs::group(&kinds);
        let same = self.items.iter().zip(&items).take_while(|(a, b)| a == b).count();
        self.list_state.splice(same..self.items.len(), items.len() - same);
        self.items = items;
    }

    /// Like [`Self::relayout`] after rows were put in front of the old ones:
    /// the old items keep their measurements and the scroll position its row.
    pub(super) fn relayout_prepended(&mut self) {
        let kinds: Vec<RowKind> = self.rows.iter().map(row_kind).collect();
        let items = runs::group(&kinds);
        let added = items.len().saturating_sub(self.items.len());
        self.list_state.splice(0..0, added);
        // Where old meets new, a run may have joined across: measure that again.
        let seam = added.saturating_sub(1)..(added + 1).min(items.len());
        self.list_state.remeasure_items(seam);
        self.items = items;
    }

    /// The list item that draws row `row`.
    pub(super) fn item_of_row(&self, row: usize) -> Option<usize> {
        let ix = self.items.partition_point(|it| it.start() <= row).checked_sub(1)?;
        match &self.items[ix] {
            ListItem::Row(n) if *n == row => Some(ix),
            ListItem::Run(r) if r.contains(&row) => Some(ix),
            _ => None,
        }
    }

    /// Scrolls to row `row`, opening the run it's folded in.
    pub(super) fn reveal_row(&mut self, row: usize) {
        let Some(ix) = self.open_run_of(row) else { return };
        self.glide.stop();
        self.follow_after = false;
        self.list_state.pause_following_tail();
        self.stick_to_bottom = false;
        self.list_state.scroll_to_reveal_item(ix);
    }

    /// Opens the run row `row` is folded in, if any; the list item drawing it.
    pub(super) fn open_run_of(&mut self, row: usize) -> Option<usize> {
        let ix = self.item_of_row(row)?;
        if let ListItem::Run(r) = &self.items[ix]
            && let Some(id) = self.rows.get(r.start).and_then(LogRow::id)
            && self.open_runs.insert(id)
        {
            self.list_state.remeasure_items(ix..ix + 1);
        }
        Some(ix)
    }

    /// Adds a row at the end. Following the tail (or trailing a reply) keeps
    /// the end in view; scrolled up, the view stays put.
    pub(super) fn push_row(&mut self, row: LogRow) {
        self.rows.push(row);
        self.relayout();
    }

    /// Row `idx` changed size: measure the item that draws it again.
    pub(super) fn remeasure_row(&self, idx: usize) {
        if let Some(ix) = self.item_of_row(idx) {
            self.list_state.remeasure_items(ix..ix + 1);
        }
    }

    /// Jumps to the end and follows new output from there.
    pub(super) fn scroll_to_bottom(&mut self) {
        self.glide.stop();
        self.follow_after = false;
        self.stick_to_bottom = true;
        self.list_state.scroll_to_end();
        self.list_state.set_follow_mode(FollowMode::Tail);
    }

    /// How far down the list is scrolled, in pixels.
    fn scroll_offset(&self) -> f32 {
        -f32::from(self.list_state.scroll_px_offset_for_scrollbar().y)
    }

    fn max_scroll(&self) -> f32 {
        f32::from(self.list_state.max_offset_for_scrollbar().y)
    }

    /// Runs each frame the transcript draws: follows a streaming reply by
    /// gliding rather than snapping, and after a turn ends goes to its end.
    ///
    /// gpui's tail following pins the end on every layout, so text jumps up as
    /// the last row grows. Instead, while Claude works the view holds through
    /// layout and glides down by what was added; when the turn is over it
    /// glides the rest of the way and follows plainly again.
    pub(super) fn follow_frame(&mut self, window: &mut Window) {
        let busy = self.turn_active;
        // Scrolled up by hand while trailing: let go.
        if self.follow_after && self.scroll_offset() < self.last_offset - 1. {
            self.glide.stop();
            self.follow_after = false;
        }
        // A turn finished: its end is where to look, even if you'd scrolled up
        // mid-turn to read earlier output.
        if std::mem::take(&mut self.turn_ended) && !self.list_state.is_following_tail() && !self.follow_after {
            self.glide.stop();
            self.follow_after = true;
        }
        if (busy || self.follow_after) && !crate::ui::motion::reduced() {
            self.trail(busy);
        } else if self.follow_after && !self.glide.is_moving() {
            self.follow_after = false;
            self.list_state.scroll_to_end();
            self.list_state.set_follow_mode(FollowMode::Tail);
        }
        if self.glide.is_moving() {
            self.glide_frame();
            window.request_animation_frame();
        }
        self.last_offset = self.scroll_offset();
        self.stick_to_bottom = self.list_state.is_following_tail() || self.follow_after;
    }

    fn trail(&mut self, busy: bool) {
        if self.list_state.is_following_tail() {
            if !busy {
                self.follow_after = false;
                return;
            }
            // Swap the "past the end" anchor for the same spot as an offset,
            // which layout leaves alone.
            let max = self.list_state.max_offset_for_scrollbar().y;
            self.list_state.scroll_to(ListOffset { item_ix: 0, offset_in_item: gpui::px(0.) });
            self.list_state.scroll_by(max);
            self.follow_after = true;
        } else if !self.follow_after {
            return;
        }
        let gap = self.max_scroll() - self.scroll_offset() - self.glide.remaining();
        if gap > 0.5 {
            self.glide.push(gap);
        } else if !busy && !self.glide.is_moving() {
            self.follow_after = false;
            self.list_state.scroll_to_end();
            self.list_state.set_follow_mode(FollowMode::Tail);
        }
    }

    /// Moves the list this frame's share of the glide, stopping at either end.
    fn glide_frame(&mut self) {
        // A held scrollbar thumb has the list; any glide gives way to it.
        if self.list_state.is_scrollbar_dragging() {
            self.glide.stop();
            self.follow_after = false;
        }
        let now = Instant::now();
        let dt = self.last_frame.map_or(1. / 120., |t| now.duration_since(t).as_secs_f32());
        self.last_frame = Some(now);
        let step = self.glide.step(dt);
        let (offset, max) = (self.scroll_offset(), self.max_scroll());
        let moved = (offset + step).clamp(0., max.max(0.)) - offset;
        if moved != 0. {
            self.list_state.scroll_by(gpui::px(moved));
        }
        if moved != step {
            self.glide.stop();
        }
        if step > 0. && offset + moved >= max - 1. {
            self.glide.stop();
            self.follow_after = true;
        }
        if !self.glide.is_moving() {
            self.last_frame = None;
            if !self.turn_active && std::mem::take(&mut self.follow_after) {
                self.list_state.scroll_to_end();
                self.list_state.set_follow_mode(FollowMode::Tail);
            }
        }
    }

    pub(super) fn finalize_streaming_rows(&mut self) {
        let mut changed = Vec::new();
        for (idx, row) in self.rows.iter_mut().enumerate().rev().take(4) {
            match row {
                LogRow::Thought { streaming, started_at, duration_ms, .. } if *streaming => {
                    *streaming = false;
                    if let Some(start) = started_at.take() {
                        *duration_ms = Some(start.elapsed().as_millis() as u64);
                    }
                    changed.push(idx);
                }
                LogRow::Prose { streaming, .. } if *streaming => *streaming = false,
                _ => {}
            }
        }
        for idx in changed {
            self.remeasure_row(idx);
        }
    }

    /// Marks every still-running subagent finished; `keep_background` spares tracks
    /// with a task id, which report their own end through a task notification.
    pub(super) fn settle_subagents(&mut self, keep_background: bool) {
        for s in self.subagents.iter_mut().filter(|s| s.running) {
            if keep_background && s.task_id.is_some() {
                continue;
            }
            s.running = false;
            s.status = "Stopped".into();
        }
    }

    /// A list row holding `body` in the reading column. Compact rows (tool
    /// traffic) sit tight; messages get room. Wide rows start where prose
    /// starts and may grow to the right, for tool output and diffs.
    pub(super) fn column_row(&self, body: AnyElement, compact: bool, wide: bool) -> gpui::Div {
        let row = div().w_full().px(px(ROW_PADDING)).map(|d| if compact { d.py_0p5() } else { d.py_2() });
        let avail = self.column_width.get();
        if !wide || avail <= 0. {
            return row.child(centered_column(body));
        }
        let prose = avail.min(CHAT_COLUMN_MAX_WIDTH);
        let left = (avail - prose) / 2.;
        let width = (avail - left).min(CHAT_WIDE_MAX_WIDTH).max(prose);
        row.child(div().ml(px(left)).w(px(width)).min_w_0().child(body))
    }

    /// One quiet line under the transcript while Claude works, saying what's
    /// going on when nothing above shows it: "Starting Claude…", "Waiting for
    /// Claude… 3s", "Working… 5s". Text only, no spinner. Its room stays for the
    /// whole turn, so the transcript doesn't hop as steps start and end.
    pub(super) fn render_status_line(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let started = self.turn_started.filter(|_| self.turn_active)?;
        let t = theme(cx).clone();
        let secs = started.elapsed().as_secs();
        let label = if !self.ready {
            "Starting Claude…".to_string()
        } else if !self.turn_output_seen {
            format!("Waiting for Claude… {secs}s")
        } else {
            format!("Working… {secs}s")
        };
        Some(
            div()
                .flex_none()
                .w_full()
                .px(px(ROW_PADDING))
                .child(centered_column(
                    div()
                        .h(px(22.))
                        .flex()
                        .items_center()
                        .px_1()
                        .text_xs()
                        .italic()
                        .text_color(t.text_placeholder)
                        .child(label),
                ))
                .into_any_element(),
        )
    }

    /// While a turn runs, repaints once a second so clocks count up ("Thinking… 4s",
    /// the status line, a running call's time) even when no output comes.
    pub(super) fn tick_clocks(&mut self, cx: &mut Context<Self>) {
        if self.clock_ticking || !self.turn_active {
            return;
        }
        self.clock_ticking = true;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(std::time::Duration::from_secs(1)).await;
                let going = this.update(cx, |this, cx| {
                    this.clock_ticking = this.turn_active;
                    cx.notify();
                    this.clock_ticking
                });
                if !matches!(going, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    /// The transcript's mouse-selection group: this view's entity id.
    pub(super) fn selection_group(&self, cx: &Context<Self>) -> u64 {
        cx.entity_id().as_u64()
    }

    /// How wide code blocks, tables and tool output may grow (past the text's
    /// width, into the right margin), once the transcript has been measured.
    pub(super) fn wide_width(&self) -> Option<f32> {
        let avail = self.column_width.get();
        (avail > 0.).then(|| {
            let prose = avail.min(CHAT_COLUMN_MAX_WIDTH);
            let left = (avail - prose) / 2.;
            (avail - left).min(CHAT_WIDE_MAX_WIDTH).max(prose)
        })
    }

    /// Claude Code's Ctrl+O: every tool call in the transcript open, or all closed again.
    pub(super) fn toggle_expand_tools(&mut self, cx: &mut Context<Self>) {
        self.expand_tools = !self.expand_tools;
        self.list_state.remeasure();
        cx.notify();
    }

    /// Page Up / Page Down, Home / End, ↑ / ↓: scrolls the transcript, gliding
    /// like the wheel. A page keeps a little of the last one in view; reaching
    /// the end follows new output again.
    pub(super) fn scroll_keys(&mut self, by: Scroll, cx: &mut Context<Self>) {
        if by == Scroll::Bottom {
            let (offset, max) = (self.scroll_offset(), self.max_scroll());
            self.glide.stop();
            if crate::ui::motion::reduced() || max <= offset {
                self.scroll_to_bottom();
            } else {
                self.glide.push(max - offset);
                self.follow_after = true;
            }
            cx.notify();
            return;
        }
        let page = f32::from(self.list_state.viewport_bounds().size.height) - 2. * ARROW_STEP;
        let distance = match by {
            Scroll::Line(dir) => dir * ARROW_STEP,
            Scroll::Page(dir) => dir * page.max(ARROW_STEP),
            Scroll::Top | Scroll::Bottom => -self.scroll_offset(),
        };
        if distance < 0. {
            self.list_state.pause_following_tail();
        }
        self.follow_after = false;
        if crate::ui::motion::reduced() {
            self.glide.stop();
            if by == Scroll::Top {
                self.list_state.scroll_to(ListOffset { item_ix: 0, offset_in_item: gpui::px(0.) });
            } else {
                let (offset, max) = (self.scroll_offset(), self.max_scroll());
                self.list_state.scroll_by(gpui::px((offset + distance).clamp(0., max.max(0.)) - offset));
            }
        } else {
            self.glide.push(distance);
        }
        cx.notify();
    }

    /// Draws list item `ix`: a row, or a run under its summary.
    pub(super) fn render_item(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        match self.items.get(ix).cloned() {
            Some(ListItem::Row(row)) => self.render_row(row, window, cx),
            Some(ListItem::Run(range)) => {
                let live = ix + 1 == self.items.len() && self.turn_active;
                let start = range.start;
                let body = self.render_run(range, live, cx);
                let body = crate::ui::ease_height::ease_height(("run-ease", start), body).into_any_element();
                self.column_row(body, true, true).into_any_element()
            }
            None => div().into_any_element(),
        }
    }

    fn render_run(&mut self, range: Range<usize>, live: bool, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx).clone();
        let Some(first) = self.rows.get(range.start).and_then(LogRow::id) else { return div().into_any_element() };
        let rows = &self.rows[range.clone()];

        let mut calls: Vec<(&str, String)> = Vec::new();
        let (mut pending, mut failed, mut added, mut removed) = (false, 0usize, 0usize, 0usize);
        for row in rows {
            match row {
                LogRow::EditRow { tool_name, path, diff, done, is_error, .. } => {
                    pending |= !done;
                    failed += usize::from(*is_error);
                    added += diff.added;
                    removed += diff.removed;
                    calls.push((tool_name.as_str(), file_name(path)));
                }
                LogRow::ToolCall { name, summary, done, is_error, .. } => {
                    pending |= !done;
                    failed += usize::from(*is_error);
                    let target = match name.as_str() {
                        "Read" | "NotebookRead" => file_name(summary),
                        "Grep" | "Glob" => summary.clone(),
                        _ => String::new(),
                    };
                    calls.push((name.as_str(), target));
                }
                _ => {}
            }
        }
        let refs: Vec<(&str, &str)> = calls.iter().map(|(n, t)| (*n, t.as_str())).collect();
        let summary = runs::summary(&refs);
        let open = self.open_runs.contains(&first) || self.expand_tools;
        let pending = pending && self.turn_active;

        // The live step spins in its own row, shown under the summary; folded
        // and not live, the summary spins for the run.
        let head = disclosure_row(SharedString::from(format!("run-{first}")), open, &t)
            .when(pending && !open && !live, |d| d.child(spinner(("run-spin", first), t.accent, px(12.))))
            .child(div().flex_1().min_w_0().truncate().text_sm().text_color(t.text_muted).child(summary))
            .when(failed > 0, |d| {
                d.child(div().flex_none().text_xs().text_color(t.error).child(format!("{failed} failed")))
            })
            .when(added + removed > 0, |d| d.child(stat(added, removed, &t)))
            .on_click(cx.listener(move |this, _, _, cx| {
                if !this.open_runs.remove(&first) {
                    this.open_runs.insert(first);
                }
                if let Some(ix) = this.items.iter().position(|it| it.start() == range.start) {
                    this.list_state.remeasure_items(ix..ix + 1);
                }
                cx.notify();
            }));

        // Open, every row of the run; folded and live, just the latest call.
        let shown: Vec<usize> = if open {
            range.clone().collect()
        } else if live {
            range.clone().rev().find(|&r| row_kind(&self.rows[r]) == RowKind::Tool).into_iter().collect()
        } else {
            Vec::new()
        };
        let body: Vec<AnyElement> = shown
            .into_iter()
            .map(|r| {
                let body = self.render_row_body(r, cx);
                self.flash_rows(r..r + 1, div().child(body), &t)
            })
            .collect();

        div().flex().flex_col().child(head).child(div().pl_4().flex().flex_col().children(body)).into_any_element()
    }
}

/// Room either side of a transcript row.
pub(super) const ROW_PADDING: f32 = 24.;

/// How far ↑ / ↓ scroll the transcript, as a browser's arrow keys do.
const ARROW_STEP: f32 = 40.;

/// A keyboard scroll of the transcript; signs are -1 up, 1 down.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Scroll {
    Line(f32),
    Page(f32),
    Top,
    Bottom,
}

/// How far tool output and diffs may widen the column, from prose's left edge.
const CHAT_WIDE_MAX_WIDTH: f32 = 1040.0;

/// How row `row` groups in the list.
pub(super) fn row_kind(row: &LogRow) -> RowKind {
    match row {
        LogRow::EditRow { .. } | LogRow::ToolCall { .. } => RowKind::Tool,
        // A permission record stays with the call it was for.
        LogRow::Thought { .. } | LogRow::PermissionNote { .. } => RowKind::Thought,
        _ => RowKind::Other,
    }
}

fn file_name(path: &str) -> String {
    Path::new(path).file_name().map_or_else(|| path.to_string(), |f| f.to_string_lossy().into_owned())
}
