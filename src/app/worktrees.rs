//! The project's git worktrees, kept up to date in the background: their
//! branch, changed files and distance from main, for the contexts section of the
//! sessions panel (`contexts.rs`) and the session header. A worktree can also be
//! opened as a project tab, on its Git view.

use kynex_core::live::Key;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use gpui::{App, Context, Entity, prelude::*};
use kynex_core::git::{self, Worktree};
use kynex_core::session::{Item, Kind};

use super::KynexApp;
use super::state::{Active, View};

/// Worktrees change behind our back (Claude makes and removes them). The git folder
/// is watched for that; this is the net under it, for what a watcher can miss (the
/// working tree's own changes, a project that has no watchable git folder).
const FALLBACK_EVERY: Duration = Duration::from_secs(60);

/// How often the watcher's flag is looked at (no `git` runs unless it's set).
const CHECK_EVERY: Duration = Duration::from_secs(1);

/// A project's worktrees, re-read in the background while the project is open: when
/// its git folder changes, and sooner when a turn ends, a tool call finishes or the
/// window comes back; otherwise only as a slow fallback.
pub struct Worktrees {
    pub list: Vec<Worktree>,
    root: PathBuf,
    /// A read is running; `again` asks for another one once it's done.
    reading: bool,
    again: bool,
    /// Watching the git folder (a worktree made or removed, a commit, a branch switch).
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

    /// Re-reads the list now (or right after the read already running).
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.reading {
            self.again = true;
            return;
        }
        self.reading = true;
        let root = self.root.clone();
        cx.spawn(async move |this, cx| {
            // The list first (one `git` call), so a new or removed worktree shows at once;
            // each one's changes follow, read in parallel.
            let at = root.clone();
            let mut list = cx.background_spawn(async move { git::list_worktrees_quick(&at).unwrap_or_default() }).await;
            this.update(cx, |this, cx| {
                let _t = kynex_core::trace::span("worktrees: publish the list (UI thread)");
                // Until its numbers are in, a worktree keeps the last ones.
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
                this.reading = false;
                // `worktrees` appears with the first linked worktree and goes with the last.
                if let Some(w) = &mut this.watch {
                    w.rewatch();
                }
                if this.list != list {
                    this.list = list;
                    cx.notify();
                }
                if std::mem::take(&mut this.again) {
                    this.refresh(cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// The worktree whose folder is `path`.
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
        cx.observe(&entity, |_, _, cx| cx.notify()).detach();
        self.worktrees.insert(path.to_path_buf(), entity.clone());
        entity
    }

    /// The linked worktree the shown session is working in, judging by its most
    /// recent tool call that points into one (Claude often works in a worktree
    /// by path while the session itself runs in the main folder).
    pub(super) fn working_worktree(&mut self, key: Key, cx: &mut Context<Self>) -> Option<Worktree> {
        let s = self.session_by_key(key).filter(|s| s.kind == Kind::Code)?;
        let cwd = s.cwd.clone();
        let inputs: Vec<String> = s
            .items
            .iter()
            .rev()
            .filter_map(|i| match i {
                Item::ToolUse { input, .. } => Some(input.clone()),
                _ => None,
            })
            .take(300)
            .collect();
        let project = self.context_project().unwrap_or_else(|| cwd.clone());
        let list = self.worktrees(&project, cx).read(cx).list.clone();
        inputs.iter().find_map(|input| git::worktree_in(input, &list)).filter(|w| w.path != cwd).cloned()
    }

    /// Re-reads the worktrees of these folders, where they're being watched.
    pub(super) fn refresh_worktrees<'a>(&mut self, folders: impl IntoIterator<Item = &'a PathBuf>, cx: &mut App) {
        for folder in folders {
            if let Some(w) = self.worktrees.get(folder) {
                w.update(cx, |w, cx| w.refresh(cx));
            }
        }
    }

    /// Opens a worktree to look at its changes: its project's context when it has
    /// one, else a tab on its Git view.
    pub(super) fn open_worktree_changes(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        // A worktree of an open project is one of its contexts: switch to it.
        if self.open_as_context(&path, cx) {
            return cx.notify();
        }
        self.add_project(path);
        // A worktree is another view of a project you have: no setup checklist for it.
        self.setup_for = None;
        if let Active::Project(i) = self.saved.active
            && let Some(p) = self.saved.projects.get_mut(i)
        {
            p.view = View::Git;
        }
        self.save();
        cx.notify();
    }
}
