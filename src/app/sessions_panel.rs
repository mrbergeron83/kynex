//! The sessions panel: the sidebar's tree of workspaces and their sessions
//! (`session_tree.rs`), its header (filter, select, new workspace, add folder, new
//! session), and each workspace's list of past sessions, read off the UI thread.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::ui::px;
use gpui::{Anchor, AnyElement, Context, Focusable, MouseButton, Window, div, prelude::*};
use kynex_core::layout::PaneId;
use kynex_core::session::{self, Kind, SessionInfo};

use super::KynexApp;
use super::keys::{NewSession, ToggleSessions};
use super::menus::Menu;
use super::session_rows::OpenSession;
use super::session_tree::TreeCtx;
use crate::theme::theme;
use crate::ui::motion::grow;
use crate::ui::{Icon, Tooltip, icon_button, scrollbar, section_label};

/// A workspace's past sessions, re-read when its open sessions change (one starts,
/// gets its id, a reply starts or finishes) or something they show does.
pub struct SessionList {
    /// What it was read for.
    pub(super) read: ListRead,
    pub items: Vec<SessionInfo>,
    /// The repository's default branch: sessions on another one say theirs.
    pub(super) default_branch: Option<String>,
    /// Something it shows changed (a rename, a setting): read it again.
    stale: bool,
}

/// What a workspace's list is read again for when it changes.
#[derive(Clone, PartialEq)]
pub struct ListRead {
    pub(super) cwd: PathBuf,
    current: Option<String>,
    /// The workspace's open sessions and whether each is busy: titles and times
    /// move when a reply starts or finishes.
    live: Vec<(Option<String>, bool)>,
}

/// The lists' reads: the newest asked for and the newest shown, per folder.
#[derive(Default)]
pub(super) struct Reads {
    reading: HashMap<PathBuf, ListRead>,
    reads: HashMap<PathBuf, (u64, u64)>,
}

impl KynexApp {
    /// Sessions were deleted: they leave the lists now, and they're read again
    /// ([`Self::reread_sessions`]), showing the rest meanwhile.
    pub(super) fn relist_sessions(&mut self, gone: &[String]) {
        for l in self.sessions.values_mut() {
            l.items.retain(|i| !gone.contains(&i.id));
        }
        self.reread_sessions();
    }

    /// Reads a folder's past sessions off the UI thread (each transcript's ends); the
    /// tree keeps showing the last list until they're in.
    fn read_sessions(&mut self, read: ListRead, cx: &mut Context<Self>) {
        let cwd = read.cwd.clone();
        self.session_reads.reading.insert(cwd.clone(), read.clone());
        let reads = self.session_reads.reads.entry(cwd.clone()).or_default();
        reads.0 += 1;
        let n = reads.0;
        cx.spawn(async move |this, cx| {
            let (mut items, default_branch) = cx
                .background_spawn(async move { (session::list_sessions(&cwd), kynex_core::git::default_branch(&cwd)) })
                .await;
            this.update(cx, |this, cx| {
                // An older read than one already shown: drop it. A newer one on its way
                // still lets this one show meanwhile.
                let reads = this.session_reads.reads.entry(read.cwd.clone()).or_default();
                if n < reads.1 {
                    return;
                }
                reads.1 = n;
                if this.session_reads.reading.get(&read.cwd) == Some(&read) {
                    this.session_reads.reading.remove(&read.cwd);
                }
                // A name given by hand wins, and counts as named, so none is generated.
                for i in &mut items {
                    if let Some(name) = this.given_name(&read.cwd, &i.id) {
                        i.title = name;
                        i.named = true;
                    }
                }
                let cwd = read.cwd.clone();
                this.sessions.insert(cwd, SessionList { read, items, default_branch, stale: false });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Has every list read again, keeping it on screen meanwhile.
    pub(super) fn reread_sessions(&mut self) {
        for l in self.sessions.values_mut() {
            l.stale = true;
        }
    }

    /// Runs `f` once the list of `cwd` is read: now if it is, else after reading it off the
    /// UI thread (a folded workspace's isn't). The tree reads it again when it's drawn.
    pub(super) fn with_list(
        &mut self,
        cwd: &Path,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut Self, &mut Context<Self>) + 'static,
    ) {
        if self.sessions.contains_key(cwd) {
            return f(self, cx);
        }
        let dir = cwd.to_path_buf();
        cx.spawn(async move |this, cx| {
            let read = dir.clone();
            let (items, default_branch) = cx
                .background_spawn(
                    async move { (session::list_sessions(&read), kynex_core::git::default_branch(&read)) },
                )
                .await;
            this.update(cx, |this, cx| {
                let read = ListRead { cwd: dir.clone(), current: None, live: Vec::new() };
                this.sessions.entry(dir).or_insert(SessionList { read, items, default_branch, stale: true });
                f(this, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Has the list of `cwd` read if it isn't, or is out of date for its `open` sessions
    /// and the shown one (`current`, when it's there). Returns whether it's still to come.
    pub(super) fn want_list(
        &mut self,
        cwd: &Path,
        current: Option<String>,
        open: &[OpenSession],
        cx: &mut Context<Self>,
    ) -> bool {
        let live = open.iter().map(|o| (o.id.clone(), o.busy)).collect();
        let read = ListRead { cwd: cwd.to_path_buf(), current, live };
        let fresh = self.sessions.get(cwd).is_some_and(|l| !l.stale && l.read == read);
        if !fresh && self.session_reads.reading.get(cwd) != Some(&read) {
            self.read_sessions(read, cx);
        }
        !self.sessions.contains_key(cwd)
    }

    /// The panel with its drag-to-resize right edge, or nothing while collapsed. Only where
    /// there are no panes to hold the tree: chat.
    pub(super) fn render_sessions_column(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let open = !self.saved.sessions_collapsed;
        let leaving = self.exits.sessions.track_flag(open, window, cx);
        if !open && leaving.is_none() {
            return None;
        }
        let t = theme(cx).clone();
        // Slides open when shown (Ctrl+B) and shut when hidden; the width is the dragged one.
        Some(grow(
            "sessions-in",
            div()
                .relative()
                .flex_none()
                .h_full()
                .bg(t.panel)
                .border_r_1()
                .border_color(t.border)
                .child(self.render_sessions_panel(window, cx, None))
                .child(
                    div()
                        .id("sessions-resize")
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .right(px(-3.))
                        .w(px(6.))
                        .cursor_col_resize()
                        .hover(|s| s.bg(t.border_focused.opacity(0.6)))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.resizing_sessions = true;
                                cx.stop_propagation();
                            }),
                        ),
                ),
            // Saved at normal size, so it grows with the zoom like what's in it.
            self.saved.sessions_width * self.saved.zoom,
            leaving,
        ))
    }

    /// The tree, in the sidebar (`pane` is `None`) or in a pane.
    pub(super) fn render_sessions_panel(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        pane: Option<PaneId>,
    ) -> impl IntoElement + use<> {
        let t = theme(cx).clone();
        let Some(live) = self.live_mut() else { return div() };
        let Some(s) = live.current() else { return div() };
        let (kind, cwd, current, current_uid) = (s.kind, s.cwd.clone(), s.session_id.clone(), s.uid);
        // The title filter, while it's open.
        let query = if self.filtering_sessions {
            self.session_filter.read(cx).text().trim().to_lowercase()
        } else {
            String::new()
        };
        let cursor = self.organize.cursor.clone().filter(|_| self.sessions_focus.is_focused(window));
        let ctx = TreeCtx { kind, cwd: cwd.clone(), current: current.clone(), current_uid, query, cursor };
        let tree = self.render_tree(&ctx, cx);
        // The rows in the order drawn, for Shift+click runs, "All" and the list's keys.
        self.organize.shown = tree.order.clone();
        // Picks stay in one workspace: the one they were made in.
        let pick_cwd = self.organize.selection_cwd.clone().unwrap_or_else(|| cwd.clone());
        let in_pick_folder: Vec<String> =
            tree.order.iter().filter(|(p, _)| *p == pick_cwd).map(|(_, id)| id.clone()).collect();
        self.sync_picks(&pick_cwd, &in_pick_folder);
        let picking = self.organize.selection.on;
        // A project with workspaces to add: new worktrees in a git repository, folders anywhere.
        let project = self.workspace_project().filter(|_| kind == Kind::Code);
        let repo = project.as_ref().is_some_and(|p| !self.worktrees(p, cx).read(cx).list.is_empty());

        let header = div()
            .flex()
            .flex_none()
            .items_center()
            .h(px(36.))
            .pl_3()
            .pr_1p5()
            .gap_1()
            .child(section_label("Sessions", cx).flex_1())
            .child(
                icon_button("filter-sessions", Icon::Search, cx)
                    .when(self.filtering_sessions, |b| b.bg(t.active))
                    .tooltip(Tooltip::text("Filter by title"))
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_session_filter(window, cx))),
            )
            .child({
                let cwd = cwd.clone();
                icon_button("select-sessions", Icon::Check, cx)
                    .when(picking, |b| b.bg(t.active))
                    .tooltip(Tooltip::text(if cfg!(target_os = "macos") {
                        "Select sessions (or Cmd+click, Shift+click)"
                    } else {
                        "Select sessions (or Ctrl+click, Shift+click)"
                    }))
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_picking(cwd.clone(), cx)))
            })
            .when(project.is_some(), |d| {
                d.child(
                    icon_button("add-workspace-folder", Icon::FolderPlus, cx)
                        .tooltip(Tooltip::text("Add a folder as a workspace: scripts, notes"))
                        .on_click(cx.listener(|this, _, _, cx| this.add_folder_workspace(cx))),
                )
            })
            .when(repo, |d| {
                d.child(
                    icon_button("new-workspace", Icon::GitBranch, cx)
                        .tooltip(Tooltip::text("New workspace: a worktree on a branch"))
                        .on_click(cx.listener(|this, _, window, cx| this.open_new_workspace(window, cx))),
                )
            })
            .child(
                icon_button("new-session", Icon::Plus, cx)
                    .when(current.is_none(), |b| b.opacity(0.4))
                    .when(current.is_none(), |b| b.tooltip(Tooltip::text("Already a new session")))
                    .when(current.is_some(), |b| b.tooltip(Tooltip::for_action("New session", NewSession, "")))
                    .on_click(cx.listener(|this, _, window, cx| this.new_session(window, cx))),
            )
            .child(
                icon_button("sessions-more", Icon::ChevronDown, cx)
                    .tooltip(Tooltip::text("Sessions options"))
                    .on_click(cx.listener(|this, ev: &gpui::ClickEvent, _, cx| {
                        this.open_menu(Menu::SessionsPanel, ev.position(), Anchor::TopLeft, cx)
                    })),
            )
            .child(match pane {
                // In a pane, the panel's own button closes the pane.
                Some(id) => icon_button("close-sessions-pane", Icon::Close, cx)
                    .tooltip(Tooltip::text("Close pane"))
                    .on_click(cx.listener(move |this, _, window, cx| this.close_pane_id(id, window, cx))),
                None => icon_button("hide-sessions", Icon::PanelLeft, cx)
                    .tooltip(Tooltip::for_action("Hide sessions", ToggleSessions, ""))
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_sessions(&ToggleSessions, window, cx))),
            });

        let filter = self.filtering_sessions.then(|| {
            div().flex_none().px_1p5().pb_1().child(
                div()
                    .h(px(26.))
                    .px_2()
                    .flex()
                    .items_center()
                    .rounded_md()
                    .border_1()
                    .border_color(t.border)
                    .bg(t.editor)
                    .text_sm()
                    .child(self.session_filter.clone())
                    // Esc closes the filter and shows every session again.
                    .on_key_down(cx.listener(|this, ev: &gpui::KeyDownEvent, window, cx| {
                        if ev.keystroke.key == "escape" {
                            cx.stop_propagation();
                            this.toggle_session_filter(window, cx);
                        }
                    })),
            )
        });
        let forms = if project.is_some() { self.render_workspace_forms(cx) } else { Vec::new() };
        // Last in the tree: the sessions of workspaces removed through Kynex.
        let removed = if project.is_some() { self.render_removed_workspaces(cx) } else { None };

        let scroll = scrollbar::handle("session-list");
        let list = div()
            .id("session-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&scroll)
            .px_1p5()
            .pb_1()
            // Its own keys while it has the focus: ↑ / ↓, Enter, F2, Delete, Shift+Delete.
            .key_context("SessionList")
            .track_focus(&self.sessions_focus)
            .on_action(cx.listener(Self::list_up))
            .on_action(cx.listener(Self::list_down))
            .on_action(cx.listener(Self::list_open))
            .on_action(cx.listener(Self::list_rename))
            .on_action(cx.listener(Self::list_archive))
            .on_action(cx.listener(Self::list_delete))
            .children(forms)
            .children(tree.rows)
            .children(removed);
        let picks = self.render_pick_bar(kind, pick_cwd, in_pick_folder, tree.busy, cx);
        div()
            .flex()
            .flex_col()
            .size_full()
            .child(header)
            .children(filter)
            .child(scrollbar::vertical_over("session-list-bar", scroll, list, |d| d.flex_1().min_h_0()))
            .children(picks)
    }

    /// Opens the title filter, or closes it and shows every session again.
    fn toggle_session_filter(&mut self, window: &mut gpui::Window, cx: &mut Context<Self>) {
        self.filtering_sessions = !self.filtering_sessions;
        if self.filtering_sessions {
            self.session_filter.read(cx).focus_handle(cx).focus(window, cx);
        } else {
            self.session_filter.update(cx, |f, cx| f.set_text("", cx));
            self.focus_input(window, cx);
        }
        cx.notify();
    }
}
