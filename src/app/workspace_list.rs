//! A workspace's row in the Sessions tree: its name, branch/folder mark, rolled-up agent
//! lamp, fold toggle, "+ New session" button, and right-click menu.

use std::path::{Path, PathBuf};

use gpui::{Anchor, AnyElement, Context, MouseButton, MouseDownEvent, Window, div, prelude::*};
use kynex_core::layout::PaneId;

use super::KynexApp;
use super::menus::Menu;
use crate::theme::{mono, theme};
use crate::ui::wash::row_wash;
use crate::ui::{Icon, Lamp, Tooltip, icon, icon_button, lamp, lamp_count, px};

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

pub(super) struct WorkspaceRow {
    pub path: PathBuf,
    pub label: String,
    pub branch: Option<String>,
    pub drift: Option<String>,
    pub tip: String,
    pub selected: bool,
    pub open: bool,
    pub count: Option<usize>,
    pub lamps: Vec<Lamp>,
    /// The `agent_cli` pane in this workspace that most wants attention, if any.
    pub wants_you: Option<PaneId>,
}

impl KynexApp {
    fn new_session_in(&mut self, path: &Path, here: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !here {
            self.select_workspace(path, cx);
            self.sync_tiles(window, cx);
        }
        self.new_session(window, cx);
    }

    fn toggle_tree_open(&mut self, path: &Path, cx: &mut Context<Self>) {
        self.edit_groups(path, |g| g.tree_open = !g.tree_open);
        cx.notify();
    }

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
        let lamp_el = match wants_you.filter(|_| !open) {
            Some(pane) => div()
                .id(("workspace-lamp-go", n))
                .flex_none()
                .cursor_pointer()
                .child(lamp_el)
                .tooltip(Tooltip::text("Go to the agent that wants you"))
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.go_to_agent(pane, window, cx);
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
                    .font_weight(if selected { gpui::FontWeight::SEMIBOLD } else { gpui::FontWeight::MEDIUM })
                    .text_color(t.text)
                    .child(w.label.clone()),
            )
            .children(
                w.count
                    .filter(|&c| c > 0)
                    .map(|c| div().flex_none().text_xs().text_color(t.text_placeholder).child(c.to_string())),
            )
            .children(w.branch.clone().map(|b| {
                let drift = w.drift.clone();
                div()
                    .id(("workspace-branch", n))
                    .flex_none()
                    .max_w(px(100.))
                    .truncate()
                    .font_family(mono())
                    .text_xs()
                    .text_color(if drift.is_some() { t.warning } else { t.text_placeholder })
                    .child(b)
                    .when_some(drift, |d, tip| d.tooltip(Tooltip::text(tip)))
            }))
            .child(
                icon_button(("workspace-new-session", n), Icon::Plus, cx)
                    .when(!selected, |b| b.invisible().group_hover("workspace-row", |s| s.visible()))
                    .tooltip(Tooltip::text("New session in this workspace"))
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
                    cx.stop_propagation();
                    this.open_menu(Menu::Workspace(menu_path.clone()), ev.position, Anchor::TopLeft, cx)
                }),
            )
            .into_any_element()
    }
}
