//! Which session started which (`kynex_core::lineage`): links recorded when a session makes or
//! forks one (by `knx`, or `/fork` by hand), held by uid until the child has an id, kept per
//! project; the caps checked before a create; and a child's answers, and the outcomes of
//! requests a session was told were pending, sent back to it as messages.

use std::path::{Path, PathBuf};

use gpui::{AnyElement, Context, div, prelude::*};
use kynex_core::control::approval::Made;
use kynex_core::lineage::{self, Lineage, Link, Notice, Placed, Route};
use kynex_core::live::Key;
use kynex_core::session::Session;
use kynex_core::toasts::Toast;

use super::KynexApp;
use crate::theme::theme;
use crate::ui::{Icon, Tooltip, icon, icon_button, px};

/// A link waiting for its child's CLI id.
pub(super) struct PendingParent {
    pub child: Key,
    pub parent: String,
    pub parent_folder: PathBuf,
    /// Its answers go back to the parent (see [`Link::reports`]).
    pub reports: bool,
}

impl KynexApp {
    /// The project an open session belongs to (none for chat).
    pub(super) fn project_of(&self, key: Key) -> Option<PathBuf> {
        self.code.iter().find(|(_, l)| l.sessions.iter().any(|s| s.uid == key)).map(|(p, _)| p.clone())
    }

    /// A project's links, read from disk the first time.
    pub(super) fn lineage_for(&mut self, project: &Path) -> &Lineage {
        self.lineage.entry(project.to_path_buf()).or_insert_with(|| Lineage::load(project))
    }

    /// Changes a project's links and saves them if anything changed.
    pub(super) fn edit_lineage(&mut self, project: &Path, f: impl FnOnce(&mut Lineage)) {
        self.lineage_for(project);
        let Some(l) = self.lineage.get_mut(project) else { return };
        let before = l.clone();
        f(l);
        if *l != before
            && let Err(e) = l.save(project)
        {
            self.toast(Toast::error(e));
        }
    }

    /// The open session that started open session `key`, if it's open.
    pub(super) fn parent_key(&self, key: Key) -> Option<Key> {
        self.parent_link(key).map(|(parent, _)| parent)
    }

    /// The open session that started open session `key`, and whether `key` reports to it.
    fn parent_link(&self, key: Key) -> Option<(Key, bool)> {
        if let Some(maker) = self.approvals.maker(&Made::Session(key)).filter(|m| self.session_by_key(*m).is_some()) {
            return Some((maker, true));
        }
        let open = |id: &str| self.all_sessions().find(|s| s.session_id.as_deref() == Some(id)).map(|s| s.uid);
        if let Some(p) = self.pending_parents.iter().find(|p| p.child == key) {
            return open(&p.parent).map(|k| (k, p.reports));
        }
        let s = self.session_by_key(key)?;
        let id = s.session_id.as_deref()?;
        let project = self.project_of(key)?;
        let link = self.lineage.get(&project)?.parent_of(id)?;
        open(&link.parent).map(|k| (k, link.reports))
    }

    /// How many open sessions `parent` started.
    fn open_children(&self, parent: Key) -> usize {
        self.all_sessions().filter(|s| self.parent_key(s.uid) == Some(parent)).count()
    }

    /// Whether open session `parent` may start another: the caps on depth and open children.
    pub(super) fn check_caps(&mut self, parent: Option<Key>) -> Result<(), String> {
        let Some(parent) = parent else { return Ok(()) };
        let open = self.open_children(parent);
        let id = self.session_by_key(parent).and_then(|s| s.session_id.clone());
        match (id, self.project_of(parent)) {
            (Some(id), Some(project)) => self.lineage_for(&project).can_add(&id, open),
            _ => Lineage::default().can_add("", open),
        }
    }

    /// Notes that open session `parent` started `child`; written once the child has an id. A
    /// session asked for it (`asked`), its answers go back to `parent`; a fork by hand only nests.
    pub(super) fn record_parent(&mut self, child: &Session, parent: Key, asked: bool) {
        if asked {
            self.approvals.record_made(Made::Session(child.uid), Some(parent));
        }
        let Some(p) = self.session_by_key(parent) else { return };
        let Some(id) = p.session_id.clone() else { return };
        let (parent_folder, reports) = (p.cwd.clone(), asked);
        self.pending_parents.push(PendingParent { child: child.uid, parent: id, parent_folder, reports });
    }

    /// Writes the links whose child now has an id; drops those whose child closed.
    pub(super) fn give_pending_parents(&mut self) {
        if self.pending_parents.is_empty() {
            return;
        }
        for p in std::mem::take(&mut self.pending_parents) {
            let Some(s) = self.session_by_key(p.child) else { continue };
            let Some(id) = s.session_id.clone() else {
                self.pending_parents.push(p);
                continue;
            };
            let folder = s.cwd.clone();
            let Some(project) = self.project_of(p.child) else { continue };
            let link = Link { parent: p.parent, parent_folder: p.parent_folder, folder, reports: p.reports };
            self.edit_lineage(&project, |l| {
                l.link(&id, link);
            });
        }
    }

    /// Past session `id` was deleted: its children, in whichever open project, move up a level.
    pub(super) fn unlink_session(&mut self, id: &str) {
        let projects: Vec<PathBuf> = self.code.keys().cloned().collect();
        for project in projects {
            self.edit_lineage(&project, |l| {
                l.remove(id);
            });
        }
    }

    /// Takes sessions `ids` of the workspace at `cwd` out of the archive, with the sessions
    /// under them that went in with them.
    pub(super) fn unarchive_nest(&mut self, cwd: &Path, ids: &[String]) {
        self.edit_groups(cwd, |g| {
            ids.iter().for_each(|id| {
                g.unarchive(id);
            })
        });
        for (id, folder) in self.nest_of(ids) {
            self.edit_groups(&folder, |g| {
                g.unarchive(&id);
            });
        }
    }

    /// Every session under `ids`, with its folder.
    fn nest_of(&mut self, ids: &[String]) -> Vec<(String, PathBuf)> {
        let Some(project) = self.workspace_project() else { return Vec::new() };
        let l = self.lineage_for(&project);
        ids.iter()
            .flat_map(|id| l.descendants(id))
            .filter_map(|d| l.parent_of(&d).map(|link| (d.clone(), link.folder.clone())))
            .collect()
    }

    /// Archives sessions `ids` of the workspace at `cwd`, and every session under them, each in
    /// its own workspace.
    pub(super) fn archive_nest(&mut self, cwd: &Path, ids: &[String]) {
        self.edit_groups(cwd, |g| ids.iter().for_each(|id| g.archive(id)));
        for (id, folder) in self.nest_of(ids) {
            self.edit_groups(&folder, |g| g.archive(&id));
        }
    }

    /// Sends `text` to open session `to`: now if it's idle, queued for its turn's end if it's
    /// working, not at all if it closed.
    fn deliver(&mut self, to: Key, text: String) {
        let busy = self.session_by_key(to).map(|s| s.busy);
        let Some(s) = self.session_for(to) else { return };
        match lineage::route(busy) {
            Route::SendNow => s.send(text, Vec::new()),
            Route::Queue => s.queue.push(text, Vec::new()),
            Route::Drop => {}
        }
    }

    /// Sessions `ended` finished a turn (the item that ended it): each one a session asked for
    /// sends its answer to it; and outcomes of requests their asker was told were pending go
    /// back to it.
    pub(super) fn report_back(&mut self, ended: &[(Key, usize)]) {
        let mut out = Vec::new();
        for &(key, end) in ended {
            let Some((parent, true)) = self.parent_link(key) else { continue };
            let Some(s) = self.session_by_key(key) else { continue };
            let Some((said, failed)) = s.items.get(..=end).and_then(lineage::turn_answer) else { continue };
            let said = match (said.is_empty(), failed) {
                (true, true) => "(The turn ended in an error.)".to_string(),
                (true, false) => "(The turn ended without a message.)".to_string(),
                (false, true) => format!("(The turn ended in an error.)\n\n{said}"),
                (false, false) => said,
            };
            let from = s.session_id.clone().unwrap_or_else(|| key.to_string());
            let title = self.running_name(s);
            out.push((parent, lineage::notice(&Notice { from, title, text: said })));
        }
        if self.approvals.any_settled() {
            for done in self.approvals.take_settled() {
                let n = Notice { from: format!("request {}", done.id), title: "Kynex".into(), text: done.text() };
                out.push((done.caller, lineage::notice(&n)));
            }
        }
        for (to, text) in out {
            self.deliver(to, text);
        }
    }
}

impl KynexApp {
    /// The marks on listed session `id`'s row (number `n`) for the sessions that started each
    /// other: a fold for the ones nested under it, how many it started that are listed
    /// elsewhere, and a link to its parent when it isn't nested under it.
    pub(super) fn nest_marks(
        &mut self,
        n: usize,
        id: &str,
        placed: &Placed,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let t = theme(cx).clone();
        let mut out = Vec::new();
        let Some(project) = self.workspace_project() else { return out };
        if placed.children > 0 {
            let folded = !placed.hidden.is_empty() || self.lineage_for(&project).folded.contains(id);
            let (fold_id, fold_project) = (id.to_string(), project.clone());
            let tip = if folded { "Show the sessions it started" } else { "Fold the sessions it started" };
            out.push(
                div()
                    .id(("nest-fold", n))
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap_0p5()
                    .px_1()
                    .rounded_sm()
                    .text_xs()
                    .text_color(t.text_placeholder)
                    .cursor_pointer()
                    .hover(|s| s.bg(t.hover))
                    .child(
                        icon(if folded { Icon::ChevronRight } else { Icon::ChevronDown }, t.text_placeholder).size_3(),
                    )
                    .when(folded, |d| d.child(placed.hidden.len().to_string()))
                    .tooltip(Tooltip::text(tip))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.edit_lineage(&fold_project, |l| l.toggle_fold(&fold_id));
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        }
        if placed.elsewhere > 0 {
            let tip = match placed.elsewhere {
                1 => "It started 1 more session, listed in another workspace or section".to_string(),
                k => format!("It started {k} more sessions, listed in other workspaces or sections"),
            };
            out.push(
                div()
                    .id(("nest-elsewhere", n))
                    .flex_none()
                    .px_1()
                    .text_xs()
                    .text_color(t.text_placeholder)
                    .child(format!("+{}", placed.elsewhere))
                    .tooltip(Tooltip::text(tip))
                    .into_any_element(),
            );
        }
        if let Some(parent) = &placed.parent_away {
            let folder = self.lineage_for(&project).parent_of(id).map(|l| l.parent_folder.clone());
            let name = folder
                .as_deref()
                .and_then(|f| self.listed_title(f, parent))
                .unwrap_or_else(|| "another session".into());
            let (to, folder) = (parent.clone(), folder.unwrap_or_else(|| project.clone()));
            out.push(
                icon_button(("nest-parent", n), Icon::ArrowUp, cx)
                    .size(px(20.))
                    .tooltip(Tooltip::text(format!("Started by {name}: show it")))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.show_in_workspace(&folder, &to, window, cx);
                    }))
                    .into_any_element(),
            );
        }
        out
    }
}

impl KynexApp {
    /// The session header's marks for session `key` (CLI id `id`): the session that started
    /// it, which a click shows, and how many it started.
    pub(super) fn family_badges(&mut self, key: Key, id: Option<&str>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let t = theme(cx).clone();
        let mut out = Vec::new();
        let Some(project) = self.project_of(key) else { return out };
        let chip = |id: &'static str| {
            div()
                .id(id)
                .flex()
                .flex_none()
                .items_center()
                .gap_1()
                .px_1p5()
                .h(px(20.))
                .rounded_md()
                .border_1()
                .border_color(t.border_variant)
                .text_xs()
                .text_color(t.text_muted)
                .cursor_pointer()
                .hover(|s| s.bg(t.hover))
        };
        let link = id.and_then(|id| self.lineage_for(&project).parent_of(id).cloned());
        if let Some(open) = self.parent_key(key) {
            let name = self.session_by_key(open).map(|s| self.running_name(s)).unwrap_or_default();
            out.push(
                chip("session-parent")
                    .child(icon(Icon::ArrowUp, t.text_muted).size_3())
                    .child(div().max_w(px(160.)).truncate().child(name.clone()))
                    .tooltip(Tooltip::with_meta(format!("Started by {name}"), "Click to show it"))
                    .on_click(cx.listener(move |this, _, window, cx| this.focus_open_session(open, window, cx)))
                    .into_any_element(),
            );
        } else if let Some(link) = link {
            let name =
                self.listed_title(&link.parent_folder, &link.parent).unwrap_or_else(|| "a closed session".into());
            out.push(
                chip("session-parent")
                    .child(icon(Icon::ArrowUp, t.text_muted).size_3())
                    .child(div().max_w(px(160.)).truncate().child(name.clone()))
                    .tooltip(Tooltip::with_meta(format!("Started by {name}"), "Click to show it"))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.show_in_workspace(&link.parent_folder, &link.parent, window, cx)
                    }))
                    .into_any_element(),
            );
        }
        // What it started: linked ones, and open ones with no id yet.
        let children: Vec<String> = id
            .map(|id| self.lineage_for(&project).children_of(id).into_iter().map(str::to_string).collect())
            .unwrap_or_default();
        let unlinked = self
            .all_sessions()
            .filter(|s| s.session_id.as_ref().is_none_or(|c| !children.contains(c)))
            .filter(|s| self.parent_key(s.uid) == Some(key))
            .count();
        let n = children.len() + unlinked;
        if n > 0 {
            let unfold = (project, id.map(str::to_string));
            out.push(
                chip("session-children")
                    .child(format!("{n} started"))
                    .tooltip(Tooltip::with_meta(
                        format!("It started {n} session{}", if n == 1 { "" } else { "s" }),
                        "Listed under it in the sessions tree",
                    ))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let (project, Some(id)) = &unfold else { return };
                        if this.lineage_for(project).folded.contains(id) {
                            this.edit_lineage(project, |l| l.toggle_fold(id));
                        }
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        }
        out
    }
}

/// How many lines of an answer a notice shows.
const NOTICE_LINES: usize = 12;

impl KynexApp {
    /// A message that carries answers from other sessions (item `ix`): each as a notice saying
    /// who it's from, which a click shows, then anything else the message said.
    pub(super) fn render_session_notices(&self, ix: usize, text: &str, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx).clone();
        let (notices, rest) = lineage::split_notices(text);
        let mut col = div().flex().flex_col().gap_2();
        for (i, n) in notices.into_iter().enumerate() {
            let open = self.all_sessions().find(|s| s.session_id.as_deref() == Some(n.from.as_str())).map(|s| s.uid);
            let mut lines: Vec<&str> = n.text.lines().collect();
            let cut = lines.len() > NOTICE_LINES;
            lines.truncate(NOTICE_LINES);
            let body = lines.join("\n") + if cut { "\n…" } else { "" };
            let head = div()
                .id(("session-notice", ix * 16 + i))
                .flex()
                .items_center()
                .gap_1p5()
                .text_xs()
                .child(icon(Icon::ArrowUp, t.accent).size_3p5().flex_none())
                .child(div().flex_none().font_weight(gpui::FontWeight::MEDIUM).text_color(t.accent).child("From"))
                .child(div().flex_1().min_w_0().truncate().text_color(t.text_muted).child(n.title.clone()))
                .when_some(open, |d, key| {
                    d.cursor_pointer()
                        .tooltip(Tooltip::text("Show that session"))
                        .on_click(cx.listener(move |this, _, window, cx| this.focus_open_session(key, window, cx)))
                });
            col = col.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .px_2()
                    .py_1p5()
                    .rounded_md()
                    .border_1()
                    .border_color(t.border_variant)
                    .child(head)
                    .when(!body.trim().is_empty(), |d| {
                        d.child(div().text_sm().text_color(t.text_muted).whitespace_normal().child(body))
                    }),
            );
        }
        if !rest.is_empty() {
            col = col.child(div().px_3().py_2().rounded_lg().bg(t.element).text_sm().child(rest));
        }
        col.into_any_element()
    }
}

impl KynexApp {
    /// Whether open session `by` started open session `key`, or something it started did, by
    /// the links kept across restarts.
    pub(super) fn started_by(&mut self, key: Key, by: Key) -> bool {
        let id = |k: Key| self.session_by_key(k).and_then(|s| s.session_id.clone());
        let (Some(child), Some(by), Some(project)) = (id(key), id(by), self.project_of(key)) else { return false };
        self.lineage_for(&project).ancestors(&child).contains(&by)
    }
}
