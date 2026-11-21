//! The Sessions & Workspaces tree: each workspace row folds/unfolds to show its active
//! `agent_cli` panes and past sessions on disk (`kynex_core::sessions` + `kynex_core::tree`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::SystemTime;

use gpui::{AnyElement, Context, div, prelude::*};
use kynex_core::sessions::SessionInfo;
use kynex_core::tree::{self, Branch, Section, View};
use kynex_core::workspace;

use super::KynexApp;
use super::agent_cli::AgentId;
use super::session_groups::DraggedSession;
use super::session_rows::{OpenSession, RowCtx, list_header};
use super::workspace_list::{WorkspaceRow, stats_text};
use crate::theme::theme;
use crate::ui::{Lamp, px, spinner};

pub(super) struct Tree {
    pub rows: Vec<AnyElement>,
    pub order: Vec<(PathBuf, String)>,
    pub busy: Vec<String>,
}

pub(super) struct TreeCtx {
    pub cwd: PathBuf,
    pub current: Option<String>,
    pub current_agent: Option<AgentId>,
    pub query: String,
    pub cursor: Option<String>,
}

struct Place {
    path: PathBuf,
    row: WorkspaceRow,
    selected: bool,
}

impl KynexApp {
    fn places(&mut self, ctx: &TreeCtx, cx: &mut Context<Self>) -> Vec<Place> {
        let Some(project) = self.workspace_project() else { return Vec::new() };
        let list = self.workspaces_of(&project, cx);
        let selected = list.iter().position(|(c, _)| c.at(&ctx.cwd)).unwrap_or(0);
        list.into_iter()
            .enumerate()
            .map(|(n, (c, worktree))| {
                let tip = match worktree.as_ref().and_then(stats_text) {
                    Some(stats) => format!("{}\n{stats}", c.path.display()),
                    None => c.path.display().to_string(),
                };
                let drift = self.drift_of(&project, &c);
                let branch = if c.folder {
                    Some(if c.path.is_dir() { "folder" } else { "missing" }.to_string())
                } else if let Some(d) = &drift {
                    Some(d.live.clone())
                } else {
                    c.branch.clone().filter(|b| *b != c.label)
                };
                let row = WorkspaceRow {
                    path: c.path.clone(),
                    label: c.label.clone(),
                    branch,
                    drift: drift.map(|d| format!("Bound to {}; now on {}", d.bound, d.live)),
                    tip,
                    selected: n == selected,
                    open: false,
                    count: None,
                    lamps: Vec::new(),
                    wants_you: None,
                };
                Place { path: c.path, row, selected: n == selected }
            })
            .collect()
    }

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

    fn open_agent_sessions(&self, ws: &Path) -> Vec<OpenSession> {
        let mut list: Vec<&super::agent_cli::LiveAgent> = self
            .agent_instances
            .values()
            .filter(|a| workspace::same_folder(&a.cwd, ws) && a.meta.provider.is_some())
            .collect();
        list.sort_by_key(|a| a.id);
        list.into_iter()
            .filter_map(|inst| {
                let provider = inst.meta.provider.as_deref()?;
                let lamp = self.instance_lamp(inst.id);
                Some(OpenSession {
                    agent_id: inst.id,
                    id: inst.meta.session_id.clone(),
                    title: self.agent_title(&inst.cwd, &inst.meta),
                    provider_badge: self.provider_badge(provider),
                    provider_name: self.provider_display_name(provider),
                    busy: lamp == Lamp::Working,
                    lamp,
                })
            })
            .collect()
    }

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
        let open = self.open_agent_sessions(&path);
        let busy: Vec<String> = open.iter().filter(|o| o.busy).filter_map(|o| o.id.clone()).collect();
        let groups = self.groups_for(&path).clone();
        let wanted = place.selected || groups.tree_open || filtering;
        let here = place.selected;
        let mut loading = false;
        if wanted {
            loading = self.want_list(&path, if here { ctx.current.clone() } else { None }, &open, cx);
        }
        let items: Rc<Vec<SessionInfo>> = self.sessions.get(&path).map(|l| l.items.clone()).unwrap_or_default();
        let default_branch = self.sessions.get(&path).and_then(|l| l.default_branch.clone());
        let index: HashMap<String, usize> = items.iter().enumerate().map(|(i, s)| (s.id.clone(), i)).collect();
        let listed: Vec<(String, SystemTime)> = items.iter().map(|i| (i.id.clone(), i.modified)).collect();
        let open_ids: Vec<String> = open.iter().filter_map(|o| o.id.clone()).collect();
        let fresh: Vec<String> = open.iter().rev().map(OpenSession::key).filter(|k| !index.contains_key(k)).collect();
        let mut sections = tree::sections(&groups, &listed, &open_ids, &fresh, SystemTime::now());
        if filtering {
            let title = |key: &str| match index.get(key) {
                Some(&i) => items[i].title.clone(),
                None => open.iter().find(|o| o.key() == key).map_or_else(|| "Session".into(), |o| o.title.clone()),
            };
            sections.retain(|k| tree::matches(&ctx.query, &title(k)));
        }
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
        let view = View { groups: &groups, show_archived: self.saved.sessions_show_archived, filtering };
        let is_open = tree::workspace_open(place.selected, groups.tree_open, filtering, sections.len());
        let branch = Branch {
            path: path.clone(),
            open: is_open,
            rows: if is_open { view.drawn(&sections) } else { Vec::new() },
        };

        let shown_count = if view.show_archived { sections.len() } else { sections.len() - sections.archived.len() };
        place.row.open = is_open;
        place.row.count = (!is_open && self.sessions.contains_key(&path) && !filtering).then_some(shown_count);
        place.row.lamps = open.iter().map(|o| o.lamp).collect();
        place.row.wants_you = open
            .iter()
            .filter(|o| matches!(o.lamp, Lamp::NeedsYou | Lamp::Done))
            .max_by_key(|o| o.lamp == Lamp::NeedsYou)
            .map(|o| o.agent_id);
        let rows = vec![self.render_workspace_row(n, &place.row, cx)];

        let divided = n > 0;
        let border = t.border_variant;
        let block =
            move |d: gpui::Stateful<gpui::Div>| d.when(divided, |d| d.mt_1().pt_1().border_t_1().border_color(border));
        if !is_open {
            return (
                vec![block(div().id(("tree-ws", n))).flex().flex_col().children(rows).into_any_element()],
                branch,
                busy,
            );
        }

        let row_ctx = RowCtx {
            cwd: path.clone(),
            here,
            current: ctx.current.clone(),
            current_agent: ctx.current_agent,
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
        let hidden = if view.is_open(Section::Archived, sections.archived.len()) { 0 } else { sections.archived.len() };
        if sections.len() == hidden {
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
        body.extend(take(self, &sections.pinned, cx));
        if !sections.recent.is_empty() {
            let mut recent: Vec<AnyElement> = Vec::new();
            match &by_provider {
                Some(split) => {
                    let new: Vec<String> =
                        sections.recent.iter().filter(|k| !index.contains_key(*k)).cloned().collect();
                    recent.extend(take(self, &new, cx));
                    for (provider_id, ids) in split {
                        let label = self.provider_display_name(provider_id);
                        recent.push(list_header(&label, false, cx).into_any_element());
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
                        this.edit_groups(&drop_cwd, |g| {
                            d.ids.iter().for_each(|id| {
                                g.drop_into(id, None);
                            })
                        });
                        this.organize.selection.clear();
                        cx.notify();
                    }))
                    .children(recent)
                    .into_any_element(),
            );
        }
        for (i, ids) in sections.groups.iter().enumerate() {
            if filtering && ids.is_empty() {
                continue;
            }
            let open_group = view.is_open(Section::Group(i), ids.len());
            let news: Vec<Lamp> = ids
                .iter()
                .filter_map(|id| open.iter().find(|o| o.id.as_deref() == Some(id.as_str())))
                .filter(|o| o.has_news())
                .map(|o| o.lamp)
                .collect();
            let group_rows = if open_group { take(self, ids, cx) } else { Vec::new() };
            body.push(self.render_group(&path, i, ids.len(), group_rows, &news, open_group, cx));
        }
        if !sections.older.is_empty() {
            let open_older = view.is_open(Section::Older, sections.older.len());
            if open_older {
                body.extend(take(self, &sections.older, cx));
            }
            if !filtering {
                let label =
                    if open_older { "Show less".to_string() } else { format!("Show {} more", sections.older.len()) };
                body.push(self.render_show_more(&path, label, cx));
            }
        }
        if view.is_open(Section::Archived, sections.archived.len()) {
            let archived = take(self, &sections.archived, cx);
            body.push(self.render_archived(&path, archived, cx));
        }
        let block = block(div().id(("tree-ws", n)))
            .flex()
            .flex_col()
            .children(rows)
            .child(div().flex().flex_col().pl_3().children(body));
        (vec![block.into_any_element()], branch, busy)
    }

    fn render_show_more(&mut self, path: &Path, label: String, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let (toggle, text) = (path.to_path_buf(), t.text);
        div()
            .id("older")
            .px_2()
            .py_1()
            .text_xs()
            .text_color(t.text_placeholder)
            .cursor_pointer()
            .hover(move |s| s.text_color(text))
            .child(label)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.edit_groups(&toggle, |g| g.older_open = !g.older_open);
                cx.notify();
            }))
            .into_any_element()
    }

    pub(super) fn folder_of_listed(&self, id: &str) -> Option<PathBuf> {
        self.sessions.iter().find(|(_, l)| l.items.iter().any(|i| i.id == id)).map(|(p, _)| p.clone())
    }

    pub(super) fn listed_title(&self, cwd: &Path, id: &str) -> Option<String> {
        self.sessions.get(cwd)?.items.iter().find(|i| i.id == id).map(|i| i.title.clone())
    }
}
