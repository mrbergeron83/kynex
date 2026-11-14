//! The sidebar's tree: each workspace of the project a row that folds (`workspace_list.rs`),
//! and under it its sessions in sections: Pinned, Recent, the groups, Older, Archived. The
//! rules for what goes where are `kynex_core::tree`; this draws them. A project with one
//! workspace, and chat, draw no workspace row: just the sessions.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use gpui::{AnyElement, Context, div, prelude::*};
use kynex_core::session::{Kind, SessionInfo};
use kynex_core::tree::{self, Branch, Section, View};
use kynex_core::workspace;

use super::KynexApp;
use super::session_groups::DraggedSession;
use super::session_rows::{OpenSession, RowCtx, list_header};
use super::workspace_list::{WorkspaceRow, stats_text};
use crate::theme::theme;
use crate::ui::{Lamp, px, spinner};

/// The tree as drawn: its rows, and what the list keys and picking need.
pub(super) struct Tree {
    pub rows: Vec<AnyElement>,
    /// Every session row drawn, in order, with its workspace.
    pub order: Vec<(PathBuf, String)>,
    /// Ids of sessions still working, which a delete of the picks leaves.
    pub busy: Vec<String>,
}

/// What the whole tree is drawn for.
pub(super) struct TreeCtx {
    pub kind: Kind,
    /// The shown session's folder, id and key.
    pub cwd: PathBuf,
    pub current: Option<String>,
    pub current_uid: kynex_core::live::Key,
    /// The title filter's query, lowercased; empty when it's off.
    pub query: String,
    pub cursor: Option<String>,
}

/// A workspace to draw: where it is, and its row unless the tree has none.
struct Place {
    path: PathBuf,
    row: Option<WorkspaceRow>,
    selected: bool,
}

impl KynexApp {
    /// The workspaces to draw: the project's, in their steady order; in chat, or a project
    /// that has just the one, its folder without a row.
    fn places(&mut self, ctx: &TreeCtx, cx: &mut Context<Self>) -> Vec<Place> {
        let project = self.workspace_project().filter(|_| ctx.kind == Kind::Code);
        let list = match &project {
            Some(p) => self.workspaces_of(p, cx),
            None => Vec::new(),
        };
        if list.len() < 2 {
            return vec![Place { path: ctx.cwd.clone(), row: None, selected: true }];
        }
        let selected = list.iter().position(|(c, _)| c.at(&ctx.cwd)).unwrap_or(0);
        list.into_iter()
            .enumerate()
            .map(|(n, (c, worktree))| {
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
                let row = WorkspaceRow {
                    path: c.path.clone(),
                    label: c.label.clone(),
                    branch,
                    tip,
                    selected: n == selected,
                    open: false,
                    lamps: Vec::new(),
                    wants_you: None,
                };
                Place { path: c.path, row: Some(row), selected: n == selected }
            })
            .collect()
    }

    /// The tree's rows, every workspace with its sessions.
    pub(super) fn render_tree(&mut self, ctx: &TreeCtx, cx: &mut Context<Self>) -> Tree {
        let _t = kynex_core::trace::span("render_tree");
        let places = self.places(ctx, cx);
        let mut out = Tree { rows: Vec::new(), order: Vec::new(), busy: Vec::new() };
        let mut branches = Vec::new();
        for (n, place) in places.into_iter().enumerate() {
            let (rows, branch, busy) = self.render_place(n, place, ctx, cx);
            out.rows.extend(rows);
            out.busy.extend(busy);
            branches.push(branch);
        }
        out.order = tree::rows_in_order(&branches);
        out
    }

    /// One workspace: its row (if drawn), and under it, when it's open, its sessions.
    fn render_place(
        &mut self,
        n: usize,
        mut place: Place,
        ctx: &TreeCtx,
        cx: &mut Context<Self>,
    ) -> (Vec<AnyElement>, Branch, Vec<String>) {
        let t = theme(cx).clone();
        let filtering = !ctx.query.is_empty();
        let path = place.path.clone();
        // The sessions open there: in chat every one, in a project those in its folder.
        let open: Vec<OpenSession> = self
            .live_mut()
            .map(|l| {
                l.sessions
                    .iter()
                    .filter(|s| ctx.kind == Kind::Chat || workspace::same_folder(&s.cwd, &path))
                    .map(OpenSession::of)
                    .collect()
            })
            .unwrap_or_default();
        let busy: Vec<String> = open.iter().filter(|o| o.busy).filter_map(|o| o.id.clone()).collect();
        let groups = self.groups_for(&path).clone();
        // Lists are read for what is open in the tree, and all of them while filtering.
        let wanted = place.selected || groups.tree_open || filtering;
        let here = place.selected;
        let mut loading = false;
        if wanted {
            loading = self.want_list(&path, if here { ctx.current.clone() } else { None }, &open, cx);
        }
        let items: Vec<SessionInfo> = self.sessions.get(&path).map(|l| l.items.clone()).unwrap_or_default();
        if here {
            self.name_sessions(&items, cx);
        }
        let default_branch = self.sessions.get(&path).and_then(|l| l.default_branch.clone());
        let index: HashMap<String, usize> = items.iter().enumerate().map(|(i, s)| (s.id.clone(), i)).collect();
        let listed: Vec<(String, SystemTime)> = items.iter().map(|i| (i.id.clone(), i.modified)).collect();
        let open_ids: Vec<String> = open.iter().filter_map(|o| o.id.clone()).collect();
        // New ones first, the newest at the top.
        let fresh: Vec<String> = open.iter().rev().map(OpenSession::key).filter(|k| !index.contains_key(k)).collect();
        let mut sections = tree::sections(&groups, &listed, &open_ids, &fresh, SystemTime::now());
        if filtering {
            let title = |key: &str| match index.get(key) {
                Some(&i) => items[i].title.clone(),
                None => open
                    .iter()
                    .find(|o| o.key() == key)
                    .and_then(|o| o.first.clone())
                    .unwrap_or_else(|| "New session".into()),
            };
            sections.retain(|k| tree::matches(&ctx.query, &title(k)));
        }
        // Grouped by provider, when picked and there's more than one: Recent's listed
        // sessions under a header each, after the new ones.
        let by_provider = (self.saved.sessions_by_provider && !filtering)
            .then(|| {
                let listed_recent: Vec<String> =
                    sections.recent.iter().filter(|k| index.contains_key(*k)).cloned().collect();
                groups.by_provider(&listed_recent)
            })
            .filter(|split| split.len() > 1);
        if let Some(split) = &by_provider {
            let new: Vec<String> = sections.recent.iter().filter(|k| !index.contains_key(*k)).cloned().collect();
            sections.recent = new.into_iter().chain(split.iter().flat_map(|(_, ids)| ids.iter().cloned())).collect();
        }
        let archived_open = self.organize.archive_open.contains(&path);
        let view = View { groups: &groups, archived_open, filtering };
        let is_open =
            place.row.is_none() || tree::workspace_open(place.selected, groups.tree_open, filtering, sections.len());
        let branch = Branch {
            path: path.clone(),
            open: is_open,
            rows: if is_open { view.drawn(&sections) } else { Vec::new() },
        };

        let mut rows: Vec<AnyElement> = Vec::new();
        if let Some(row) = &mut place.row {
            row.open = is_open;
            let agents: Vec<Lamp> = self.agents_in(&path).into_iter().map(|(_, _, l)| l).collect();
            row.lamps = open.iter().map(OpenSession::lamp).chain(agents).collect();
            row.wants_you = open.iter().filter(|o| o.waiting || o.finished).max_by_key(|o| o.waiting).map(|o| o.uid);
            rows.push(self.render_workspace_row(n, row, cx));
        }
        if !is_open {
            return (vec![div().id(("tree-ws", n)).flex().flex_col().children(rows).into_any_element()], branch, busy);
        }

        let row_ctx = RowCtx {
            kind: ctx.kind,
            cwd: path.clone(),
            here,
            current: ctx.current.clone(),
            current_uid: ctx.current_uid,
            picking: self.organize.selection.on,
            cursor: ctx.cursor.clone(),
            picked_ids: self.organize.selection.ids.clone(),
            default_branch,
            open: &open,
            t: t.clone(),
        };
        let take = |this: &mut Self, keys: &[String], cx: &mut Context<Self>| -> Vec<AnyElement> {
            keys.iter()
                .filter_map(|k| match index.get(k) {
                    Some(&i) => Some(this.past_row(i, &items[i], &row_ctx, cx)),
                    None => open.iter().find(|o| o.key() == *k).map(|o| this.fresh_row(o, &row_ctx, cx)),
                })
                .collect()
        };
        let mut body: Vec<AnyElement> = Vec::new();
        if sections.is_empty() {
            let text = match (loading || (wanted && !self.sessions.contains_key(&path)), filtering) {
                (true, _) => "Loading sessions…",
                (false, true) => "No sessions match",
                (false, false) => "No sessions yet",
            };
            let spin = text.starts_with("Loading");
            body.push(
                div()
                    .px_2()
                    .py_1p5()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_sm()
                    .text_color(t.text_placeholder)
                    .when(spin, |d| d.child(spinner(("sessions-loading", n), t.text_placeholder, px(12.))))
                    .child(text)
                    .into_any_element(),
            );
        }
        let headers = sections.headers();
        if !sections.pinned.is_empty() {
            if headers {
                body.push(list_header("Pinned", body.is_empty(), cx).into_any_element());
            }
            body.extend(take(self, &sections.pinned, cx));
        }
        if !sections.recent.is_empty() || headers {
            // Dropped here, a session goes back to the loose ones: out of its group, the
            // archive and the pinned ones.
            let mut recent: Vec<AnyElement> = Vec::new();
            if headers {
                recent.push(list_header("Recent", body.is_empty(), cx).into_any_element());
            }
            match &by_provider {
                Some(split) => {
                    let new: Vec<String> =
                        sections.recent.iter().filter(|k| !index.contains_key(*k)).cloned().collect();
                    recent.extend(take(self, &new, cx));
                    for (provider, ids) in split {
                        recent.push(list_header(provider.label(), false, cx).into_any_element());
                        recent.extend(take(self, ids, cx));
                    }
                }
                None => recent.extend(take(self, &sections.recent, cx)),
            }
            let (drop_cwd, hover) = (path.clone(), t.hover);
            body.push(
                div()
                    .id("recent-block")
                    .flex()
                    .flex_col()
                    .rounded_md()
                    .drag_over::<DraggedSession>(move |s, _, _, _| s.bg(hover))
                    .on_drop(cx.listener(move |this, d: &DraggedSession, _, cx| {
                        cx.stop_propagation();
                        if d.cwd != drop_cwd {
                            return;
                        }
                        this.edit_groups(&drop_cwd, |g| d.ids.iter().for_each(|id| g.move_to(id, None)));
                        this.organize.selection.clear();
                        cx.notify();
                    }))
                    .children(recent)
                    .into_any_element(),
            );
        }
        for (i, ids) in sections.groups.iter().enumerate() {
            // Filtering, groups with nothing that matches step aside.
            if filtering && ids.is_empty() {
                continue;
            }
            let open_group = view.is_open(Section::Group(i), ids.len());
            let news: Vec<Lamp> = ids
                .iter()
                .filter_map(|id| open.iter().find(|o| o.id.as_deref() == Some(id.as_str())))
                .filter(|o| o.has_news())
                .map(OpenSession::lamp)
                .collect();
            let group_rows = if open_group { take(self, ids, cx) } else { Vec::new() };
            body.push(self.render_group(&path, i, ids.len(), group_rows, &news, open_group, cx));
        }
        if !sections.older.is_empty() {
            let open_older = view.is_open(Section::Older, sections.older.len());
            let label = if open_older { "Older".to_string() } else { format!("Older · {}", sections.older.len()) };
            let (toggle, text) = (path.clone(), t.text);
            body.push(
                list_header(&label, body.is_empty(), cx)
                    .id("older")
                    .cursor_pointer()
                    .hover(move |s| s.text_color(text))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.edit_groups(&toggle, |g| g.older_open = !g.older_open);
                        cx.notify();
                    }))
                    .into_any_element(),
            );
            if open_older {
                body.extend(take(self, &sections.older, cx));
            }
        }
        if !sections.archived.is_empty() {
            let open_archive = view.is_open(Section::Archived, sections.archived.len());
            let archived = if open_archive { take(self, &sections.archived, cx) } else { Vec::new() };
            body.push(self.render_archived(&path, sections.archived.len(), archived, open_archive, cx));
        }
        let nested = place.row.is_some();
        let block = div()
            .id(("tree-ws", n))
            .flex()
            .flex_col()
            .children(rows)
            .child(div().flex().flex_col().when(nested, |d| d.pl_3()).children(body));
        (vec![block.into_any_element()], branch, busy)
    }

    /// The folder holding listed session `id`, among the lists read.
    pub(super) fn folder_of_listed(&self, id: &str) -> Option<PathBuf> {
        self.sessions.iter().find(|(_, l)| l.items.iter().any(|i| i.id == id)).map(|(p, _)| p.clone())
    }

    /// A session's title in its folder's list, if it's read.
    pub(super) fn listed_title(&self, cwd: &Path, id: &str) -> Option<String> {
        self.sessions.get(cwd)?.items.iter().find(|i| i.id == id).map(|i| i.title.clone())
    }
}
