//! The project's git worktrees, kept up to date in the background: their
//! branch, changed files and distance from main, for the workspaces section of the
//! sessions panel (`workspaces.rs`) and the Git pane.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use gpui::{App, Context, Entity, prelude::*};
use kynex_core::git::{self, Worktree};

use super::KynexApp;

/// Fallback poll interval for what a filesystem watcher can miss.
const FALLBACK_EVERY: Duration = Duration::from_secs(60);

/// How often the watcher's flag is looked at (no `git` runs unless it's set).
const CHECK_EVERY: Duration = Duration::from_secs(1);

/// A project's worktrees, re-read in the background while the project is open.
pub struct Worktrees {
    pub list: Vec<Worktree>,
    root: PathBuf,
    reading: bool,
    again: bool,
    watch: Option<kynex_core::watch::GitWatch>,
}

impl Worktrees {
    fn new(root: PathBuf, cx: &mut Context<Self>) -> Self {
        let changed = Arc::new(AtomicBool::new(false));
        let flag = changed.clone();
        let watch = kynex_core::watch::watch_git(&root, move || flag.store(true, Ordering::Relaxed));
        cx.spawn(async move |this, cx| {
            let mut last: Option<Instant> = None;
            loop {
                let due = last.is_none_or(|t| t.elapsed() >= FALLBACK_EVERY);
                if changed.swap(false, Ordering::Relaxed) || due {
                    if this.update(cx, |this, cx| this.refresh(cx)).is_err() {
                        break;
                    }
                    last = Some(Instant::now());
                }
                cx.background_executor().timer(CHECK_EVERY).await;
            }
        })
        .detach();
        Self { list: Vec::new(), root, reading: false, again: false, watch }
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

    /// Re-reads the worktrees of these folders, where they're being watched.
    pub(super) fn refresh_worktrees<'a>(&mut self, folders: impl IntoIterator<Item = &'a PathBuf>, cx: &mut App) {
        for folder in folders {
            if let Some(w) = self.worktrees.get(folder) {
                w.update(cx, |w, cx| w.refresh(cx));
            }
        }
    }
}
