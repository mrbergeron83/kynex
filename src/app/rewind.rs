//! Rewind, Claude Code's Esc Esc on an empty composer (or `/rewind`): pick an
//! earlier message, then what to restore — the conversation, the files Claude
//! changed since, or both. The popup sits above the composer like the history
//! search; `kynex_core::rewind` reads the points and restores files.

use crate::ui::px;
use gpui::{AnyElement, Context, prelude::*};
use kynex_core::live::Key;
use kynex_core::rewind::{self, Point};

use kynex_core::toasts::Toast;

use super::KynexApp;
use crate::ui::completions::{Completion, completions};

/// The open rewind popup.
pub(super) struct RewindMenu {
    /// The session it's for; it closes if another is shown.
    key: Key,
    /// Newest first.
    points: Vec<Point>,
    selected: usize,
    /// The message picked, while choosing what to restore.
    picked: Option<usize>,
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

    fn code(self) -> bool {
        self != Restore::Conversation
    }

    fn conversation(self) -> bool {
        self != Restore::Code
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

impl KynexApp {
    /// Opens the popup for the shown session, once its messages are read.
    pub(super) fn open_rewind(&mut self, cx: &mut Context<Self>) {
        self.open_rewind_at(None, cx);
    }

    /// Opens the popup; with `message` (from the timeline), straight on that
    /// message's restore choices, else on the list of messages.
    pub(super) fn open_rewind_at(&mut self, message: Option<String>, cx: &mut Context<Self>) {
        let Some(s) = self.active_session() else { return };
        if s.busy {
            self.toast(Toast::warning("Stop Claude or let it finish before rewinding."));
            return cx.notify();
        }
        let Some(id) = s.session_id.clone() else {
            self.toast(Toast::info("Nothing to rewind to yet."));
            return cx.notify();
        };
        let (key, cwd) = (s.uid, s.cwd.clone());
        cx.spawn(async move |this, cx| {
            let points = cx.background_spawn(async move { rewind::points(&cwd, &id) }).await;
            this.update(cx, |this, cx| {
                match points {
                    Ok(p) if p.is_empty() => {
                        this.toast(Toast::info("Nothing to rewind to yet."));
                    }
                    Ok(points) if this.active_key() == Some(key) => {
                        // Newest first, so a message sent twice finds its latest copy.
                        let picked =
                            message.as_deref().and_then(|m| points.iter().position(|p| p.text.trim() == m.trim()));
                        if message.is_some() && picked.is_none() {
                            this.toast(Toast::warning("Can't rewind to that message; pick it from the list."));
                        }
                        this.rewind = Some(RewindMenu { key, points, selected: 0, picked });
                    }
                    Ok(_) => {}
                    Err(e) => {
                        this.toast(Toast::error(e));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn rewinding(&self) -> bool {
        self.rewind.is_some()
    }

    /// ↑ / ↓ in the popup; false if it isn't open.
    pub(super) fn rewind_move(&mut self, down: bool, cx: &mut Context<Self>) -> bool {
        let Some(r) = &mut self.rewind else { return false };
        let n = match r.picked {
            Some(i) => choices(&r.points[i]).len(),
            None => r.points.len(),
        };
        if n > 0 {
            r.selected = if down { (r.selected + 1) % n } else { (r.selected + n - 1) % n };
        }
        cx.notify();
        true
    }

    /// Enter / Tab: picks the message, then the choice. False if it isn't open.
    pub(super) fn rewind_accept(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(r) = &mut self.rewind else { return false };
        match r.picked {
            None => {
                r.picked = Some(r.selected);
                r.selected = 0;
            }
            Some(i) => {
                let point = r.points[i].clone();
                let restore = choices(&point).get(r.selected).copied().unwrap_or(Restore::Conversation);
                let key = r.key;
                self.rewind = None;
                self.restore(key, point, restore, cx);
            }
        }
        cx.notify();
        true
    }

    /// Esc: from the choices back to the messages, then closed. False if it isn't open.
    pub(super) fn rewind_back(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(r) = &mut self.rewind else { return false };
        match r.picked.take() {
            Some(i) => r.selected = i,
            None => self.rewind = None,
        }
        cx.notify();
        true
    }

    fn restore(&mut self, key: Key, point: Point, restore: Restore, cx: &mut Context<Self>) {
        let Some(s) = self.session_for(key) else { return };
        let (Some(id), cwd) = (s.session_id.clone(), s.cwd.clone()) else { return };
        // The next message starts a process that reads the rewound state.
        s.kill();
        if !restore.code() {
            return self.rewind_conversation(key, &point, cx);
        }
        self.toast(Toast::info("Restoring files…"));
        cx.spawn(async move |this, cx| {
            let p = point.clone();
            let done = cx.background_spawn(async move { rewind::restore_files(&cwd, &id, &p) }).await;
            this.update(cx, |this, cx| {
                match done {
                    Ok(()) => {
                        this.toast(Toast::success(format!(
                            "Restored {} to before that message.",
                            files(point.changed.len())
                        )));
                        if restore.conversation() {
                            this.rewind_conversation(key, &point, cx);
                        }
                    }
                    Err(e) => {
                        this.toast(Toast::error(e));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The transcript goes back to before `point`, and its message comes back
    /// into the composer to edit and send again, as in Claude Code.
    fn rewind_conversation(&mut self, key: Key, point: &Point, cx: &mut Context<Self>) {
        let Some(s) = self.session_for(key) else { return };
        s.rewind_to(point);
        self.threads.remove(&key);
        self.expanded.retain(|(k, _)| *k != key);
        self.open_runs.retain(|(k, _)| *k != key);
        if self.active_key() == Some(key) {
            self.set_composer(point.text.clone(), cx);
        }
        cx.notify();
    }

    /// The popup above the composer: the messages, or what to restore.
    pub(super) fn render_rewind(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.rewind.as_ref().is_some_and(|r| self.active_key() != Some(r.key)) {
            self.rewind = None;
        }
        let r = self.rewind.as_ref()?;
        let (items, empty): (Vec<Completion>, _) = match r.picked {
            None => (
                r.points
                    .iter()
                    .map(|p| {
                        let first: String = p.text.lines().next().unwrap_or("").chars().take(80).collect();
                        Completion {
                            label: first.into(),
                            hint: p.time.map(kynex_core::fmt::ago).unwrap_or_default().into(),
                            detail: match p.changed.len() {
                                0 => "".into(),
                                n => format!("{} changed since", files(n)).into(),
                            },
                            ..Default::default()
                        }
                    })
                    .collect(),
                "No earlier message",
            ),
            Some(i) => {
                let p = &r.points[i];
                let detail = |c: Restore| match c {
                    Restore::Both => format!("{} back as they were, and the conversation", files(p.changed.len())),
                    Restore::Conversation if p.changed.is_empty() => "back to before this message".into(),
                    Restore::Conversation => "files stay as they are now".into(),
                    Restore::Code => format!("{} back; the conversation stays", files(p.changed.len())),
                };
                let items = choices(p)
                    .into_iter()
                    .map(|c| Completion { label: c.label().into(), detail: detail(c).into(), ..Default::default() })
                    .collect();
                (items, "")
            }
        };
        let this = cx.entity().downgrade();
        let list = completions(&items, r.selected, empty, cx, move |i, _, cx| {
            this.update(cx, |this, cx| {
                if let Some(r) = &mut this.rewind {
                    r.selected = i;
                }
                this.rewind_accept(cx);
            })
            .ok();
        });
        Some(list.w_full().max_w(px(640.)).into_any_element())
    }
}
