//! The workspaces in the sidebar, as in herdr: one block per workspace of the selected
//! project (its main tree, worktrees and added folders) with its name, branch or folder
//! mark and a state lamp rolled up from its sessions, and under it the sessions open in its
//! panes, each with its own lamp. Clicking a workspace shows it; clicking a session
//! focuses its pane. The selected workspace's past sessions are the list below.

use gpui::{Anchor, AnyElement, Context, MouseButton, MouseDownEvent, Window, div, prelude::*};
use kynex_core::live::Key;
use kynex_core::session::{Item, Session};
use kynex_core::workspace;

use super::KynexApp;
use super::menus::Menu;
use super::workspaces::plain;
use crate::theme::{mono, theme};
use crate::ui::wash::row_wash;
use crate::ui::{Icon, Lamp, Tooltip, icon_button, lamp, px, section_label};

/// A worktree's changes in words, for a tooltip: `3 files +42 -7, 2 ahead, 1 behind`.
fn stats_text(w: &kynex_core::git::Worktree) -> Option<String> {
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

impl KynexApp {
    /// What a session is called in the sidebar: its name; until it has one, the first words
    /// of its first message.
    fn open_session_label(&self, s: &Session) -> String {
        s.session_id
            .as_deref()
            .and_then(|id| self.session_name(s.uid, id))
            .or_else(|| {
                s.items.iter().find_map(|i| match i {
                    Item::User { text, .. } if !text.trim().is_empty() => {
                        Some(kynex_core::fmt::first_words(text, kynex_core::titles::FIRST_WORDS))
                    }
                    _ => None,
                })
            })
            .unwrap_or_else(|| "New session".to_string())
    }

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

    /// The sessions open in the workspace at `path`: the ones in its panes (every tab),
    /// else, before it has been shown, those running there.
    fn open_in_workspace(&self, project: &std::path::Path, path: &std::path::Path) -> Vec<Key> {
        let Some(live) = self.code.get(project) else { return Vec::new() };
        let open = |k: &Key| live.sessions.iter().any(|s| s.uid == *k);
        match self.tiles.iter().find(|(f, _)| workspace::same_folder(f, path)) {
            Some((_, t)) => t.sessions().into_iter().filter(open).collect(),
            None => live
                .sessions
                .iter()
                .filter(|s| s.kind == kynex_core::session::Kind::Code && workspace::same_folder(&s.cwd, path))
                .map(|s| s.uid)
                .collect(),
        }
    }

    /// The section: a header with "New workspace" (git repositories) and "Add folder", and a
    /// block per workspace.
    pub(super) fn render_workspaces(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let _t = kynex_core::trace::span("render_workspaces");
        let project = self.workspace_project()?;
        // A git repository, where a worktree can be made: git read it once the worktree
        // list isn't empty. Any project can add plain folders.
        let repo = !self.worktrees(&project, cx).read(cx).list.is_empty();
        let list = self.workspaces_of(&project, cx);
        let t = theme(cx).clone();
        let cwd = self.code.get(&project)?.current()?.cwd.clone();
        let shown = self.code.get(&project)?.current().map(|s| s.uid);
        let colour = self.project_colour(&project, &t).unwrap_or(t.accent);
        // The shown session's workspace, else the saved selection, else the main tree.
        let saved = self.saved_workspaces(&project).selected_in(&plain(&list)).path.clone();
        let selected =
            list.iter().position(|(c, _)| c.at(&cwd)).or_else(|| list.iter().position(|(c, _)| c.at(&saved)));
        let selected = selected.unwrap_or(0);
        let blocks: Vec<AnyElement> = list
            .iter()
            .enumerate()
            .map(|(n, (c, worktree))| {
                let open = self.open_in_workspace(&project, &c.path);
                let live = self.code.get(&project);
                let sessions: Vec<&Session> =
                    open.iter().filter_map(|k| live.and_then(|l| l.sessions.iter().find(|s| s.uid == *k))).collect();
                let agents = self.agents_in(&c.path);
                let lamp_state = Lamp::most_urgent(
                    sessions
                        .iter()
                        .map(|s| Lamp::of(s.busy, s.waiting(), s.finished))
                        .chain(agents.iter().map(|(_, _, l)| *l)),
                );
                let is_selected = n == selected;
                let path = c.path.clone();
                let menu_path = c.path.clone();
                let tip = match worktree.as_ref().and_then(stats_text) {
                    Some(stats) => format!("{}\n{stats}", c.path.display()),
                    None => c.path.display().to_string(),
                };
                // The branch, when the folder's name doesn't already say it.
                let branch = if c.folder {
                    Some(if c.path.is_dir() { "folder" } else { "missing" }.to_string())
                } else {
                    c.branch.clone().filter(|b| *b != c.label)
                };
                let rows: Vec<AnyElement> = sessions
                    .iter()
                    .enumerate()
                    .map(|(i, s)| {
                        let key = s.uid;
                        let on_show = is_selected && Some(key) == shown;
                        let state = Lamp::of(s.busy, s.waiting(), s.finished);
                        div()
                            .id(("workspace-session", n * 1000 + i))
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .h(px(24.))
                            .pl_5()
                            .pr_1()
                            .rounded_md()
                            .cursor_pointer()
                            .text_sm()
                            .relative()
                            .child(row_wash(on_show, t.hover, t.selection))
                            .child(lamp(("workspace-session-lamp", n * 1000 + i), state, cx))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(if on_show { t.text } else { t.text_muted })
                                    .child(self.open_session_label(s)),
                            )
                            .on_click(cx.listener(move |this, _, window, cx| this.focus_open_session(key, window, cx)))
                            .into_any_element()
                    })
                    // Agent panes, after the sessions: their lamps read off their screens.
                    .chain(agents.iter().enumerate().map(|(i, (pane, name, state))| {
                        let (pane, ws) = (*pane, c.path.clone());
                        div()
                            .id(("workspace-agent", n * 1000 + i))
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .h(px(24.))
                            .pl_5()
                            .pr_1()
                            .rounded_md()
                            .cursor_pointer()
                            .text_sm()
                            .hover(|s| s.bg(t.hover))
                            .child(lamp(("workspace-agent-lamp", n * 1000 + i), *state, cx))
                            .child(div().flex_1().min_w_0().truncate().text_color(t.text_muted).child(name.clone()))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.select_workspace(&ws, cx);
                                this.sync_tiles(window, cx);
                                this.focus_pane(pane, window, cx);
                                this.focus_input(window, cx);
                            }))
                            .into_any_element()
                    }))
                    .collect();
                div()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .id(("workspace", n))
                            .flex()
                            .flex_col()
                            .justify_center()
                            .min_h(px(28.))
                            .pl_2()
                            .pr_1()
                            .py_0p5()
                            .rounded_md()
                            .cursor_pointer()
                            .text_sm()
                            .relative()
                            .child(row_wash(is_selected && open.is_empty(), t.hover, t.selection))
                            .when(is_selected, |d| {
                                d.child(
                                    div().absolute().left_0().top_1().bottom_1().w(px(2.)).rounded_full().bg(colour),
                                )
                            })
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1p5()
                                    .child(lamp(("workspace-lamp", n), lamp_state, cx))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .font_weight(gpui::FontWeight::MEDIUM)
                                            .text_color(if is_selected { t.text } else { t.text_muted })
                                            .child(c.label.clone()),
                                    )
                                    .children(branch.map(|b| {
                                        div()
                                            .flex_none()
                                            .max_w(px(100.))
                                            .truncate()
                                            .font_family(mono())
                                            .text_xs()
                                            .text_color(t.text_placeholder)
                                            .child(b)
                                    })),
                            )
                            .tooltip(Tooltip::text(tip))
                            .on_click(cx.listener(move |this, _, _, cx| this.select_workspace(&path, cx)))
                            .on_mouse_down(
                                MouseButton::Right,
                                cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                                    this.open_menu(Menu::Workspace(menu_path.clone()), ev.position, Anchor::TopLeft, cx)
                                }),
                            ),
                    )
                    .children(rows)
                    .into_any_element()
            })
            .collect();
        let forms = self.render_workspace_forms(cx);
        Some(
            div()
                .flex()
                .flex_col()
                .flex_none()
                .px_1p5()
                .pb_1p5()
                .border_b_1()
                .border_color(t.border_variant)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .child(section_label("Workspaces", cx).flex_1().px_2().pt_1p5().pb_1())
                        .child(
                            icon_button("add-workspace-folder", Icon::FolderPlus, cx)
                                .tooltip(Tooltip::text("Add a folder as a workspace: scripts, notes"))
                                .on_click(cx.listener(|this, _, _, cx| this.add_folder_workspace(cx))),
                        )
                        .when(repo, |d| {
                            d.child(
                                icon_button("new-workspace", Icon::Plus, cx)
                                    .tooltip(Tooltip::text("New workspace: a worktree on a branch"))
                                    .on_click(cx.listener(|this, _, window, cx| this.open_new_workspace(window, cx))),
                            )
                        }),
                )
                .children(blocks)
                .children(forms)
                .into_any_element(),
        )
    }
}
