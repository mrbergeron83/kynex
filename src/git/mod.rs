//! The Git view for one project: a top bar with the branch picker and
//! fetch / pull / push, a left column with the Changes tab (commit box, staged
//! and unstaged files) and the History tab, and a diff on the right.
//!
//! | file           | what                                                   |
//! |----------------|--------------------------------------------------------|
//! | `mod.rs`       | state, refreshing, running operations, the layout      |
//! | `top_bar.rs`   | branch picker, upstream, fetch / pull / push           |
//! | `changes.rs`   | commit box and the staged / unstaged file lists        |
//! | `history.rs`   | the commit list                                        |
//! | `diff_view.rs` | a diff (one file, or a whole commit) as numbered rows  |
//!
//! Git itself is `kynex_core::git`; every call runs on the background executor.

mod changes;
mod diff_view;
mod history;
mod top_bar;

use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui::{Context, Entity, FontWeight, SharedString, UniformListScrollHandle, Window, div, prelude::*, px};
use kynex_core::git::{self, Branch, Change, Commit, CommitDetail, Status};

use self::diff_view::DiffState;
use crate::theme::theme;
use crate::ui::input::{InputEvent, TextInput};
use crate::ui::wash::row_wash;
use crate::ui::{Icon, icon, icon_button};

/// How often the working tree is re-read while the view exists.
const POLL_EVERY: Duration = Duration::from_secs(3);
const HISTORY_LIMIT: usize = 500;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Changes,
    History,
}

/// What the right-hand side shows.
#[derive(Clone, PartialEq)]
enum Selection {
    File { path: String, staged: bool },
    Commit(String),
}

pub struct GitView {
    root: PathBuf,
    /// `None` until the first read finishes.
    status: Option<Status>,
    commits: Vec<Commit>,
    branches: Vec<Branch>,
    tab: Tab,
    selected: Option<Selection>,
    diff: DiffState,
    detail: Option<CommitDetail>,
    /// An operation in flight ("Committing", "Pushing"…); the rest wait for it.
    busy: Option<SharedString>,
    /// The last thing that went wrong, until dismissed.
    error: Option<String>,
    /// A status read is running.
    polling: bool,
    message: Entity<TextInput>,
    amend: bool,
    branch_menu: bool,
    new_branch: Entity<TextInput>,
    /// A file whose discard button was clicked once and awaits a second click.
    confirm_discard: Option<String>,
    history_scroll: UniformListScrollHandle,
    diff_scroll: UniformListScrollHandle,
}

impl GitView {
    pub fn new(root: PathBuf, cx: &mut Context<Self>) -> Self {
        let message = cx.new(|cx| {
            TextInput::new(cx).multiline().placeholder("Message (Enter to commit, Shift+Enter for a new line)")
        });
        cx.subscribe(&message, |this, _, ev, cx| {
            if let InputEvent::Submit = ev {
                this.commit(cx);
            }
        })
        .detach();
        let new_branch = cx.new(|cx| TextInput::new(cx).placeholder("New branch name"));
        cx.subscribe(&new_branch, |this, _, ev, cx| {
            if let InputEvent::Submit = ev {
                this.create_branch(cx);
            }
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
        let mut view = Self {
            root,
            status: None,
            commits: Vec::new(),
            branches: Vec::new(),
            tab: Tab::Changes,
            selected: None,
            diff: DiffState::default(),
            detail: None,
            busy: None,
            error: None,
            polling: false,
            message,
            amend: false,
            branch_menu: false,
            new_branch,
            confirm_discard: None,
            history_scroll: UniformListScrollHandle::new(),
            diff_scroll: UniformListScrollHandle::new(),
        };
        view.refresh(cx);
        view
    }

    /// Re-reads everything: status, history and branches.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let root = self.root.clone();
        cx.spawn(async move |this, cx| {
            let (status, commits, branches) = cx
                .background_spawn(
                    async move { (git::status(&root), git::log(&root, HISTORY_LIMIT), git::branches(&root)) },
                )
                .await;
            this.update(cx, |this, cx| {
                match status {
                    Ok(s) => this.set_status(s, cx),
                    Err(e) => this.error = Some(e),
                }
                if let Ok(c) = commits {
                    this.commits = c;
                }
                if let Ok(b) = branches {
                    this.branches = b;
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The cheap periodic check: status only, and everything else if HEAD moved.
    fn poll(&mut self, cx: &mut Context<Self>) {
        if self.polling || self.busy.is_some() {
            return;
        }
        self.polling = true;
        let root = self.root.clone();
        cx.spawn(async move |this, cx| {
            let status = cx.background_spawn(async move { git::status(&root) }).await;
            this.update(cx, |this, cx| {
                this.polling = false;
                let Ok(status) = status else { return };
                let moved = this.status.as_ref().is_none_or(|s| s.oid != status.oid || s.head != status.head);
                if moved {
                    this.refresh(cx);
                } else {
                    this.set_status(status, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    fn set_status(&mut self, status: Status, cx: &mut Context<Self>) {
        if self.status.as_ref() == Some(&status) {
            return;
        }
        // Drop a file selection that no longer has changes on that side.
        if let Some(Selection::File { path, staged }) = &self.selected {
            let still = status
                .files
                .iter()
                .any(|f| &f.path == path && if *staged { f.staged.is_some() } else { f.unstaged.is_some() });
            if !still {
                self.selected = None;
                self.diff = DiffState::default();
            }
        }
        self.status = Some(status);
        if matches!(self.selected, Some(Selection::File { .. })) {
            self.load_diff(cx);
        }
        cx.notify();
    }

    fn select(&mut self, selection: Selection, cx: &mut Context<Self>) {
        if self.selected.as_ref() == Some(&selection) {
            return;
        }
        self.selected = Some(selection);
        self.diff = DiffState::loading();
        self.detail = None;
        self.diff_scroll = UniformListScrollHandle::new();
        self.load_diff(cx);
        cx.notify();
    }

    /// Loads the diff for the current selection.
    fn load_diff(&mut self, cx: &mut Context<Self>) {
        let Some(selection) = self.selected.clone() else { return };
        let root = self.root.clone();
        let untracked = match &selection {
            Selection::File { path, .. } => self
                .status
                .as_ref()
                .and_then(|s| s.files.iter().find(|f| &f.path == path))
                .is_some_and(|f| f.unstaged == Some(Change::Untracked)),
            Selection::Commit(_) => false,
        };
        cx.spawn(async move |this, cx| {
            let sel = selection.clone();
            let loaded = cx
                .background_spawn(async move {
                    match &sel {
                        Selection::File { path, staged } => {
                            git::file_diff(&root, path, *staged, untracked).map(|d| (vec![d], None))
                        }
                        Selection::Commit(sha) => git::commit_detail(&root, sha).map(|mut d| {
                            let files = std::mem::take(&mut d.files);
                            (files, Some(d))
                        }),
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                // The selection may have moved on while this ran.
                if this.selected.as_ref() != Some(&selection) {
                    return;
                }
                let multi = matches!(selection, Selection::Commit(_));
                match loaded {
                    Ok((files, detail)) => {
                        this.diff = DiffState::new(&files, multi);
                        this.detail = detail;
                    }
                    Err(e) => this.diff = DiffState::failed(e),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Runs a git operation in the background, then refreshes. `then` runs on
    /// success, back on the UI thread.
    fn run(
        &mut self,
        label: &'static str,
        cx: &mut Context<Self>,
        op: impl FnOnce(&Path) -> Result<(), String> + Send + 'static,
        then: impl FnOnce(&mut Self, &mut Context<Self>) + 'static,
    ) {
        if self.busy.is_some() {
            return;
        }
        self.busy = Some(label.into());
        self.error = None;
        cx.notify();
        let root = self.root.clone();
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { op(&root) }).await;
            this.update(cx, |this, cx| {
                this.busy = None;
                match result {
                    Ok(()) => then(this, cx),
                    Err(e) => this.error = Some(e),
                }
                this.refresh(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn commit(&mut self, cx: &mut Context<Self>) {
        let message = self.message.read(cx).text().to_string();
        let amend = self.amend;
        // The same rule as the Commit button: Enter must not stage everything
        // only for the commit to fail on an empty message.
        let changed = self.status.as_ref().is_some_and(|s| s.staged().chain(s.unstaged()).next().is_some());
        if message.trim().is_empty() || self.busy.is_some() || !(amend || changed) {
            return;
        }
        // Nothing staged: commit everything, like most git UIs do.
        let stage_all = self.status.as_ref().is_some_and(|s| s.staged().next().is_none());
        self.run(
            if amend { "Amending" } else { "Committing" },
            cx,
            move |root| {
                if stage_all && !amend {
                    git::stage_all(root)?;
                }
                git::commit(root, &message, amend).map(drop)
            },
            |this, cx| {
                this.amend = false;
                this.message.update(cx, |m, cx| m.set_text("", cx));
            },
        );
    }

    fn toggle_amend(&mut self, cx: &mut Context<Self>) {
        self.amend = !self.amend;
        cx.notify();
        // Start from the last message, the usual reason to amend being a typo in it.
        if self.amend && self.message.read(cx).text().trim().is_empty() {
            let root = self.root.clone();
            cx.spawn(async move |this, cx| {
                let message = cx.background_spawn(async move { git::last_message(&root) }).await;
                this.update(cx, |this, cx| {
                    if let Ok(m) = message
                        && this.amend
                    {
                        this.message.update(cx, |input, cx| input.set_text(m, cx));
                    }
                })
                .ok();
            })
            .detach();
        }
    }

    fn create_branch(&mut self, cx: &mut Context<Self>) {
        let name = self.new_branch.read(cx).text().trim().to_string();
        if name.is_empty() {
            return;
        }
        self.branch_menu = false;
        self.run(
            "Creating branch",
            cx,
            move |root| git::create_branch(root, &name),
            |this, cx| {
                this.new_branch.update(cx, |i, cx| i.set_text("", cx));
            },
        );
    }
}

impl Render for GitView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx).clone();
        let top = self.render_top_bar(window, cx);

        let tab_button = |id: &'static str, label: String, tab: Tab, cx: &mut Context<Self>| {
            let selected = self.tab == tab;
            let hover = t.hover;
            div()
                .id(id)
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .h(px(26.))
                .rounded_md()
                .text_sm()
                .cursor_pointer()
                .text_color(if selected { t.text } else { t.text_muted })
                .when(selected, |d| d.font_weight(FontWeight::MEDIUM))
                .relative()
                .child(row_wash(selected, hover, t.active))
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.tab = tab;
                    cx.notify();
                }))
        };
        let changed = self.status.as_ref().map_or(0, |s| s.files.len());
        let tabs = div()
            .flex()
            .flex_none()
            .gap_1()
            .p_1p5()
            .border_b_1()
            .border_color(t.border_variant)
            .child(tab_button(
                "git-tab-changes",
                if changed > 0 { format!("Changes ({changed})") } else { "Changes".into() },
                Tab::Changes,
                cx,
            ))
            .child(tab_button("git-tab-history", "History".into(), Tab::History, cx));
        let left = match self.tab {
            Tab::Changes => self.render_changes(window, cx).into_any_element(),
            Tab::History => self.render_history(cx).into_any_element(),
        };

        let error = self.error.clone().map(|e| {
            div()
                .flex()
                .flex_none()
                .items_start()
                .gap_2()
                .px_3()
                .py_1p5()
                .border_b_1()
                .border_color(t.border_variant)
                .bg(t.error.opacity(0.08))
                .child(icon(Icon::Dot, t.error).size_3().mt_1())
                .child(div().flex_1().min_w_0().text_sm().text_color(t.text).child(e))
                .child(icon_button("git-dismiss-error", Icon::Close, cx).on_click(cx.listener(|this, _, _, cx| {
                    this.error = None;
                    cx.notify();
                })))
        });
        let conflicts = self.status.as_ref().is_some_and(Status::has_conflicts).then(|| {
            div()
                .flex_none()
                .px_3()
                .py_1p5()
                .border_b_1()
                .border_color(t.border_variant)
                .bg(t.warning.opacity(0.08))
                .text_sm()
                .child("Conflicts: resolve the files marked ! in your editor, then stage them to continue.")
        });

        div().flex().flex_col().size_full().child(top).children(error).children(conflicts).child(
            div()
                .flex()
                .flex_1()
                .min_h_0()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .w(px(340.))
                        .flex_none()
                        .bg(t.panel)
                        .border_r_1()
                        .border_color(t.border)
                        .child(tabs)
                        .child(div().flex().flex_col().flex_1().min_h_0().child(left)),
                )
                .child(div().flex().flex_col().flex_1().min_w_0().bg(t.editor).child(self.render_diff(cx))),
        )
    }
}
