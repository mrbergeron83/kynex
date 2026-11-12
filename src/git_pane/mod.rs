//! The Git pane: the changes of a workspace's worktrees next to the session that made
//! them. A compact worktree list on top, the selected worktree's changed files as a
//! folder tree below, each with a box that stages it, and the commit box at the foot. A
//! click on a file asks the app for its whole-file diff, and "Commit & history…" for the
//! full Git view.
//!
//! Each worktree's operations go through a [`GitView`] of its own that the pane hosts
//! without showing it whole: staging, the commit box, errors act as they do in the Git view.
//!
//! | file            | what                                                     |
//! |-----------------|----------------------------------------------------------|
//! | `mod.rs`        | state, reading and polling, the pane's layout            |
//! | `worktrees.rs`  | the worktree list on top                                 |
//! | `tree.rs`       | the file tree's rows and their stage boxes               |
//! | `actions.rs`    | staging, discarding, a row's menu                        |
//! | `file_modal.rs` | `FileDiffView`: one file's whole diff, change by change  |

mod actions;
pub mod file_modal;
mod tree;
mod worktrees;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use crate::ui::px;
use gpui::{Context, Entity, EventEmitter, FontWeight, UniformListScrollHandle, Window, div, prelude::*};
use kynex_core::git::{self, Change, TreeFile, TreeRow, tree_files, tree_rows};
use kynex_core::workspace::same_folder;

use self::actions::{Confirm, RowMenu};
use crate::app::ToggleGit;
use crate::app::worktrees::Worktrees;
use crate::git::{GitEvent, GitView};
use crate::theme::theme;
use crate::ui::{Icon, Tooltip, icon, icon_button, outline_button};

/// How often the working tree is re-read while the pane is on screen.
const POLL_EVERY: Duration = Duration::from_secs(3);

/// What the pane asks of the app.
pub enum GitPaneEvent {
    /// Show this file's whole diff.
    OpenFile { root: PathBuf, path: String, untracked: bool },
    /// Open the full Git view (commit box, history, branches) for this worktree.
    OpenFull(PathBuf),
    /// Something was done to this worktree (staged, committed…): other views of it are old.
    Changed(PathBuf),
    /// Close the pane.
    Close,
}

impl EventEmitter<GitPaneEvent> for GitPane {}

pub struct GitPane {
    /// The workspace's folder: the worktree selected to begin with.
    root: PathBuf,
    selected: PathBuf,
    worktrees: Entity<Worktrees>,
    /// Each worktree's Git view, for its operations and the parts drawn here.
    views: HashMap<PathBuf, Entity<GitView>>,
    files: Rc<Vec<TreeFile>>,
    rows: Rc<Vec<TreeRow>>,
    /// The selected worktree has been read, so an empty list is really empty.
    loaded: bool,
    error: Option<String>,
    /// Collapsed folders, per worktree.
    closed: HashMap<PathBuf, HashSet<String>>,
    /// A read is running; `again` asks for another once it's done.
    busy: bool,
    again: bool,
    /// When the pane last drew; polling waits while it isn't on screen.
    drawn: Option<Instant>,
    scroll: UniformListScrollHandle,
    /// A row's right-click menu.
    menu: Option<RowMenu>,
    /// A row whose discard was asked for and awaits a second click.
    confirm: Option<Confirm>,
}

/// Status and line counts of every changed file in `root`. Blocking.
fn read(root: &Path) -> Result<Vec<TreeFile>, String> {
    let status = git::status(root)?;
    let untracked: Vec<&str> = status
        .files
        .iter()
        .filter(|f| f.staged == Some(Change::Untracked) || f.unstaged == Some(Change::Untracked))
        .map(|f| f.path.as_str())
        .collect();
    let stats = git::file_stats(root, &untracked)?;
    Ok(tree_files(&status, &stats))
}

impl GitPane {
    pub fn new(root: PathBuf, worktrees: Entity<Worktrees>, cx: &mut Context<Self>) -> Self {
        cx.observe(&worktrees, |this, _, cx| {
            this.poll(cx);
            cx.notify();
        })
        .detach();
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL_EVERY).await;
                if this.update(cx, |this, cx| this.poll(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        let mut pane = Self {
            selected: root.clone(),
            root,
            worktrees,
            views: HashMap::new(),
            files: Rc::default(),
            rows: Rc::default(),
            loaded: false,
            error: None,
            closed: HashMap::new(),
            busy: false,
            again: false,
            drawn: None,
            scroll: UniformListScrollHandle::new(),
            menu: None,
            confirm: None,
        };
        pane.load(cx);
        pane
    }

    /// Shows worktree `path`'s changes.
    pub fn select(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if !same_folder(&self.selected, &path) {
            self.selected = path;
            self.files = Rc::default();
            self.rows = Rc::default();
            self.loaded = false;
            self.error = None;
            self.menu = None;
            self.confirm = None;
        }
        self.load(cx);
        cx.notify();
    }

    /// Re-reads the selected worktree now.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.load(cx);
        let view = self.view(cx);
        view.update(cx, |v, cx| v.refresh(cx));
    }

    /// The selected worktree's Git view, made the first time it's needed.
    fn view(&mut self, cx: &mut Context<Self>) -> Entity<GitView> {
        if let Some(v) = self.views.get(&self.selected) {
            return v.clone();
        }
        let root = self.selected.clone();
        let view = cx.new(|cx| GitView::new(root.clone(), cx));
        cx.observe(&view, |_, _, cx| cx.notify()).detach();
        cx.subscribe(&view, move |this, _, ev: &GitEvent, cx| match ev {
            GitEvent::Changed => {
                this.load(cx);
                this.worktrees.update(cx, |w, cx| w.refresh(cx));
                cx.emit(GitPaneEvent::Changed(root.clone()));
            }
            GitEvent::OpenFolder(_) => {}
        })
        .detach();
        self.views.insert(self.selected.clone(), view.clone());
        view
    }

    fn poll(&mut self, cx: &mut Context<Self>) {
        let on_screen = self.drawn.is_some_and(|t| t.elapsed() < POLL_EVERY * 2);
        if on_screen && cx.active_window().is_some() {
            self.load(cx);
        }
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            self.again = true;
            return;
        }
        self.busy = true;
        let path = self.selected.clone();
        cx.spawn(async move |this, cx| {
            let at = path.clone();
            let files = cx.background_spawn(async move { read(&at) }).await;
            this.update(cx, |this, cx| {
                this.busy = false;
                // Another worktree was picked meanwhile, or something changed: read again.
                if this.selected != path || std::mem::take(&mut this.again) {
                    this.load(cx);
                    if this.selected != path {
                        return;
                    }
                }
                match files {
                    Ok(files) => {
                        this.error = None;
                        if !this.loaded || *this.files != files {
                            this.files = Rc::new(files);
                            this.rebuild();
                        }
                        this.loaded = true;
                    }
                    Err(e) => this.error = Some(e),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn rebuild(&mut self) {
        let none = HashSet::new();
        self.rows = Rc::new(tree_rows(&self.files, self.closed.get(&self.selected).unwrap_or(&none)));
    }

    fn toggle(&mut self, folder: &str, cx: &mut Context<Self>) {
        let closed = self.closed.entry(self.selected.clone()).or_default();
        if !closed.remove(folder) {
            closed.insert(folder.to_string());
        }
        self.rebuild();
        cx.notify();
    }

    /// The selected worktree's folder name and branch, for the bar.
    fn names(&self, cx: &Context<Self>) -> (String, Option<String>) {
        let list = &self.worktrees.read(cx).list;
        let branch = list.iter().find(|w| same_folder(&w.path, &self.selected)).map(|w| w.label().to_string());
        let name = self.selected.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        (name, branch)
    }
}

impl Render for GitPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.drawn = Some(Instant::now());
        let t = theme(cx).clone();
        let view = self.view(cx);
        let (name, branch) = self.names(cx);

        let full = self.selected.clone();
        let header = div()
            .flex()
            .flex_none()
            .items_center()
            .gap_1p5()
            .h(px(36.))
            .px_3()
            .border_b_1()
            .border_color(t.border_variant)
            .child(div().flex_none().text_sm().font_weight(FontWeight::MEDIUM).child(name))
            .when_some(branch, |d, b| {
                d.child(icon(Icon::GitBranch, t.text_muted).size_3())
                    .child(div().min_w_0().truncate().text_xs().text_color(t.text_muted).child(b))
            })
            .child(div().flex_1())
            .child(
                icon_button("git-pane-refresh", Icon::Refresh, cx)
                    .tooltip(Tooltip::text("Refresh"))
                    .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
            )
            .child(
                outline_button("git-pane-full", "Commit & history…", cx)
                    .tooltip(Tooltip::text("Commit, history and branches"))
                    .on_click(cx.listener(move |_, _, _, cx| cx.emit(GitPaneEvent::OpenFull(full.clone())))),
            )
            .child(
                icon_button("git-pane-close", Icon::Close, cx)
                    .tooltip(Tooltip::for_action("Close the Git pane", ToggleGit, ""))
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(GitPaneEvent::Close))),
            );

        let worktrees = self.render_worktrees(cx);
        let notices = view.update(cx, |v, cx| v.render_notices(cx));
        let tools = self.render_tree_bar(&view, cx);
        let body = self.render_tree(cx);
        let commit = view.update(cx, |v, cx| v.render_commit_area(window, cx));
        let menu = self.render_menu(cx);
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(t.panel)
            .text_color(t.text)
            .child(header)
            .child(worktrees)
            .children(notices)
            .children(tools)
            .child(body)
            .child(div().flex_none().border_t_1().border_color(t.border_variant).child(commit))
            .children(menu)
    }
}
