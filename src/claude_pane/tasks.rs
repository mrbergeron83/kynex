//! The subagents of the turn in progress, and background tasks (`run_in_background` commands, background agents,
//! Monitors): the strip of running ones over the composer, where Watch
//! streams a task's output into the terminal panel and Stop ends it; and the row in the
//! transcript when one ends.

use std::path::{Path, PathBuf};

use crate::ui::px;
use gpui::{AnyElement, Context, ElementId, FontWeight, Window, div, prelude::*};
use kynex_core::claude::session::Kind;
use kynex_core::claude::subagents::State;
use kynex_core::claude::tasks::{self, Notice, Status};
use kynex_core::fmt;
use kynex_core::toasts::Toast;

use super::ClaudePane;
use super::Key;
use crate::theme::{Palette, theme};
use crate::ui::{Icon, Tooltip, ghost_button, icon, spinner};

/// The most subagent rows the strip shows; the rest are counted.
const AGENT_ROWS: usize = 6;

impl ClaudePane {
    /// Shows a task's output in the project's terminal; `follow` keeps streaming it.
    fn show_task_output(&mut self, path: &Path, follow: bool, window: &mut Window, cx: &mut Context<Self>) {
        // Checked here, not when the button draws, so no frame touches the disk.
        if !path.is_file() {
            self.toast(Toast::error(format!("The task's output is gone: {}", path.display())), cx);
            return cx.notify();
        }
        let (kind, command) = tasks::output_command(path, follow);
        self.run_in_terminal(Some(kind), command, window, cx);
    }

    /// The running tasks over the composer, each with Watch when its output is known.
    pub(super) fn render_running_tasks(&mut self, key: Key, cx: &mut Context<Self>) -> Option<AnyElement> {
        let session = self.session_by_key(key)?;
        if !session.has_tasks() {
            return None;
        }
        let t = theme(cx).clone();
        let can_watch = self.session.kind == Kind::Code;
        let rows: Vec<_> = session
            .tasks
            .running
            .iter()
            .enumerate()
            .map(|(i, task)| {
                let what = match task.kind.as_str() {
                    "local_agent" | "remote_agent" => "Agent",
                    "monitor" => "Monitor",
                    _ => "Command",
                };
                let mut detail = fmt::duration(task.started.elapsed().as_millis() as u64);
                if let Some(tool) = &task.last_tool {
                    detail = format!("{detail} · {tool}");
                }
                let id = task.id.clone();
                let stop = if task.stopping {
                    div().flex_none().px_1p5().text_color(t.text_placeholder).child("Stopping…").into_any_element()
                } else {
                    ghost_button(("task-stop", i), cx)
                        .h(px(20.))
                        .text_xs()
                        .text_color(t.text_muted)
                        .child(icon(Icon::Stop, t.text_muted).size_3())
                        .child("Stop")
                        .tooltip(Tooltip::text(format!("Stop this {}", what.to_lowercase())))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(s) = this.session_for(key) {
                                s.stop_task(&id);
                            }
                            cx.notify();
                        }))
                        .into_any_element()
                };
                let watch = task.output_file.clone().filter(|_| can_watch).map(|path| {
                    ghost_button(("task-watch", i), cx)
                        .h(px(20.))
                        .text_xs()
                        .text_color(t.text_muted)
                        .child("Watch")
                        .tooltip(Tooltip::text("Stream its output in the terminal"))
                        .on_click(
                            cx.listener(move |this, _, window, cx| this.show_task_output(&path, true, window, cx)),
                        )
                });
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .pl_2()
                    .pr_1()
                    .h(px(24.))
                    .rounded_md()
                    .border_1()
                    .border_color(t.border_variant)
                    .bg(t.panel)
                    .text_xs()
                    // Keyed by the task, so it keeps turning as other rows come and go.
                    .child(div().flex_none().child(spinner(
                        ElementId::Name(format!("task-spin-{}", task.id).into()),
                        t.accent,
                        px(12.),
                    )))
                    .child(div().flex_none().text_color(t.text_placeholder).child(what))
                    .child(div().flex_1().min_w_0().truncate().text_color(t.text_muted).child(task.description.clone()))
                    .child(div().flex_none().text_color(t.text_placeholder).child(detail))
                    .children(watch)
                    .child(stop)
            })
            .collect();
        Some(div().flex().flex_col().gap_1().children(rows).into_any_element())
    }

    /// The subagents of the turn in progress: a line counting them, then one row
    /// each (working ones first, at most [`AGENT_ROWS`]) with its state, agent,
    /// task and time. Gone when the turn ends; the transcript keeps them.
    pub(super) fn render_turn_agents(&mut self, key: Key, cx: &mut Context<Self>) -> Option<AnyElement> {
        let session = self.session_by_key(key)?;
        if !session.busy {
            return None;
        }
        let mut subs = session.agents.of_turn(&session.items);
        if subs.is_empty() {
            return None;
        }
        let t = theme(cx).clone();
        let working = subs.iter().filter(|s| s.state == State::Working).count();
        let failed = subs.iter().filter(|s| s.state == State::Failed).count();
        let done = subs.len() - working - failed;
        let mut counts = vec![format!("{working} working"), format!("{done} done")];
        if failed > 0 {
            counts.push(format!("{failed} failed"));
        }
        // Working first, then the latest to finish.
        subs.sort_by_key(|s| s.state != State::Working);
        let hidden = subs.len().saturating_sub(AGENT_ROWS);
        let rows = subs.into_iter().take(AGENT_ROWS).map(|s| {
            // A working one spins, keyed by its call so it keeps turning as rows re-sort.
            let mark = match s.state {
                State::Working => spinner(ElementId::Name(format!("agent-spin-{}", s.id).into()), t.accent, px(12.))
                    .into_any_element(),
                State::Done => icon(Icon::Check, t.text_placeholder).size_3().into_any_element(),
                State::Failed => icon(Icon::Close, t.error).size_3().into_any_element(),
            };
            let label = match s.state {
                State::Working => "Working",
                State::Done => "Done",
                State::Failed => "Failed",
            };
            let time = s.elapsed.map(|d| fmt::duration(d.as_millis() as u64)).unwrap_or_default();
            div()
                .flex()
                .items_center()
                .gap_2()
                .px_2()
                .h(px(22.))
                .text_xs()
                .child(div().flex_none().child(mark))
                .child(div().flex_none().w(px(52.)).text_color(t.text_placeholder).child(label))
                .child(div().flex_none().text_color(t.text_muted).child(s.agent))
                .child(div().flex_1().min_w_0().truncate().text_color(t.text_placeholder).child(s.description))
                .child(div().flex_none().text_color(t.text_placeholder).child(time))
        });
        Some(
            div()
                .flex()
                .flex_col()
                .py_1()
                .rounded_md()
                .border_1()
                .border_color(t.border_variant)
                .bg(t.panel)
                .child(
                    div()
                        .px_2()
                        .pb_0p5()
                        .text_xs()
                        .text_color(t.text_muted)
                        .child(format!("Subagents · {}", counts.join(" · "))),
                )
                .children(rows)
                .when(hidden > 0, |d| {
                    d.child(
                        div().px_2().text_xs().text_color(t.text_placeholder).child(format!("and {hidden} more done")),
                    )
                })
                .into_any_element(),
        )
    }

    /// A task that ended, where its notice reached Claude: status, what it was,
    /// how long it ran, and its output.
    pub(super) fn render_task_notice(&self, ix: usize, n: &Notice, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx).clone();
        let (which, color, label) = status_look(n.status, &t);
        let output = n.output_file.clone().filter(|_| self.session.kind == Kind::Code);
        let open: Option<AnyElement> = output.map(|path: PathBuf| {
            let follow = n.status == Status::Event;
            ghost_button(("task-output", ix), cx)
                .h(px(20.))
                .flex_none()
                .text_xs()
                .text_color(t.text_muted)
                .child("Output")
                .tooltip(Tooltip::text("Show its output in the terminal"))
                .on_click(cx.listener(move |this, _, window, cx| this.show_task_output(&path, follow, window, cx)))
                .into_any_element()
        });
        div()
            .flex()
            .items_center()
            .gap_1p5()
            .px_1()
            .text_xs()
            .child(icon(which, color).size_3p5().flex_none())
            .child(div().flex_none().font_weight(FontWeight::MEDIUM).text_color(color).child(label))
            .child(div().flex_1().min_w_0().truncate().text_color(t.text_muted).child(n.summary.clone()))
            .children(n.duration_ms.map(|ms| div().flex_none().text_color(t.text_placeholder).child(fmt::duration(ms))))
            .children(open)
            .into_any_element()
    }
}

fn status_look(status: Status, t: &Palette) -> (Icon, gpui::Hsla, &'static str) {
    match status {
        Status::Completed => (Icon::Check, t.success, "Task done"),
        Status::Failed => (Icon::Close, t.error, "Task failed"),
        Status::Stopped => (Icon::Stop, t.text_muted, "Task stopped"),
        Status::Event => (Icon::Dot, t.info, "Monitor"),
    }
}
