//! The transcript list's plumbing: rows stay flat in `rows`, and the drawn list
//! items come from `kynex_core::claude::runs::group`, which gathers consecutive
//! tool calls into one run under a summary ("Edited app.rs ×3, ran 2 commands").
//! A run opens to its calls; folded while Claude is still in it, the latest
//! call shows under the summary.

use std::ops::Range;
use std::path::Path;

use gpui::{AnyElement, Context, FontWeight, SharedString, Window, div, prelude::*};
use kynex_core::claude::runs::{self, ListItem, RowKind};

use super::state::*;
use super::{CHAT_COLUMN_MAX_WIDTH, ClaudeChatView, centered_column};
use crate::theme::{mono, theme};
use crate::ui::{Icon, icon, px, spinner};

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
        let Some(ix) = self.item_of_row(row) else { return };
        if let ListItem::Run(r) = &self.items[ix]
            && let Some(id) = self.rows.get(r.start).and_then(LogRow::id)
            && self.open_runs.insert(id)
        {
            self.list_state.remeasure_items(ix..ix + 1);
        }
        self.stick_to_bottom = false;
        self.list_state.scroll_to_reveal_item(ix);
    }

    pub(super) fn push_row(&mut self, row: LogRow) {
        let was_at_bottom = self.stick_to_bottom || self.list_state.is_scrolled_to_end().unwrap_or(true);
        self.rows.push(row);
        self.relayout();
        if was_at_bottom {
            self.stick_to_bottom = true;
            self.list_state.scroll_to_end();
        }
    }

    /// Row `idx` changed size: measure the item that draws it again.
    pub(super) fn remeasure_row(&self, idx: usize) {
        if let Some(ix) = self.item_of_row(idx) {
            self.list_state.remeasure_items(ix..ix + 1);
            if self.stick_to_bottom {
                self.list_state.scroll_to_end();
            }
        }
    }

    pub(super) fn scroll_to_bottom(&mut self) {
        self.stick_to_bottom = true;
        self.list_state.scroll_to_end();
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
        let row = div().w_full().px_4().map(|d| if compact { d.py_0p5() } else { d.py_2() });
        let avail = self.column_width.get();
        if !wide || avail <= 0. {
            return row.child(centered_column(body));
        }
        let prose = avail.min(CHAT_COLUMN_MAX_WIDTH);
        let left = (avail - prose) / 2.;
        let width = (avail - left).min(CHAT_WIDE_MAX_WIDTH).max(prose);
        row.child(div().ml(px(left)).w(px(width)).min_w_0().child(body))
    }

    /// One quiet line under the transcript while Claude works: what it's doing
    /// and for how long ("Thinking… 5s"). Shown for the whole turn, so the
    /// transcript doesn't hop as steps start and end.
    pub(super) fn render_status_line(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let started = self.turn_started.filter(|_| self.turn_active)?;
        let t = theme(cx).clone();
        let detail = if self.status_detail.is_empty() { "Working…" } else { self.status_detail.as_str() };
        let label = format!("{detail} {}s", started.elapsed().as_secs());
        Some(
            div()
                .flex_none()
                .w_full()
                .px_4()
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

    /// Draws list item `ix`: a row, or a run under its summary.
    pub(super) fn render_item(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        match self.items.get(ix).cloned() {
            Some(ListItem::Row(row)) => self.render_row(row, window, cx),
            Some(ListItem::Run(range)) => {
                let live = ix + 1 == self.items.len() && self.turn_active;
                let body = self.render_run(range, live, cx);
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
                LogRow::ReadSearchRun { entries, .. } => {
                    for e in entries {
                        pending |= !e.done;
                        failed += usize::from(e.is_error);
                        let target = match e.kind {
                            ReadKind::Read => e.path.as_deref().map(file_name).unwrap_or_default(),
                            _ => e.label.clone(),
                        };
                        calls.push((e.tool_name.as_str(), target));
                    }
                }
                LogRow::EditRow { tool_name, path, diff, done, is_error, .. } => {
                    pending |= !done;
                    failed += usize::from(*is_error);
                    added += diff.added;
                    removed += diff.removed;
                    calls.push((tool_name.as_str(), file_name(path)));
                }
                LogRow::ToolCall { name, done, is_error, .. } => {
                    pending |= !done;
                    failed += usize::from(*is_error);
                    calls.push((name.as_str(), String::new()));
                }
                _ => {}
            }
        }
        let refs: Vec<(&str, &str)> = calls.iter().map(|(n, t)| (*n, t.as_str())).collect();
        let summary = runs::summary(&refs);
        let open = self.open_runs.contains(&first);
        let chev = if open { Icon::ChevronDown } else { Icon::ChevronRight };

        let head = div()
            .id(SharedString::from(format!("run-hdr-{first}")))
            .flex()
            .items_center()
            .gap_1p5()
            .h(px(24.))
            .cursor_pointer()
            .text_xs()
            .text_color(t.text_muted)
            .hover(|s| s.text_color(t.text))
            // Folded and not live, the summary spins for the run; live, the latest
            // call spins in its own row under it.
            .when(pending && !open && !live, |d| d.child(spinner(("run-spin", first), t.accent, px(12.))))
            .child(icon(chev, t.text_placeholder).size(px(12.)))
            .child(div().flex_1().min_w_0().truncate().child(summary))
            .when(failed > 0, |d| d.child(div().flex_none().text_color(t.error).child(format!("{failed} failed"))))
            .when(added > 0, |d| {
                d.child(div().flex_none().font_family(mono()).text_color(t.success).child(format!("+{added}")))
            })
            .when(removed > 0, |d| {
                d.child(div().flex_none().font_family(mono()).text_color(t.error).child(format!("-{removed}")))
            })
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
        let body: Vec<AnyElement> = shown.into_iter().map(|r| self.render_row_body(r, cx)).collect();

        div()
            .w_full()
            .flex()
            .flex_col()
            .gap_1()
            .child(head)
            .when(!body.is_empty(), |d| {
                d.child(
                    div()
                        .ml(px(5.))
                        .pl_3()
                        .border_l_1()
                        .border_color(t.border_variant)
                        .flex()
                        .flex_col()
                        .gap_1()
                        .font_weight(FontWeight::NORMAL)
                        .children(body),
                )
            })
            .into_any_element()
    }
}

/// How far tool output and diffs may widen the column, from prose's left edge.
const CHAT_WIDE_MAX_WIDTH: f32 = 1040.0;

/// How row `row` groups in the list.
pub(super) fn row_kind(row: &LogRow) -> RowKind {
    match row {
        LogRow::ReadSearchRun { .. } | LogRow::EditRow { .. } | LogRow::ToolCall { .. } => RowKind::Tool,
        // A permission record stays with the call it was for.
        LogRow::Thought { .. } | LogRow::PermissionNote { .. } => RowKind::Thought,
        _ => RowKind::Other,
    }
}

fn file_name(path: &str) -> String {
    Path::new(path).file_name().map_or_else(|| path.to_string(), |f| f.to_string_lossy().into_owned())
}
