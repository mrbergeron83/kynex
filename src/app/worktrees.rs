//! The project's git worktrees, kept up to date in the background: their
//! branch, changed files and distance from main, for the workspaces section of the
//! sessions panel (`workspaces.rs`) and the Git pane.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::Poll;
use std::time::{Duration, Instant};

use gpui::{App, Context, Entity, prelude::*};
use kynex_core::git::{self, Worktree};

use super::KynexApp;

/// Fallback poll interval for what a filesystem watcher can miss (active window only).
const FALLBACK_EVERY: Duration = Duration::from_secs(60);

/// Short debounce after a filesystem event before re-reading git worktrees.
const WATCH_DEBOUNCE: Duration = Duration::from_millis(200);

/// Waits until either a wake arrives on `rx` or `wait` elapses, without spinning.
async fn wait_or_wake(rx: &async_channel::Receiver<()>, wait: Duration, cx: &gpui::AsyncApp) -> bool {
    let mut recv = std::pin::pin!(rx.recv());
    let mut timer = std::pin::pin!(cx.background_executor().timer(wait));
    std::future::poll_fn(|cx| {
        if let Poll::Ready(res) = recv.as_mut().poll(cx) {
            return Poll::Ready(res.is_ok());
        }
        if timer.as_mut().poll(cx).is_ready() {
            return Poll::Ready(true);
        }
        Poll::Pending
    })
    .await
}

/// A project's worktrees, re-read in the background while the project is open.
pub struct Worktrees {
    pub list: Vec<Worktree>,
    root: PathBuf,
    reading: bool,
    again: bool,
    watch: Option<kynex_core::watch::GitWatch>,
    wake: async_channel::Sender<()>,
}

impl Worktrees {
    fn new(root: PathBuf, cx: &mut Context<Self>) -> Self {
        let changed = Arc::new(AtomicBool::new(false));
        let (wake_tx, wake_rx) = async_channel::bounded::<()>(1);
        let flag = changed.clone();
        let notify_tx = wake_tx.clone();
        let watch = kynex_core::watch::watch_git(&root, move || {
            flag.store(true, Ordering::Relaxed);
            if crate::ui::motion::window_active() {
                let _ = notify_tx.try_send(());
            }
        });
        cx.spawn(async move |this, cx| {
            let mut last: Option<Instant> = None;
            loop {
                if !crate::ui::motion::window_active() {
                    if wake_rx.recv().await.is_err() {
                        break;
                    }
                    continue;
                }
                let due = last.is_none_or(|t| t.elapsed() >= FALLBACK_EVERY);
                let dirty = changed.swap(false, Ordering::Relaxed);
                if dirty || due {
                    if dirty && last.is_some() {
                        cx.background_executor().timer(WATCH_DEBOUNCE).await;
                        changed.store(false, Ordering::Relaxed);
                    }
                    if this.update(cx, |this, cx| this.refresh(cx)).is_err() {
                        break;
                    }
                    last = Some(Instant::now());
                }
                let wait = last.map_or(FALLBACK_EVERY, |t| FALLBACK_EVERY.saturating_sub(t.elapsed()));
                if !wait_or_wake(&wake_rx, wait.max(WATCH_DEBOUNCE), cx).await {
                    break;
                }
            }
        })
        .detach();
        Self { list: Vec::new(), root, reading: false, again: false, watch, wake: wake_tx }
    }

    /// Wakes the background watcher (e.g. when the window regains focus).
    pub fn wake(&self) {
        let _ = self.wake.try_send(());
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.reading {
            self.again = true;
            return;
        }
        self.reading = true;
        let root = self.root.clone();
        cx.spawn(async move |this, cx| {
            let at = root.clone();
            let read = cx.background_spawn(async move { git::list_worktrees_quick(&at) }).await;
            let Ok(mut list) = read else {
                this.update(cx, |this, cx| this.read_done(cx)).ok();
                return;
            };
            this.update(cx, |this, cx| {
                let _t = kynex_core::trace::span("worktrees: publish the list (UI thread)");
                let known: Vec<Worktree> = list
                    .iter()
                    .map(|w| match this.list.iter().find(|o| o.path == w.path) {
                        Some(o) => Worktree {
                            changes: o.changes,
                            lines: o.lines,
                            ahead_behind: o.ahead_behind,
                            upstream: o.upstream,
                            ..w.clone()
                        },
                        None => w.clone(),
                    })
                    .collect();
                if this.list != known {
                    this.list = known;
                    cx.notify();
                }
            })
            .ok();
            let list = cx
                .background_spawn(async move {
                    git::fill_worktrees(&root, &mut list);
                    list
                })
                .await;
            this.update(cx, |this, cx| {
                let _t = kynex_core::trace::span("worktrees: publish the numbers (UI thread)");
                if this.list != list {
                    this.list = list;
                    cx.notify();
                }
                this.read_done(cx);
            })
            .ok();
        })
        .detach();
    }

    fn read_done(&mut self, cx: &mut Context<Self>) {
        self.reading = false;
        if let Some(w) = &mut self.watch {
            w.rewatch();
        }
        if std::mem::take(&mut self.again) {
            self.refresh(cx);
        }
    }

    pub fn at(&self, path: &Path) -> Option<&Worktree> {
        self.list.iter().find(|w| w.path == path)
    }
}

impl KynexApp {
    /// The worktree list for a project folder, started on first use.
    pub(super) fn worktrees(&mut self, path: &Path, cx: &mut Context<Self>) -> Entity<Worktrees> {
        if let Some(w) = self.worktrees.get(path) {
            return w.clone();
        }
        let root = path.to_path_buf();
        let entity = cx.new(|cx| Worktrees::new(root, cx));
        let project = path.to_path_buf();
        cx.observe(&entity, move |this, _, cx| {
            this.worktrees_changed(&project, cx);
            cx.notify();
        })
        .detach();
        self.worktrees.insert(path.to_path_buf(), entity.clone());
        entity
    }

    /// Re-reads the worktrees of these folders, where they're being watched, and wakes
    /// their watcher loops after window focus resumes.
    pub(super) fn refresh_worktrees<'a>(&mut self, folders: impl IntoIterator<Item = &'a PathBuf>, cx: &mut App) {
        for folder in folders {
            if let Some(w) = self.worktrees.get(folder) {
                w.update(cx, |w, cx| {
                    w.wake();
                    w.refresh(cx);
                });
            }
        }
    }
}
