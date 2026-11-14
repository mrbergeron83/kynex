//! A workspace's row in the sidebar's tree, as in herdr: its name, its branch or folder
//! mark, and a lamp rolled up from its sessions (with how many have news). It folds its
//! sessions away or opens them, and has "New session" and the workspace's menu. Clicking
//! it shows the workspace; clicking a lamp that wants you goes to that session.

use std::path::{Path, PathBuf};

use gpui::{Anchor, AnyElement, Context, MouseButton, MouseDownEvent, Window, div, prelude::*};
use kynex_core::live::Key;
use kynex_core::workspace;

use super::KynexApp;
use super::menus::Menu;
use crate::theme::{mono, theme};
use crate::ui::wash::row_wash;
use crate::ui::{Icon, Lamp, Tooltip, icon, icon_button, lamp, lamp_count, px};

/// A worktree's changes in words, for a tooltip: `3 files +42 -7, 2 ahead, 1 behind`.
pub(super) fn stats_text(w: &kynex_core::git::Worktree) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(n) = w.changes.filter(|&n| n > 0) {
        let files = format!("{n} file{}", if n == 1 { "" } else { "s" });
        parts.push(match w.lines {
            Some((added, removed)) => format!("{files} +{added} -{removed}"),
            None => files,
        });
    }
    if let Some((ahead, behind)) = w.upstream {
        if ahead > 0 {
            parts.push(format!("{ahead} ahead"));
        }
        if behind > 0 {
            parts.push(format!("{behind} behind"));
        }
    }
    (!parts.is_empty()).then(|| parts.join(", "))
}

/// A workspace's row as the tree draws it.
pub(super) struct WorkspaceRow {
    pub path: PathBuf,
    pub label: String,
    /// Its branch when the name doesn't say it already, or "folder" / "missing".
    pub branch: Option<String>,
    pub tip: String,
    pub selected: bool,
    /// Its sessions are drawn under it.
    pub open: bool,
    /// The lamps of what runs there: its open sessions and agent panes.
    pub lamps: Vec<Lamp>,
    /// The open session that most wants you, if one does (needs you, or finished).
    pub wants_you: Option<Key>,
}

impl KynexApp {
    /// Shows the open session `key`: its workspace comes up if it's another one, and its pane
    /// takes the focus.
    pub(super) fn focus_open_session(&mut self, key: Key, window: &mut Window, cx: &mut Context<Self>) {
        let Some(project) = self.workspace_project() else { return };
        let Some(cwd) = self.session_by_key(key).map(|s| s.cwd.clone()) else { return };
        let here = self.code.get(&project).and_then(|l| l.current()).map(|s| s.cwd.clone());
        if !here.is_some_and(|h| workspace::same_folder(&h, &cwd)) {
            self.select_workspace(&cwd, cx);
        }
        if let Some(live) = self.code.get_mut(&project) {
            live.focus(key);
        }
        self.focus_input(window, cx);
        cx.notify();
    }

    /// A new session in the workspace at `path`, which comes up first if it isn't shown.
    fn new_session_in(&mut self, path: &Path, here: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !here {
            self.select_workspace(path, cx);
        }
        self.new_session(window, cx);
    }

    /// Opens a folded workspace in the tree, or folds an open one (not the selected one,
    /// which is always open).
    fn toggle_tree_open(&mut self, path: &Path, cx: &mut Context<Self>) {
        self.edit_groups(path, |g| g.tree_open = !g.tree_open);
        cx.notify();
    }

    /// The row of workspace number `n`.
    pub(super) fn render_workspace_row(&mut self, n: usize, w: &WorkspaceRow, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx).clone();
        let colour = self.workspace_project().and_then(|p| self.project_colour(&p, &t)).unwrap_or(t.accent);
        let news = w.lamps.iter().filter(|l| **l != Lamp::Idle).count();
        let state = Lamp::most_urgent(w.lamps.iter().copied());
        let (path, menu_path, fold_path, new_path) = (w.path.clone(), w.path.clone(), w.path.clone(), w.path.clone());
        let (selected, open, wants_you) = (w.selected, w.open, w.wants_you);
        let lamp_el = match news {
            0 | 1 => lamp(("workspace-lamp", n), state, cx),
            n_news => lamp_count(("workspace-lamp", n), state, n_news, cx),
        };
        // Folded, a lamp that wants you goes to that session.
        let lamp_el = match wants_you.filter(|_| !open) {
            Some(key) => div()
                .id(("workspace-lamp-go", n))
                .flex_none()
                .cursor_pointer()
                .child(lamp_el)
                .tooltip(Tooltip::text("Go to the session that wants you"))
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.focus_open_session(key, window, cx);
                }))
                .into_any_element(),
            None => lamp_el,
        };
        let chevron = div()
            .id(("workspace-fold", n))
            .flex_none()
            .size(px(14.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_sm()
            .when(!selected, |d| d.cursor_pointer().hover(|s| s.bg(t.hover)))
            .child(
                icon(if open { Icon::ChevronDown } else { Icon::ChevronRight }, t.text_placeholder)
                    .size(px(10.))
                    .when(selected, |i| i.opacity(0.4)),
            )
            .when(!selected, |d| d.tooltip(Tooltip::text(if open { "Fold its sessions" } else { "Show its sessions" })))
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                if !selected {
                    this.toggle_tree_open(&fold_path, cx);
                }
            }));
        div()
            .id(("workspace", n))
            .flex()
            .items_center()
            .gap_1p5()
            .min_h(px(28.))
            .pl_1()
            .pr_1()
            .py_0p5()
            .rounded_md()
            .cursor_pointer()
            .text_sm()
            .relative()
            .group("workspace-row")
            .child(row_wash(false, t.hover, t.selection))
            .when(selected, |d| {
                d.child(div().absolute().left_0().top_1().bottom_1().w(px(2.)).rounded_full().bg(colour))
            })
            .child(chevron)
            .child(lamp_el)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(if selected { t.text } else { t.text_muted })
                    .child(w.label.clone()),
            )
            .children(w.branch.clone().map(|b| {
                div()
                    .flex_none()
                    .max_w(px(100.))
                    .truncate()
                    .font_family(mono())
                    .text_xs()
                    .text_color(t.text_placeholder)
                    .child(b)
            }))
            .child(
                icon_button(("workspace-new-session", n), Icon::Plus, cx)
                    .invisible()
                    .group_hover("workspace-row", |s| s.visible())
                    .tooltip(Tooltip::text("New session here"))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.new_session_in(&new_path, selected, window, cx);
                    })),
            )
            .tooltip(Tooltip::text(w.tip.clone()))
            .on_click(cx.listener(move |this, _, _, cx| this.select_workspace(&path, cx)))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                    // Not the pane's menu as well.
                    cx.stop_propagation();
                    this.open_menu(Menu::Workspace(menu_path.clone()), ev.position, Anchor::TopLeft, cx)
                }),
            )
            .into_any_element()
    }
}
