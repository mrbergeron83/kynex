//! The project's git worktrees, at the bottom of the sessions panel: one row
//! per worktree with its branch, changed files, distance from main, and
//! whether a session is working in it. Clicking one opens it as a project tab.

use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui::{AnyElement, Context, Entity, FontWeight, div, prelude::*, px};
use kynex_core::git::{self, Worktree};
use kynex_core::paths::project_name;
use kynex_core::session::{Item, Kind};

use super::KynexApp;
use super::state::{Active, View};
use crate::theme::theme;
use crate::ui::{Icon, Tooltip, icon, spinner};

/// Worktrees change behind our back (Claude makes and removes them), so re-read often.
const REFRESH_EVERY: Duration = Duration::from_secs(10);

/// A project's worktrees, re-read in the background while the project is open.
pub struct Worktrees {
    pub list: Vec<Worktree>,
}

impl Worktrees {
    fn new(root: PathBuf, cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this, cx| {
            loop {
                let root = root.clone();
                let list = cx.background_spawn(async move { git::list_worktrees(&root).unwrap_or_default() }).await;
                let alive = this.update(cx, |this, cx| {
                    if this.list != list {
                        this.list = list;
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
                cx.background_executor().timer(REFRESH_EVERY).await;
            }
        })
        .detach();
        Self { list: Vec::new() }
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
    pub(super) fn working_worktree(&mut self, cx: &mut Context<Self>) -> Option<Worktree> {
        let s = self.active_session().filter(|s| s.kind == Kind::Code)?;
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
        let list = self.worktrees(&cwd, cx).read(cx).list.clone();
        inputs.iter().find_map(|input| git::worktree_in(input, &list)).filter(|w| w.path != cwd).cloned()
    }

    /// Opens a worktree as a tab, on its Git view, to look at its changes.
    pub(super) fn open_worktree_changes(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.add_project(path);
        if let Active::Project(i) = self.saved.active
            && let Some(p) = self.saved.projects.get_mut(i)
        {
            p.view = View::Git;
        }
        self.save();
        cx.notify();
    }

    /// The Worktrees section, or nothing when the repo has just the one.
    pub(super) fn render_worktrees(&mut self, cwd: &Path, cx: &mut Context<Self>) -> Option<AnyElement> {
        let list = self.worktrees(cwd, cx).read(cx).list.clone();
        if list.len() < 2 {
            return None;
        }
        let t = theme(cx).clone();
        let working = self.working_worktree(cx).map(|w| w.path);
        let rows = list.into_iter().enumerate().map(|(n, w)| {
            let current = w.path == cwd;
            let claude_here = working.as_ref() == Some(&w.path);
            let live = self.code.get(&w.path);
            let (busy, waiting) = live.map_or((false, false), |l| (l.busy(), l.waiting()));
            let hover = t.hover;
            let details = w.details();
            let tip = w.path.display().to_string();
            let meta = match (current, claude_here) {
                (true, _) => "This tab",
                (false, true) => "Claude is working here · click to open it as a tab",
                (false, false) => "Click to open it as a tab",
            };
            let path = w.path.clone();
            div()
                .id(("worktree", n))
                .flex()
                .items_center()
                .gap_1p5()
                .px_2()
                .py_1()
                .rounded_md()
                .text_sm()
                .when(current, |d| d.bg(t.active))
                .when(!current && !w.prunable, |d| d.cursor_pointer().hover(move |s| s.bg(hover)))
                .child(icon(Icon::GitBranch, if current { t.text } else { t.text_muted }).size_3p5())
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .truncate()
                                .text_color(if current { t.text } else { t.text_muted })
                                .when(w.prunable, |d| d.line_through())
                                .child(w.label().to_string()),
                        )
                        .child(div().truncate().text_xs().text_color(t.text_placeholder).child(
                            std::iter::once(project_name(&w.path)).chain(details).collect::<Vec<_>>().join(" · "),
                        )),
                )
                .when(claude_here, |d| {
                    d.child(
                        div()
                            .flex_none()
                            .px_1()
                            .rounded_sm()
                            .bg(t.accent.opacity(0.15))
                            .text_xs()
                            .text_color(t.accent)
                            .child("Claude"),
                    )
                })
                .when(waiting, |d| d.child(icon(Icon::Dot, t.warning).size_3().flex_none()))
                .when(busy && !waiting, |d| d.child(spinner(("worktree-spin", n), t.accent, px(12.))))
                .tooltip(Tooltip::with_meta(tip, meta))
                .when(!current && !w.prunable, |d| {
                    d.on_click(cx.listener(move |this, _, _, cx| {
                        this.add_project(path.clone());
                        cx.notify();
                    }))
                })
        });
        Some(
            div()
                .flex()
                .flex_col()
                .flex_none()
                .child(
                    div()
                        .px_3()
                        .pt_2()
                        .pb_1()
                        .text_xs()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(t.text_muted)
                        .child("Worktrees"),
                )
                .child(div().id("worktree-list").max_h(px(220.)).overflow_y_scroll().px_1p5().pb_1().children(rows))
                .into_any_element(),
        )
    }
}
