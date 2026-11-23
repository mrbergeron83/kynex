//! The Sessions & Workspaces tree pane: reads each workspace's past sessions off the UI
//! thread via `kynex_core::sessions` and renders the tree (`session_tree.rs`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime};

use crate::ui::px;
use gpui::{Anchor, Context, Focusable, Window, div, prelude::*};
use kynex_core::groups::SessionGroups;
use kynex_core::layout::PaneId;
use kynex_core::sessions::{self, SessionInfo};
use kynex_core::tree::{self, Sections};

use super::KynexApp;
use super::menus::Menu;
use super::session_rows::OpenSession;
use super::session_tree::TreeCtx;
use crate::theme::theme;
use crate::ui::{Icon, Tooltip, icon_button, scrollbar, section_label};

/// How long cached `tree::Sections` stay valid before re-checking the 7-day recent cutoff.
const SECTIONS_CACHE_TTL: Duration = Duration::from_secs(60);

struct CachedSections {
    groups_gen: u64,
    open_ids: Vec<String>,
    fresh: Vec<String>,
    at: Instant,
    sections: Rc<Sections>,
}

pub struct SessionList {
    pub(super) read: ListRead,
    pub items: Rc<Vec<SessionInfo>>,
    pub(super) index: Rc<HashMap<String, usize>>,
    pub(super) listed: Rc<Vec<(String, SystemTime)>>,
    pub(super) default_branch: Option<String>,
    stale: bool,
    sections_cache: Option<CachedSections>,
}

impl SessionList {
    fn new(read: ListRead, items: Vec<SessionInfo>, default_branch: Option<String>, stale: bool) -> Self {
        let index: HashMap<String, usize> = items.iter().enumerate().map(|(i, s)| (s.id.clone(), i)).collect();
        let listed: Vec<(String, SystemTime)> = items.iter().map(|i| (i.id.clone(), i.modified)).collect();
        Self {
            read,
            items: Rc::new(items),
            index: Rc::new(index),
            listed: Rc::new(listed),
            default_branch,
            stale,
            sections_cache: None,
        }
    }

    fn rebuild_indices(&mut self) {
        let index: HashMap<String, usize> = self.items.iter().enumerate().map(|(i, s)| (s.id.clone(), i)).collect();
        let listed: Vec<(String, SystemTime)> = self.items.iter().map(|i| (i.id.clone(), i.modified)).collect();
        self.index = Rc::new(index);
        self.listed = Rc::new(listed);
        self.sections_cache = None;
    }

    pub(super) fn sections(
        &mut self,
        groups_gen: u64,
        groups: &SessionGroups,
        open_ids: &[String],
        fresh: &[String],
    ) -> Rc<Sections> {
        if let Some(c) = &self.sections_cache
            && c.groups_gen == groups_gen
            && c.open_ids == open_ids
            && c.fresh == fresh
            && c.at.elapsed() < SECTIONS_CACHE_TTL
        {
            return c.sections.clone();
        }
        let sections = Rc::new(tree::sections(groups, &self.listed, open_ids, fresh, SystemTime::now()));
        self.sections_cache = Some(CachedSections {
            groups_gen,
            open_ids: open_ids.to_vec(),
            fresh: fresh.to_vec(),
            at: Instant::now(),
            sections: sections.clone(),
        });
        sections
    }
}

#[derive(Clone, PartialEq)]
pub struct ListRead {
    pub(super) cwd: PathBuf,
    current: Option<String>,
    live: Vec<(Option<String>, bool)>,
}

#[derive(Default)]
pub(super) struct Reads {
    reading: HashMap<PathBuf, ListRead>,
    reads: HashMap<PathBuf, (u64, u64)>,
}

impl KynexApp {
    pub(super) fn relist_sessions(&mut self, gone: &[String]) {
        for l in self.sessions.values_mut() {
            let before = l.items.len();
            Rc::make_mut(&mut l.items).retain(|i| !gone.contains(&i.id));
            if l.items.len() != before {
                l.rebuild_indices();
            }
        }
    }

    fn read_sessions(&mut self, read: ListRead, cx: &mut Context<Self>) {
        let cwd = read.cwd.clone();
        self.session_reads.reading.insert(cwd.clone(), read.clone());
        let reads = self.session_reads.reads.entry(cwd.clone()).or_default();
        reads.0 += 1;
        let n = reads.0;
        let saved = self.groups_for(&cwd).clone();
        cx.spawn(async move |this, cx| {
            let (mut items, default_branch, gone) = cx
                .background_spawn(async move {
                    let gone = sessions::gone_sessions(&cwd, &saved);
                    (sessions::list_sessions(&cwd), kynex_core::git::default_branch(&cwd), gone)
                })
                .await;
            this.update(cx, |this, cx| {
                let reads = this.session_reads.reads.entry(read.cwd.clone()).or_default();
                if n < reads.1 {
                    return;
                }
                reads.1 = n;
                if this.session_reads.reading.get(&read.cwd) == Some(&read) {
                    this.session_reads.reading.remove(&read.cwd);
                }
                if let Some(gone) = gone.filter(|g| !g.is_empty()) {
                    let open: Vec<String> = this
                        .tiles
                        .values()
                        .flat_map(|t| t.agent_panes())
                        .filter_map(|(_, m)| m.session_id.clone())
                        .chain(this.agent_instances.values().filter_map(|a| a.meta.session_id.clone()))
                        .chain(this.claude_chats.values().filter_map(|c| c.meta.session_id.clone()))
                        .collect();
                    this.edit_groups(&read.cwd, |g| {
                        g.drop_gone(|id| !gone.iter().any(|x| x == id), &open);
                    });
                }
                for i in &mut items {
                    if let Some(name) = this.given_name(&read.cwd, &i.id) {
                        i.title = name;
                    }
                }
                let cwd = read.cwd.clone();
                this.sessions.insert(cwd, SessionList::new(read, items, default_branch, false));
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn reread_sessions(&mut self) {
        for l in self.sessions.values_mut() {
            l.stale = true;
        }
    }

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
                    async move { (sessions::list_sessions(&read), kynex_core::git::default_branch(&read)) },
                )
                .await;
            this.update(cx, |this, cx| {
                let read = ListRead { cwd: dir.clone(), current: None, live: Vec::new() };
                this.sessions.entry(dir).or_insert_with(|| SessionList::new(read, items, default_branch, true));
                f(this, cx);
            })
            .ok();
        })
        .detach();
    }

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

    pub(super) fn render_sessions_panel(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        pane: Option<PaneId>,
    ) -> impl IntoElement + use<> {
        let t = theme(cx).clone();
        let Some((_, cwd)) = self.tiling_scope() else { return div() };
        let current = self.focused_session_id();
        let current_agent = self.focused_agent_id();
        let query = if self.filtering_sessions {
            self.session_filter.read(cx).text().trim().to_lowercase()
        } else {
            String::new()
        };
        let cursor = self.organize.cursor.clone().filter(|_| self.sessions_focus.is_focused(window));
        let ctx = TreeCtx { cwd: cwd.clone(), current, current_agent, query, cursor };
        let tree = self.render_tree(&ctx, cx);
        self.organize.shown = tree.order.clone();
        let pick_cwd = self.organize.selection_cwd.clone().unwrap_or_else(|| cwd.clone());
        let in_pick_folder: Vec<String> =
            tree.order.iter().filter(|(p, _)| *p == pick_cwd).map(|(_, id)| id.clone()).collect();
        self.sync_picks(&pick_cwd, &in_pick_folder);
        let picking = self.organize.selection.on;

        let header = div()
            .id("sessions-header")
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
            .child(icon_button("add-workspace", Icon::FolderPlus, cx).tooltip(Tooltip::text("Add workspace")).on_click(
                cx.listener(|this, ev: &gpui::ClickEvent, _, cx| {
                    this.open_menu(Menu::AddWorkspace, ev.position(), Anchor::TopLeft, cx)
                }),
            ))
            .child(
                icon_button("sessions-more", Icon::ChevronDown, cx)
                    .tooltip(Tooltip::text("Sessions options"))
                    .on_click(cx.listener(|this, ev: &gpui::ClickEvent, _, cx| {
                        this.open_menu(Menu::SessionsPanel, ev.position(), Anchor::TopLeft, cx)
                    })),
            )
            .when_some(pane, |d, id| {
                d.child(
                    icon_button("close-sessions-pane", Icon::Close, cx)
                        .tooltip(Tooltip::text("Close pane"))
                        .on_click(cx.listener(move |this, _, window, cx| this.close_pane_id(id, window, cx))),
                )
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
                    .on_key_down(cx.listener(|this, ev: &gpui::KeyDownEvent, window, cx| {
                        if ev.keystroke.key == "escape" {
                            cx.stop_propagation();
                            this.toggle_session_filter(window, cx);
                        }
                    })),
            )
        });
        let forms = self.render_workspace_forms(cx);
        let removed = self.render_removed_workspaces(cx);

        let scroll = scrollbar::handle("session-list");
        let list = div()
            .id("session-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&scroll)
            .px_1p5()
            .pb_1()
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
        let picks = self.render_pick_bar(pick_cwd, in_pick_folder, tree.busy, cx);
        div()
            .flex()
            .flex_col()
            .size_full()
            .child(crate::ui::pane_drag::pane_handle(header, pane, "Sessions"))
            .children(filter)
            .child(scrollbar::vertical_over("session-list-bar", scroll, list, |d| d.flex_1().min_h_0()))
            .children(picks)
    }

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
