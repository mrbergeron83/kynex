//! Background tasks (`run_in_background` commands, background agents,
//! Monitors): the strip of running ones over the composer, where Watch
//! streams a task's output into the terminal panel and Stop ends it; the row in the transcript
//! when one ends; and a toast when that happens in a session that isn't shown.

use std::path::{Path, PathBuf};

use crate::ui::px;
use gpui::{AnyElement, Context, FontWeight, Window, div, prelude::*};
use kynex_core::fmt;
use kynex_core::live::Key;
use kynex_core::session::Item;
use kynex_core::tasks::{self, Notice, Status};
use kynex_core::toasts::{Action, Toast};

use super::KynexApp;
use super::state::{Active, View};
use crate::theme::{Palette, theme};
use crate::ui::{Icon, Tooltip, ghost_button, icon};

impl KynexApp {
    /// Shows a task's output in the project's terminal; `follow` keeps streaming it.
    fn show_task_output(&mut self, path: &Path, follow: bool, window: &mut Window, cx: &mut Context<Self>) {
        // Checked here, not when the button draws, so no frame touches the disk.
        if !path.is_file() {
            self.toast(Toast::error(format!("The task's output is gone: {}", path.display())));
            return cx.notify();
        }
        let (kind, command) = tasks::output_command(path, follow);
        self.run_in_terminal(kind, command, window, cx);
    }

    /// After a poll: each task that ended in a session that isn't shown gets a
    /// toast; several in one session merge into one that counts them.
    pub(super) fn toast_new_tasks(&mut self, news: Vec<(Key, Notice)>) {
        let shown = self.active_key();
        for (key, n) in news.into_iter().filter(|(k, n)| Some(*k) != shown && n.status != Status::Event) {
            let session = self.session_title(key).unwrap_or_else(|| "another session".into());
            let failed = n.status == Status::Failed;
            let toast = if failed { Toast::error("") } else { Toast::success("") };
            let toast = self.toast(toast.detail(n.summary).session(key).action(Action::GoToSession(key)));
            let (count, what) = (toast.count, if failed { "failed" } else { "finished" });
            toast.title = match count {
                1 => format!("Background task {what} in {session}"),
                n => format!("{n} background tasks {what} in {session}"),
            };
        }
    }

    /// Goes to an open session, in whichever tab it's in.
    pub(super) fn go_to_session(&mut self, key: Key, window: &mut Window, cx: &mut Context<Self>) {
        let target = if self.chat.sessions.iter().any(|s| s.uid == key) {
            Some(Active::Chat)
        } else {
            self.code
                .iter()
                .find(|(_, l)| l.sessions.iter().any(|s| s.uid == key))
                .and_then(|(path, _)| self.saved.projects.iter().position(|p| &p.path == path))
                .map(Active::Project)
        };
        let Some(target) = target else { return };
        if let Active::Project(i) = target
            && let Some(p) = self.saved.projects.get_mut(i)
        {
            p.view = View::Code;
        }
        self.select(target, window, cx);
        self.show_open(|s| s.uid == key);
        self.focus_input(window, cx);
        cx.notify();
    }

    /// The running tasks over the composer, each with Watch when its output is known.
    pub(super) fn render_running_tasks(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let session = self.active_session()?;
        if !session.has_tasks() {
            return None;
        }
        let t = theme(cx).clone();
        let can_watch = self.active_project().is_some();
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
                            if let Some(s) = this.session_mut() {
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
                    .child(icon(Icon::Loader, t.accent).size_3().flex_none())
                    .child(div().flex_none().text_color(t.text_placeholder).child(what))
                    .child(div().flex_1().min_w_0().truncate().text_color(t.text_muted).child(task.description.clone()))
                    .child(div().flex_none().text_color(t.text_placeholder).child(detail))
                    .children(watch)
                    .child(stop)
            })
            .collect();
        Some(div().flex().flex_col().gap_1().children(rows).into_any_element())
    }

    /// A task that ended, where its notice reached Claude: status, what it was,
    /// how long it ran, and its output.
    pub(super) fn render_task_notice(&self, ix: usize, n: &Notice, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx).clone();
        let (which, color, label) = status_look(n.status, &t);
        let output = n.output_file.clone().filter(|_| self.active_project().is_some());
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

    /// An open session's name: its title, else its first message.
    pub(super) fn session_title(&self, key: Key) -> Option<String> {
        let s = self
            .chat
            .sessions
            .iter()
            .chain(self.code.values().flat_map(|l| l.sessions.iter()))
            .find(|s| s.uid == key)?;
        let first =
            s.items.iter().find_map(|i| if let Item::User { text, .. } = i { Some(text.as_str()) } else { None })?;
        let line = first.lines().find(|l| !l.trim().is_empty())?.trim();
        Some(format!("“{}”", line.chars().take(40).collect::<String>()))
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
