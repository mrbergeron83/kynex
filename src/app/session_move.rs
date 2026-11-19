//! A session that makes a git worktree (`git worktree add`) moves into it when that turn ends:
//! its log is copied to the worktree's transcripts folder, and it resumes there with the
//! worktree as its folder, told so. It's listed under that workspace from then on. Messages
//! queued meanwhile are held back from the old process and go after the move.
//!
//! The worktree is known once git lists it ([`Worktrees`](super::worktrees::Worktrees) reads
//! the list again when a tool call finishes), so a call that added one is watched for a while.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gpui::{AppContext, Context};
use kynex_core::git;
use kynex_core::live::Key;
use kynex_core::session::{self, Item, Kind, Session};
use kynex_core::toasts::Toast;
use kynex_core::workspace::same_folder;

use super::KynexApp;

/// How long a `git worktree add` waits for git to list what it made.
const WATCH_FOR: Duration = Duration::from_secs(60);

/// When the old log is deleted, so the old process has let go of it: tried after each wait.
const DELETE_AFTER: [Duration; 3] = [Duration::from_secs(3), Duration::from_secs(10), Duration::from_secs(30)];

#[derive(Default)]
pub(super) struct Moves {
    /// Calls that added a worktree git hasn't listed yet: session, text naming it, since when.
    watching: Vec<(Key, String, Instant)>,
    /// Sessions on their way to a worktree.
    pending: Vec<Pending>,
}

struct Pending {
    key: Key,
    to: PathBuf,
    branch: Option<String>,
    /// Its log is being copied.
    copying: bool,
}

/// If session `s`'s call `id` added a git worktree: text naming the folder.
pub(super) fn adds_worktree(s: &Session, id: &str) -> Option<String> {
    let (name, input) = s.items.iter().rev().find_map(|i| match i {
        Item::ToolUse { id: c, name, input, .. } if c == id => Some((name, input)),
        _ => None,
    })?;
    git::adds_worktree(name, input)
}

/// What the session is told once it's in the worktree.
fn moved_note(to: &Path, branch: Option<&str>) -> String {
    let on = branch.map(|b| format!(" (branch {b})")).unwrap_or_default();
    format!(
        "Kynex moved this session into the worktree you created, {}{on}: your working directory \
         is that folder now. Work there from here on.",
        to.display()
    )
}

impl KynexApp {
    /// Session `key` ran a `git worktree add` naming `text`: it moves into the worktree once
    /// git lists it.
    pub(super) fn worktree_added(&mut self, key: Key, text: String, cx: &mut Context<Self>) {
        self.moves.watching.retain(|(k, ..)| *k != key);
        self.moves.watching.push((key, text, Instant::now()));
        if let Some(project) = self.project_of(key) {
            self.check_added(&project, cx);
        }
    }

    /// `project`'s worktree list changed: sessions whose new worktree is in it start moving.
    pub(super) fn check_added(&mut self, project: &Path, cx: &mut Context<Self>) {
        if self.moves.watching.is_empty() {
            return;
        }
        self.moves.watching.retain(|(.., since)| since.elapsed() < WATCH_FOR);
        let list = self.worktrees(project, cx).read(cx).list.clone();
        let found: Vec<(Key, git::Worktree)> = self
            .moves
            .watching
            .iter()
            .filter(|(k, ..)| self.project_of(*k).as_deref() == Some(project))
            .filter_map(|(k, text, _)| Some((*k, git::worktree_in(text, &list)?.clone())))
            .collect();
        for (key, w) in found {
            self.moves.watching.retain(|(k, ..)| *k != key);
            self.begin_move(key, w);
        }
    }

    /// Session `key` moves into worktree `w` when its turn ends.
    fn begin_move(&mut self, key: Key, w: git::Worktree) {
        let Some(s) = self.session_for(key) else { return };
        let movable = s.kind == Kind::Code
            && !s.provider().provider().is_acp()
            && s.session_id.is_some()
            && !same_folder(&s.cwd, &w.path);
        if !movable {
            return;
        }
        // What's queued goes after the move, not to the old process.
        s.hold_queue = true;
        self.moves.pending.retain(|p| p.key != key);
        self.moves.pending.push(Pending { key, to: w.path, branch: w.branch, copying: false });
    }

    /// Moves the sessions whose turn has ended: their log is copied, then they resume there.
    pub(super) fn advance_moves(&mut self, cx: &mut Context<Self>) {
        if self.moves.pending.is_empty() {
            return;
        }
        // A session closed meanwhile isn't moved.
        let open: Vec<Key> = self.all_sessions().map(|s| s.uid).collect();
        self.moves.pending.retain(|p| open.contains(&p.key));
        let ready: Vec<(Key, PathBuf, PathBuf, String)> = self
            .moves
            .pending
            .iter()
            .filter(|p| !p.copying)
            .filter_map(|p| {
                let s = self.session_by_key(p.key).filter(|s| !s.busy)?;
                Some((p.key, s.cwd.clone(), p.to.clone(), s.session_id.clone()?))
            })
            .collect();
        for (key, from, to, id) in ready {
            if let Some(p) = self.moves.pending.iter_mut().find(|p| p.key == key) {
                p.copying = true;
            }
            cx.spawn(async move |this, cx| {
                let (f, t, i) = (from.clone(), to.clone(), id.clone());
                let copied = cx.background_spawn(async move { session::move_to(&f, &i, &t) }).await;
                this.update(cx, |this, cx| {
                    match copied {
                        Ok(()) => this.finish_move(key, from, id, cx),
                        Err(e) => {
                            // It stays where it is; what was held back goes now.
                            this.moves.pending.retain(|p| p.key != key);
                            if let Some(s) = this.session_for(key) {
                                s.hold_queue = false;
                                s.send_queue_now();
                            }
                            this.toast(
                                Toast::error(format!("Couldn't move the session to {}", to.display())).detail(e),
                            );
                        }
                    }
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
    }

    /// The log is in the worktree's folder: the old session closes and resumes there.
    fn finish_move(&mut self, key: Key, from: PathBuf, id: String, cx: &mut Context<Self>) {
        // Sent a message while the log was copied: copied again once that turn is over.
        if self.session_by_key(key).is_none_or(|s| s.busy) {
            if let Some(p) = self.moves.pending.iter_mut().find(|p| p.key == key) {
                p.copying = false;
            }
            return;
        }
        let Some(i) = self.moves.pending.iter().position(|p| p.key == key) else { return };
        let p = self.moves.pending.remove(i);
        let Some(project) = self.project_of(key) else { return };
        let Some(old) = self.session_by_key(key) else { return };
        let (mode, own_model) = (old.mode, old.own_model.clone());
        let settings = old.settings_dir.clone().unwrap_or_else(|| from.clone());
        let shown = self.code.get(&project).and_then(|l| l.current()).map(|s| s.uid) == Some(key);
        let name = self.running_name(old);
        let queued = self.session_for(key).and_then(|s| s.queue.take_all());

        // Its name, pin, model and provider go with it.
        let g = self.groups_for(&from);
        let (title, pinned, model, provider) = (
            g.name_of(&id).map(str::to_string),
            g.is_pinned(&id),
            g.model_of(&id).map(str::to_string),
            g.provider_of(&id),
        );
        let to = p.to.clone();
        self.edit_groups(&to, |g| {
            if let Some(t) = &title {
                g.rename(&id, t);
            }
            if pinned && !g.is_pinned(&id) {
                g.toggle_pin(&id);
            }
            g.set_model(&id, model.as_deref());
            g.set_provider(&id, provider);
        });
        self.edit_groups(&from, |g| g.forget(&id));

        // Out of its pane in the old workspace, where that leaves others.
        for t in self.tiles.values_mut() {
            if let Some(pane) = t.pane_of(key) {
                t.close_pane(pane);
            }
        }
        let closed = self.code.get_mut(&project).map(|l| l.close_where(|s| s.uid == key)).unwrap_or_default();
        self.forget(&closed);

        let mut s = Session::resume(Kind::Code, to.clone(), id.clone(), self.wake.clone());
        s.mode = mode;
        s.own_model = own_model;
        s.settings_dir = Some(settings).filter(|d| !same_folder(d, &to));
        self.start_model(&mut s);
        s.wake = self.wake.clone();
        // What you queued goes now, told of the move first. With nothing queued the move starts
        // no turn: the new process's own context names its working directory.
        if let Some(m) = queued {
            s.send(format!("{}\n\n{}", moved_note(&to, p.branch.as_deref()), m.text), m.attachments);
        }
        let new = s.uid;
        if let Some(live) = self.code.get_mut(&project) {
            live.sessions.push(s);
            if shown {
                live.focus(new);
            }
        }
        let workspaces = self.saved_workspaces(&project);
        workspaces.remember(&to, Some(&id));
        if shown {
            workspaces.select(&to);
        }
        self.save_workspaces(&project, cx);
        self.reread_sessions();
        let label = p.branch.clone().unwrap_or_else(|| to.display().to_string());
        self.toast(Toast::info(format!("Moved \u{201c}{name}\u{201d} to {label}")));

        // The old log goes once the old process has let go of it.
        cx.spawn(async move |this, cx| {
            let mut gone = Err(String::new());
            for wait in DELETE_AFTER {
                cx.background_executor().timer(wait).await;
                let (f, i, t) = (from.clone(), id.clone(), to.clone());
                gone = cx.background_spawn(async move { session::drop_moved(&f, &i, &t) }).await;
                if gone.is_ok() {
                    break;
                }
            }
            this.update(cx, |this, cx| {
                match gone {
                    Ok(()) => this.reread_sessions(),
                    Err(e) => {
                        _ = this.toast(Toast::warning("The moved session is still listed in its old folder").detail(e))
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}
