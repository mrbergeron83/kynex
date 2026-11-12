//! The Git pane: the changes of a workspace's worktrees next to the session that made
//! them. A compact worktree list on top, the selected worktree's changed files as a
//! folder tree below; a click on a file asks the app for its whole-file diff, and
//! "Commit & history…" for the full Git view.
//!
//! | file            | what                                                     |
//! |-----------------|----------------------------------------------------------|
//! | `mod.rs`        | state, reading and polling, the pane's layout            |
//! | `tree.rs`       | the file tree's rows                                     |
//! | `file_modal.rs` | `FileDiffView`: one file's whole diff, change by change  |

pub mod file_modal;
mod tree;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use crate::ui::px;
use gpui::{Context, Entity, EventEmitter, FontWeight, UniformListScrollHandle, Window, div, prelude::*};
use kynex_core::git::{self, Change, TreeFile, TreeRow, tree_files, tree_rows};
use kynex_core::workspace::same_folder;

use crate::app::worktrees::Worktrees;
use crate::git::diff_view::counts;
use crate::theme::theme;
use crate::ui::wash::row_wash;
use crate::ui::{Icon, Tooltip, icon, icon_button, outline_button};

/// How often the working tree is re-read while the pane is on screen.
const POLL_EVERY: Duration = Duration::from_secs(3);

/// What the pane asks of the app.
pub enum GitPaneEvent {
    /// Show this file's whole diff.
    OpenFile { root: PathBuf, path: String, untracked: bool },
    /// Open the full Git view (commit box, history, branches) for this worktree.
    OpenFull(PathBuf),
}

impl EventEmitter<GitPaneEvent> for GitPane {}

pub struct GitPane {
    /// The workspace's folder: the worktree selected to begin with.
    root: PathBuf,
    selected: PathBuf,
    worktrees: Entity<Worktrees>,
    files: Rc<Vec<TreeFile>>,
    rows: Rc<Vec<TreeRow>>,
    /// The selected worktree has been read, so an empty list is really empty.
    loaded: bool,
    error: Option<String>,
    /// Collapsed folders, per worktree.
    closed: HashMap<PathBuf, HashSet<String>>,
    /// A read is running.
    busy: bool,
    /// When the pane last drew; polling waits while it isn't on screen.
    drawn: Option<Instant>,
    scroll: UniformListScrollHandle,
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
            files: Rc::default(),
            rows: Rc::default(),
            loaded: false,
            error: None,
            closed: HashMap::new(),
            busy: false,
            drawn: None,
            scroll: UniformListScrollHandle::new(),
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
        }
        self.load(cx);
        cx.notify();
    }

    /// Re-reads the selected worktree now.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.load(cx);
    }

    fn poll(&mut self, cx: &mut Context<Self>) {
        let on_screen = self.drawn.is_some_and(|t| t.elapsed() < POLL_EVERY * 2);
        if on_screen && cx.active_window().is_some() {
            self.load(cx);
        }
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.busy = true;
        let path = self.selected.clone();
        cx.spawn(async move |this, cx| {
            let at = path.clone();
            let files = cx.background_spawn(async move { read(&at) }).await;
            this.update(cx, |this, cx| {
                this.busy = false;
                // Another worktree was picked meanwhile: read that one.
                if this.selected != path {
                    this.load(cx);
                    return;
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
}

impl Render for GitPane {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.drawn = Some(Instant::now());
        let t = theme(cx).clone();
        let list = self.worktrees.read(cx).list.clone();
        let branch = list.iter().find(|w| same_folder(&w.path, &self.selected)).map(|w| w.label().to_string());

        let full = self.selected.clone();
        let header = div()
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .h(px(36.))
            .px_3()
            .border_b_1()
            .border_color(t.border_variant)
            .child(div().text_sm().font_weight(FontWeight::MEDIUM).child("Changes"))
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
            );

        // One row per worktree; a project with a single checkout still shows it.
        let mut rows = Vec::new();
        for (i, w) in list.iter().enumerate() {
            let selected = same_folder(&w.path, &self.selected);
            let path = w.path.clone();
            rows.push(
                div()
                    .id(("git-pane-wt", i))
                    .relative()
                    .flex()
                    .items_center()
                    .gap_2()
                    .h(px(26.))
                    .px_2()
                    .rounded_md()
                    .cursor_pointer()
                    .child(row_wash(selected, t.hover, t.active))
                    .child(icon(Icon::GitBranch, t.text_muted).size_3())
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .text_color(if selected { t.text } else { t.text_muted })
                            .child(w.label().to_string()),
                    )
                    .when(!w.main, |d| d.child(div().flex_none().text_xs().text_color(t.info).child("worktree")))
                    .when_some(w.changes.filter(|&n| n > 0), |d, n| {
                        d.child(div().flex_none().text_xs().text_color(t.warning).child(n.to_string()))
                            .when_some(w.lines, |d, (a, r)| d.child(counts(a, r, &t)))
                    })
                    .tooltip(Tooltip::text(w.path.display().to_string()))
                    .on_click(cx.listener(move |this, _, _, cx| this.select(path.clone(), cx)))
                    .into_any_element(),
            );
        }
        if rows.is_empty() {
            let name = self.root.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
            rows.push(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .h(px(26.))
                    .px_2()
                    .rounded_md()
                    .bg(t.active)
                    .child(icon(Icon::GitBranch, t.text_muted).size_3())
                    .child(div().text_sm().child(name))
                    .into_any_element(),
            );
        }
        let worktrees = div()
            .id("git-pane-worktrees")
            .flex_none()
            .max_h(px(150.))
            .overflow_y_scroll()
            .p_1()
            .border_b_1()
            .border_color(t.border_variant)
            .children(rows);

        let body = self.render_tree(cx);
        div().size_full().flex().flex_col().bg(t.panel).text_color(t.text).child(header).child(worktrees).child(body)
    }
}
