//! Rewind, Claude Code's Esc Esc on an empty composer: pick an earlier message,
//! then what to restore: the conversation, the files Claude changed since, or
//! both. The popup sits above the composer; `kynex_core::claude::rewind` reads
//! the points. Files come back through the live client (`rewind_files`); the
//! conversation by restarting the CLI at the message (`--resume-session-at`).

use std::sync::Arc;

use gpui::{AnyElement, AppContext, Context, FontWeight, SharedString, div, prelude::*};
use kynex_core::claude::rewind::{self, Point};
use kynex_core::claude::{ClaudeClient, SessionStart, SpawnConfig};

use super::state::LogRow;
use super::{ClaudeChatView, centered_column};
use crate::theme::{theme, ui_font};
use crate::ui::{popover, px};

/// The open rewind popup.
pub(crate) struct RewindMenu {
    /// Newest first.
    points: Vec<Point>,
    selected: usize,
    /// The message picked, while choosing what to restore.
    picked: Option<usize>,
    /// Its messages are still being read.
    loading: bool,
}

/// A file restore sent to the CLI, waiting for its answer.
pub(crate) struct PendingRewind {
    pub request_id: String,
    point: Point,
    conversation: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum Restore {
    Both,
    Conversation,
    Code,
}

impl Restore {
    fn label(self) -> &'static str {
        match self {
            Restore::Both => "Restore code and conversation",
            Restore::Conversation => "Restore conversation",
            Restore::Code => "Restore code",
        }
    }
}

/// What going back to `p` can restore: files only where the CLI kept a
/// checkpoint and something changed since.
fn choices(p: &Point) -> Vec<Restore> {
    if p.checkpoint && !p.changed.is_empty() {
        vec![Restore::Both, Restore::Conversation, Restore::Code]
    } else {
        vec![Restore::Conversation]
    }
}

fn files(n: usize) -> String {
    format!("{n} file{}", if n == 1 { "" } else { "s" })
}

impl ClaudeChatView {
    /// Opens the popup, once the session's messages are read.
    pub(super) fn open_rewind(&mut self, cx: &mut Context<Self>) {
        if self.turn_active {
            self.push_notice("Stop Claude or let it finish before rewinding.", false, cx);
            return;
        }
        let Some(id) = self.session_id.clone() else {
            self.push_notice("Nothing to rewind to yet.", false, cx);
            return;
        };
        if self.rewind.as_ref().is_some_and(|r| r.loading) {
            return;
        }
        self.rewind = Some(RewindMenu { points: Vec::new(), selected: 0, picked: None, loading: true });
        cx.notify();
        let cwd = self.config.workspace.clone();
        cx.spawn(async move |this, cx| {
            let points = cx.background_spawn(async move { rewind::points(&cwd, &id) }).await;
            this.update(cx, |this, cx| {
                if !this.rewind.as_ref().is_some_and(|r| r.loading) {
                    return;
                }
                this.rewind = None;
                match points {
                    Ok(p) if p.is_empty() => this.push_notice("Nothing to rewind to yet.", false, cx),
                    Ok(points) => {
                        this.rewind = Some(RewindMenu { points, selected: 0, picked: None, loading: false });
                    }
                    Err(e) => this.push_notice(e, true, cx),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The popup's keys: ↑ / ↓, Enter / Tab, Esc. True if it took the key.
    pub(super) fn rewind_key(&mut self, key: &str, cx: &mut Context<Self>) -> bool {
        let Some(r) = &mut self.rewind else { return false };
        match key {
            "up" | "down" => {
                let n = match r.picked {
                    Some(i) => choices(&r.points[i]).len(),
                    None => r.points.len(),
                };
                if n > 0 {
                    r.selected = if key == "down" { (r.selected + 1) % n } else { (r.selected + n - 1) % n };
                }
            }
            "enter" | "tab" => self.rewind_accept(cx),
            "escape" => match r.picked.take() {
                Some(i) => r.selected = i,
                None => self.rewind = None,
            },
            _ => return false,
        }
        cx.notify();
        true
    }

    /// Picks the message, then the choice.
    fn rewind_accept(&mut self, cx: &mut Context<Self>) {
        let Some(r) = &mut self.rewind else { return };
        if r.points.is_empty() {
            return;
        }
        match r.picked {
            None => {
                r.picked = Some(r.selected);
                r.selected = 0;
            }
            Some(i) => {
                let point = r.points[i].clone();
                let restore = choices(&point).get(r.selected).copied().unwrap_or(Restore::Conversation);
                self.rewind = None;
                self.restore(point, restore, cx);
            }
        }
    }

    fn restore(&mut self, point: Point, restore: Restore, cx: &mut Context<Self>) {
        if restore == Restore::Conversation {
            return self.rewind_conversation(&point, cx);
        }
        let sent = self.client.as_ref().map(|c| c.rewind_files(&point.uuid));
        match sent {
            Some(Ok(request_id)) => {
                self.set_status(kynex_core::agent_panes::AgentState::Working, "Restoring files…", cx);
                self.pending_rewind = Some(PendingRewind { request_id, point, conversation: restore == Restore::Both });
            }
            Some(Err(e)) => self.push_notice(format!("Couldn't restore the files: {e}"), true, cx),
            None => self.push_notice("Claude isn't running; start it again to restore files.", true, cx),
        }
    }

    /// The CLI answered request `request_id`: if it was a file restore, says how
    /// it went and goes on with the conversation. True if it was one.
    pub(super) fn rewind_answered(&mut self, request_id: &str, error: Option<String>, cx: &mut Context<Self>) -> bool {
        if self.pending_rewind.as_ref().is_none_or(|p| p.request_id != request_id) {
            return false;
        }
        let Some(p) = self.pending_rewind.take() else { return false };
        self.set_status(kynex_core::agent_panes::AgentState::Idle, "Ready", cx);
        match error {
            Some(e) => self.push_notice(format!("Couldn't restore the files: {e}"), true, cx),
            None => {
                let done = format!("Restored {} to before that message.", files(p.point.changed.len()));
                self.push_notice(done, false, cx);
                if p.conversation {
                    self.rewind_conversation(&p.point, cx);
                }
            }
        }
        true
    }

    /// The transcript goes back to before `point`, Claude restarts there, and the
    /// message comes back into the composer to edit and send again.
    fn rewind_conversation(&mut self, point: &Point, cx: &mut Context<Self>) {
        let Some(id) = self.session_id.clone() else { return };
        let at = self
            .rows
            .iter()
            .rposition(|r| matches!(r, LogRow::UserPrompt { uuid: Some(u), .. } if *u == point.uuid))
            .or_else(|| {
                self.rows
                    .iter()
                    .rposition(|r| matches!(r, LogRow::UserPrompt { text, .. } if text.trim() == point.text.trim()))
            });
        if let Some(at) = at {
            self.rows.truncate(at);
            self.open_runs.clear();
            self.relayout();
        }
        let session = match &point.before {
            Some(before) => SessionStart::ResumeAt { id, at: before.clone() },
            None => {
                self.session_id = None;
                SessionStart::New
            }
        };
        self.respawn(session, cx);
        self.set_composer(point.text.clone(), cx);
        self.scroll_to_bottom();
        cx.notify();
    }

    /// Stops the CLI and starts it again as `session`, with the pane's model,
    /// effort and permission mode.
    fn respawn(&mut self, session: SessionStart, cx: &mut Context<Self>) {
        if let Some(c) = self.client.take() {
            c.close();
        }
        self.turn_active = false;
        self.exited = false;
        self.ready = false;
        self.pending_card = None;
        self.queued_cards.clear();
        let cfg = SpawnConfig {
            cwd: self.config.workspace.clone(),
            session,
            launch: self.config.launch.clone(),
            model: self.active_model.clone(),
            effort: Some(self.effort),
            permission_mode: Some(self.permission_mode),
            settings_json: None,
            extra_env: self.config.extra_env.clone(),
            on_wake: Some(Arc::clone(&self.wake)),
        };
        match ClaudeClient::spawn(cfg) {
            Ok(c) => {
                if !self.thinking_on {
                    let _ = c.set_thinking(false);
                }
                self.client = Some(c);
                self.set_status(kynex_core::agent_panes::AgentState::Idle, "Starting…", cx);
            }
            Err(e) => {
                self.exited = true;
                self.push_notice(e, true, cx);
            }
        }
    }

    /// The popup above the composer: the messages, or what to restore.
    pub(super) fn render_rewind(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let r = self.rewind.as_ref()?;
        let t = theme(cx).clone();
        let row = |i: usize, label: String, hint: String, detail: String, selected: bool| {
            div()
                .id(SharedString::from(format!("rewind-{i}")))
                .flex()
                .items_center()
                .gap_2()
                .h(px(26.))
                .px_2()
                .rounded_md()
                .when(selected, |d| d.bg(t.selection))
                .child(div().flex_1().min_w_0().truncate().text_xs().text_color(t.text).child(label))
                .when(!detail.is_empty(), |d| {
                    d.child(div().flex_none().text_xs().text_color(t.text_muted).child(detail))
                })
                .when(!hint.is_empty(), |d| {
                    d.child(div().flex_none().text_xs().text_color(t.text_placeholder).child(hint))
                })
                .into_any_element()
        };
        let (header, rows): (String, Vec<AnyElement>) = match r.picked {
            None if r.loading => {
                ("Rewind".into(), vec![row(0, "Reading messages…".into(), String::new(), String::new(), false)])
            }
            None => (
                "Rewind to before…".into(),
                r.points
                    .iter()
                    .enumerate()
                    .map(|(i, p)| {
                        let first: String = p.text.lines().next().unwrap_or("").chars().take(80).collect();
                        let hint = p.time.map(kynex_core::fmt::ago).unwrap_or_default();
                        let detail = match p.changed.len() {
                            0 => String::new(),
                            n => format!("{} changed since", files(n)),
                        };
                        row(i, first, hint, detail, i == r.selected)
                    })
                    .collect(),
            ),
            Some(i) => {
                let p = &r.points[i];
                let first: String = p.text.lines().next().unwrap_or("").chars().take(60).collect();
                (
                    format!("Back to before “{first}”"),
                    choices(p)
                        .into_iter()
                        .enumerate()
                        .map(|(n, c)| {
                            let detail =
                                if c == Restore::Conversation { String::new() } else { files(p.changed.len()) };
                            row(100 + n, c.label().into(), String::new(), detail, n == r.selected)
                        })
                        .collect(),
                )
            }
        };
        Some(
            div()
                .flex_none()
                .px_4()
                .mb_1()
                .child(centered_column(
                    popover(cx)
                        .w_full()
                        .font_family(ui_font())
                        .child(
                            div()
                                .px_2()
                                .py_1()
                                .text_size(px(10.5))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(t.text_placeholder)
                                .child(format!("{header}   ↑↓ choose · Enter pick · Esc back")),
                        )
                        .child(
                            div()
                                .id("rewind-list")
                                .max_h(px(260.))
                                .overflow_y_scroll()
                                .flex()
                                .flex_col()
                                .children(rows),
                        ),
                ))
                .into_any_element(),
        )
    }
}
