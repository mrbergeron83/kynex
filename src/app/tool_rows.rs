//! Tool calls in the transcript, laid out by `kynex_core::transcript`: each
//! call is one row with its result folded in, and a run of calls sits under
//! one summary row ("Edited app.rs ×3, read 2 files") that opens to them.

use std::time::Duration;

use gpui::{AnyElement, Context, FontWeight, div, prelude::*, px};
use kynex_core::session::{Item, PermState, Session};
use kynex_core::transcript::{Call, Run, shows_result};
use kynex_core::{fmt, mcp};

use super::KynexApp;
use super::transcript::{code_box, diff_box, disclosure_row};
use crate::prompt;
use crate::theme::mono;
use crate::theme::theme;
use crate::ui::{Icon, Tooltip, icon, spinner};

impl KynexApp {
    /// A run of tool calls at transcript row `row`. While Claude is still in
    /// it (`live`), the latest call shows under the summary.
    pub(super) fn render_run(&self, run: &Run, row: usize, live: bool, cx: &mut Context<Self>) -> AnyElement {
        if let [call] = run.calls.as_slice() {
            return self.render_call(call, row, cx);
        }
        let t = theme(cx).clone();
        let Some(session) = self.active_session() else { return div().into_any_element() };
        let (key, busy) = (session.uid, session.busy);
        let first = run.first();
        let open = self.expand_tools || self.open_runs.contains(&(key, first));
        let pending = busy && run.calls.iter().any(|c| c.result.is_none());
        let failed = run
            .calls
            .iter()
            .filter(|c| {
                c.result.is_some_and(|r| matches!(session.items.get(r), Some(Item::ToolResult { is_error: true, .. })))
            })
            .count();
        let (add, del) = run
            .calls
            .iter()
            .filter_map(|c| match session.items.get(c.ix) {
                Some(Item::ToolUse { name, input, .. }) => prompt::diff_stat(name, &serde_json::from_str(input).ok()?),
                _ => None,
            })
            .fold((0, 0), |(a, d), (add, del)| (a + add, d + del));
        let toggle = cx.listener(move |this, _, _, cx| {
            let k = (key, first);
            if !this.open_runs.remove(&k) {
                this.open_runs.insert(k);
            }
            this.remeasure_row(key, row);
            cx.notify();
        });
        let head = disclosure_row(("run", first), open, &t)
            .child(if pending {
                spinner(("run-spin", first), t.accent, px(12.)).into_any_element()
            } else {
                icon(Icon::Terminal, t.text_muted).size_3().into_any_element()
            })
            .child(div().flex_1().min_w_0().truncate().text_sm().text_color(t.text).child(run.summary.clone()))
            .when(failed > 0, |d| {
                d.child(div().flex_none().text_xs().text_color(t.error).child(format!("{failed} failed")))
            })
            .when(add + del > 0, |d| d.child(stat(add, del, &t)))
            .on_click(toggle);
        let shown = if open { run.calls.len() } else { usize::from(live) };
        let hidden = &run.calls[..run.calls.len() - shown];
        // Pictures the folded calls returned (a screenshot, a `Read` of an image) still show.
        let images: Vec<_> = hidden
            .iter()
            .filter_map(|c| match session.items.get(c.result?) {
                Some(Item::ToolResult { images, .. }) => Some(images.iter().cloned()),
                _ => None,
            })
            .flatten()
            .collect();
        let images = (!images.is_empty())
            .then(|| div().pl_6().pt_1().child(self.thumbnails(("run-images", first).into(), &images, 160., cx)));
        let calls: Vec<AnyElement> =
            run.calls[run.calls.len() - shown..].iter().map(|c| self.render_call(c, row, cx)).collect();
        div()
            .flex()
            .flex_col()
            .child(head)
            .children(images)
            .child(div().pl_4().flex().flex_col().children(calls))
            .into_any_element()
    }

    /// While the shown session runs a call with a time limit, repaints once a
    /// second so its elapsed time counts up; stops when none is running.
    pub(super) fn tick_timed_calls(&mut self, cx: &mut Context<Self>) {
        if self.calls_ticking || !self.active_session().is_some_and(Session::timed_call_running) {
            return;
        }
        self.calls_ticking = true;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let going = this.update(cx, |this, cx| {
                    this.calls_ticking = this.active_session().is_some_and(Session::timed_call_running);
                    cx.notify();
                    this.calls_ticking
                });
                if !matches!(going, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    /// One tool call, with its result, at transcript row `row`. Collapsed it's
    /// one line, with a marker if it failed; open, it shows the diff or the
    /// input and output.
    fn render_call(&self, call: &Call, row: usize, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx).clone();
        let Some(session) = self.active_session() else { return div().into_any_element() };
        let key = session.uid;
        let ix = call.ix;
        let Some(Item::ToolUse { name, summary, input, timing, .. }) = session.items.get(ix) else {
            return div().into_any_element();
        };
        let result = call.result.and_then(|r| match session.items.get(r) {
            Some(Item::ToolResult { content, images, is_error, .. }) => Some((r, content, images, *is_error)),
            _ => None,
        });
        let running = session.busy && result.is_none();
        let failed = result.is_some_and(|(_, _, _, e)| e);
        let expanded = self.expand_tools || self.expanded.contains(&(key, ix));
        let parsed = serde_json::from_str::<serde_json::Value>(input).ok();
        // MCP tools by what they do, "Edge · Click", not `mcp__playwright-edge__browser_click`.
        let label =
            parsed.as_ref().and_then(|v| mcp::describe(name, v)).map_or_else(|| name.clone(), |(label, _)| label);
        // Claude's description of the call reads as prose; a bare command or path stays mono.
        let described =
            parsed.as_ref().is_some_and(|v| v["description"].as_str().is_some_and(|d| !d.trim().is_empty()));
        // File edits show how many lines they change, and open as a diff.
        let stat_lines = parsed.as_ref().and_then(|v| prompt::diff_stat(name, v));
        // A call with a time limit (Bash's `timeout`, in ms) says how long it may
        // run, and when watched live how long it has: "1m 12s / max 2m".
        let timeout = parsed.as_ref().and_then(|v| v["timeout"].as_u64()).filter(|&ms| ms > 0).map(|ms| {
            let max = fmt::limit(ms);
            let ran = match (running, timing.elapsed()) {
                (true, Some(d)) => Some(fmt::elapsed(d)),
                (false, Some(d)) if timing.took.is_some() => Some(fmt::duration(d.as_millis() as u64)),
                _ => None,
            };
            let tip = match (&ran, running) {
                (Some(r), true) => format!("Running for {r}; will run for at most {max}"),
                (Some(r), false) => format!("Ran for {r}; could have run for {max}"),
                (None, _) => format!("Will run for at most {max}"),
            };
            div()
                .id(("tool-timeout", ix))
                .flex_none()
                .text_xs()
                .text_color(t.text_placeholder)
                .child(match ran {
                    Some(r) => format!("{r} / max {max}"),
                    None => format!("max {max}"),
                })
                .tooltip(Tooltip::text(tip))
        });
        // The prompt it raised: waiting, or how it was answered.
        let permission = call.permission.and_then(|p| match session.items.get(p) {
            Some(Item::Permission { state, .. }) => Some(match state {
                PermState::Pending => ("waiting for you below".to_string(), t.warning),
                PermState::Allowed(n) => (n.clone(), t.text_placeholder),
                PermState::Denied(n) => (n.clone(), t.error),
            }),
            _ => None,
        });
        // A failure stays visible collapsed: a marker, with the error's first line.
        let error_mark = result.filter(|_| failed).map(|(_, content, _, _)| {
            let first: String = content.lines().next().unwrap_or("").chars().take(160).collect();
            div()
                .id(("tool-error", ix))
                .flex_none()
                .flex()
                .items_center()
                .gap_1()
                .text_xs()
                .text_color(t.error)
                .child(icon(Icon::Close, t.error).size_3())
                .child("failed")
                .tooltip(Tooltip::text(first))
        });
        let summary_el = div().flex_1().min_w_0().truncate().child(summary.clone());
        let summary_el = if described {
            summary_el.text_sm().text_color(t.text)
        } else {
            summary_el.font_family(mono()).text_xs().text_color(t.text_muted)
        };
        let toggle = cx.listener(move |this, _, _, cx| {
            let k = (key, ix);
            if !this.expanded.remove(&k) {
                this.expanded.insert(k);
            }
            this.remeasure_row(key, row);
            cx.notify();
        });
        let head = disclosure_row(("tool", ix), expanded, &t)
            .child(if running {
                spinner(("tool-spin", ix), t.accent, px(12.)).into_any_element()
            } else {
                icon(Icon::Terminal, t.text_muted).size_3().into_any_element()
            })
            .child(div().flex_none().text_xs().font_weight(FontWeight::MEDIUM).text_color(t.text_muted).child(label))
            .child(summary_el)
            .children(permission.map(|(note, color)| div().flex_none().text_xs().text_color(color).child(note)))
            .children(timeout)
            .children(error_mark)
            .children(stat_lines.filter(|&(add, del)| add + del > 0).map(|(add, del)| stat(add, del, &t)))
            .on_click(toggle);
        let mut body: Vec<AnyElement> = Vec::new();
        // Pictures it returned (a screenshot, a `Read` of an image) show even folded.
        if let Some((r, _, images, _)) = result.filter(|(_, _, images, _)| !images.is_empty()) {
            body.push(
                div()
                    .pl_6()
                    .pt_1()
                    .child(self.thumbnails(("result-images", r).into(), images, 160., cx))
                    .into_any_element(),
            );
        }
        if expanded {
            match parsed.filter(|_| stat_lines.is_some()) {
                Some(v) => {
                    body.push(diff_box(("tool-diff", ix).into(), prompt::preview(name, &v, &t), &t).into_any_element())
                }
                None => body.push(code_box(("tool-body", ix).into(), input.clone(), (key, ix), &t).into_any_element()),
            }
            if let Some((r, content, _, _)) = result.filter(|&(_, _, _, e)| shows_result(name, e)) {
                body.push(code_box(("result-body", r).into(), content.clone(), (key, r), &t).into_any_element());
            }
        }
        div().flex().flex_col().child(head).children(body).into_any_element()
    }
}

/// "+12 −3" in the diff colours.
fn stat(add: usize, del: usize, t: &crate::theme::Palette) -> impl IntoElement + use<> {
    div()
        .flex()
        .flex_none()
        .gap_1()
        .font_family(mono())
        .text_xs()
        .when(add > 0, |d| d.child(div().text_color(t.success).child(format!("+{add}"))))
        .when(del > 0, |d| d.child(div().text_color(t.error).child(format!("−{del}"))))
}
