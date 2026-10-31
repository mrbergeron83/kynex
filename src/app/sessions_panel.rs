//! The resizable panel left of the transcript: the folder's open and past
//! sessions (in their groups, see `session_groups.rs`), new / switch / delete,
//! and the project's Claude settings button.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::ui::px;
use gpui::{
    Anchor, AnyElement, App, Context, Focusable, MouseButton, MouseDownEvent, SharedString, Window, div, prelude::*,
};
use kynex_core::dates::{self, Bucket};
use kynex_core::fmt::ago;
use kynex_core::live::Key;
use kynex_core::session::{self, Item, SessionInfo};

use kynex_core::toasts::Toast;

use super::KynexApp;
use super::keys::{NewSession, ToggleSessions};
use super::menus::Menu;
use super::session_groups::DraggedSession;
use crate::theme::{Palette, mono, theme};
use crate::ui::motion::grow;
use crate::ui::wash::row_wash;
use crate::ui::{Icon, Lamp, Tooltip, icon_button, lamp, scrollbar, section_label};

/// Past sessions for the active conversation's folder, re-read when the folder
/// or the current session changes, or a reply starts or finishes.
pub struct SessionList {
    /// What it was read for.
    pub(super) read: ListRead,
    pub items: Vec<SessionInfo>,
    /// Each session's local day of last activity, for the date headers;
    /// worked out once per read, not every frame.
    days: HashMap<String, dates::Day>,
    /// The repository's default branch: sessions on another one say theirs.
    default_branch: Option<String>,
    /// Something it shows changed (a rename, a setting): read it again.
    stale: bool,
}

/// What the sessions list is read again for when it changes.
#[derive(Clone, PartialEq)]
pub struct ListRead {
    pub(super) cwd: PathBuf,
    current: Option<String>,
    /// The folder's open sessions and whether each is busy: titles and times
    /// move when a reply starts or finishes.
    live: Vec<(Option<String>, bool)>,
}

/// An open session as the sessions panel shows it.
struct OpenSession {
    uid: Key,
    /// The CLI's session id, once it has one.
    id: Option<String>,
    /// The first thing the user said, for a title before the transcript lists it.
    first: Option<String>,
    busy: bool,
    waiting: bool,
    finished: bool,
    /// It runs on OpenRouter (the row shows so).
    openrouter: bool,
}

impl KynexApp {
    /// Reads the folder's past sessions off the UI thread (each transcript's
    /// ends); the panel keeps showing the last list until they're in.
    fn read_sessions(&mut self, read: ListRead, cx: &mut Context<Self>) {
        self.sessions_reading = Some(read.clone());
        let cwd = read.cwd.clone();
        cx.spawn(async move |this, cx| {
            let (mut items, default_branch) = cx
                .background_spawn(async move { (session::list_sessions(&cwd), kynex_core::git::default_branch(&cwd)) })
                .await;
            this.update(cx, |this, cx| {
                // A newer read is on its way.
                if this.sessions_reading.as_ref() != Some(&read) {
                    return;
                }
                this.sessions_reading = None;
                // A name given by hand wins, and counts as named, so none is generated.
                for i in &mut items {
                    if let Some(name) = this.given_name(&read.cwd, &i.id) {
                        i.title = name;
                        i.named = true;
                    }
                }
                let days = items.iter().map(|i| (i.id.clone(), dates::local_day(i.modified))).collect();
                this.sessions = Some(SessionList { read, items, days, default_branch, stale: false });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Has the sessions list read again, keeping it on screen meanwhile.
    pub(super) fn reread_sessions(&mut self) {
        if let Some(l) = &mut self.sessions {
            l.stale = true;
        }
    }

    /// The panel with its drag-to-resize right edge, or nothing while collapsed.
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
                .child(self.render_sessions_panel(window, cx))
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

    fn render_sessions_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = theme(cx).clone();
        let Some(live) = self.live_mut() else { return div() };
        let Some(s) = live.current() else { return div() };
        let (kind, cwd, current, current_uid) = (s.kind, s.cwd.clone(), s.session_id.clone(), s.uid);
        let open: Vec<OpenSession> = live
            .sessions
            .iter()
            .map(|s| OpenSession {
                uid: s.uid,
                id: s.session_id.clone(),
                first: s.items.iter().find_map(|i| match i {
                    Item::User { text, .. } if !text.is_empty() => {
                        Some(text.split_whitespace().collect::<Vec<_>>().join(" "))
                    }
                    _ => None,
                }),
                busy: s.busy,
                openrouter: s.provider().provider() == kynex_core::provider::Provider::OpenRouter,
                waiting: s.waiting(),
                finished: s.finished,
            })
            .collect();
        let live: Vec<(Option<String>, bool)> = open.iter().map(|o| (o.id.clone(), o.busy)).collect();
        // Re-read when the folder or session changes, and when a reply starts or finishes (titles and times move).
        let read = ListRead { cwd: cwd.clone(), current: current.clone(), live };
        let fresh = self.sessions.as_ref().is_some_and(|l| !l.stale && l.read == read);
        if !fresh && self.sessions_reading.as_ref() != Some(&read) {
            self.read_sessions(read, cx);
        }
        // Until the first read for this folder is in, the list is empty.
        let all = self.sessions.as_ref().filter(|l| l.read.cwd == cwd).map(|l| l.items.clone()).unwrap_or_default();
        self.name_sessions(&all, cx);
        // The title filter, while it's open.
        let query = if self.filtering_sessions {
            self.session_filter.read(cx).text().trim().to_lowercase()
        } else {
            String::new()
        };
        let matches = |title: &str| query.is_empty() || title.to_lowercase().contains(&query);
        let shown: Vec<SessionInfo> = all.iter().filter(|i| matches(&i.title)).cloned().collect();
        let items = &shown;
        self.sync_picks(&cwd, &items.iter().map(|i| i.id.clone()).collect::<Vec<_>>());
        let picking = self.organize.selection.on;
        let busy: Vec<String> = open.iter().filter(|o| o.busy).filter_map(|o| o.id.clone()).collect();
        // Working, waiting for you, or finished and unseen: the Active section.
        let active: Vec<String> =
            open.iter().filter(|o| o.busy || o.waiting || o.finished).filter_map(|o| o.id.clone()).collect();
        let default_branch = self.sessions.as_ref().and_then(|l| l.default_branch.clone());
        let cursor = self.organize.cursor.clone().filter(|_| self.sessions_focus.is_focused(window));
        let picked_ids = self.organize.selection.ids.clone();
        let open_state = |id: &str| {
            open.iter()
                .find(|o| o.id.as_deref() == Some(id))
                .map_or((false, false, false), |o| (o.busy, o.waiting, o.finished))
        };

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
            .child({
                let cwd = cwd.clone();
                icon_button("new-group", Icon::FolderPlus, cx)
                    .tooltip(Tooltip::text("New group"))
                    .on_click(cx.listener(move |this, _, window, cx| this.new_group(cwd.clone(), window, cx)))
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
                    .tooltip(Tooltip::text("Tidy old sessions"))
                    .on_click(cx.listener(|this, ev: &gpui::ClickEvent, _, cx| {
                        this.open_menu(Menu::SessionsPanel, ev.position(), Anchor::TopLeft, cx)
                    })),
            )
            .child(
                icon_button("hide-sessions", Icon::PanelLeft, cx)
                    .tooltip(Tooltip::for_action("Hide sessions", ToggleSessions, ""))
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_sessions(&ToggleSessions, window, cx))),
            );

        let mut rows: Vec<AnyElement> = Vec::new();
        // Open sessions with no transcript listed yet: new ones, before or just
        // after their first message. Working ones join the Active section.
        let (mut fresh, mut fresh_active): (Vec<AnyElement>, Vec<AnyElement>) = (Vec::new(), Vec::new());
        let unlisted = open.iter().filter(|o| o.id.as_ref().is_none_or(|id| !all.iter().any(|i| i.id == *id)));
        for o in unlisted {
            let (uid, is_current) = (o.uid, o.uid == current_uid);
            let title = o.first.clone().unwrap_or_else(|| "New session".into());
            if !matches(&title) {
                continue;
            }
            let to = if o.busy || o.waiting || o.finished { &mut fresh_active } else { &mut fresh };
            to.push(
                session_row(("open", uid as usize), title.into(), is_current, &t)
                    .when(o.openrouter, |r| r.child(openrouter_mark(("open-provider", uid as usize), &t)))
                    .children(news_lamp(("open-status", uid as usize), o.busy, o.waiting, o.finished, cx))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.confirm_delete = None;
                        this.show_code_view();
                        if !is_current {
                            this.show_open(|s| s.uid == uid);
                        }
                        this.focus_input(window, cx);
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        }
        if items.is_empty() {
            rows.push(
                div()
                    .px_3()
                    .py_2()
                    .text_sm()
                    .text_color(t.text_placeholder)
                    .child(if query.is_empty() { "No past sessions yet" } else { "No sessions match" })
                    .into_any_element(),
            );
        }
        // Each past session's row, placed below by group.
        let mut past: HashMap<String, AnyElement> = HashMap::new();
        for (n, info) in items.iter().enumerate() {
            let is_current = current.as_deref() == Some(info.id.as_str());
            let confirming = self.confirm_delete.as_deref() == Some(info.id.as_str());
            let id = info.id.clone();
            let (open_id, del_id) = (id.clone(), id.clone());
            let (kind2, cwd2, cwd3) = (kind, cwd.clone(), cwd.clone());
            let picked = self.organize.selection.contains(&id);
            let (busy, waiting, finished) = open_state(&id);
            let delete = if confirming {
                div()
                    .id(("confirm-delete", n))
                    .px_1p5()
                    .h(px(20.))
                    .flex()
                    .items_center()
                    .rounded_sm()
                    .text_xs()
                    .text_color(t.error)
                    .cursor_pointer()
                    .hover(|s| s.bg(t.hover))
                    .child("Delete?")
            } else {
                icon_button(("delete", n), Icon::Trash, cx).invisible().group_hover("session-row", |s| s.visible())
            };
            // A picked row takes the other picked ones with it, in a drag or its menu.
            let with_picks = picked && picked_ids.len() > 1;
            let group_ids = if with_picks { picked_ids.clone() } else { vec![id.clone()] };
            let drag = DraggedSession { ids: group_ids.clone(), title: info.title.clone().into() };
            let menu = if with_picks { Menu::Sessions(group_ids) } else { Menu::Session(id.clone()) };
            let elsewhere = info.ran_elsewhere(&cwd, default_branch.as_deref()).map(str::to_string);
            // When it was last used is in the tooltip; the row is just its title,
            // or the field renaming it.
            let title = match self.session_name_field(&id, false, cx) {
                Some(field) => field.flex_1().into_any_element(),
                None => {
                    let text = match &elsewhere {
                        // Only as wide as the title, so the branch follows it.
                        Some(_) => div()
                            .min_w_0()
                            .truncate()
                            .text_color(if is_current || picked { t.text } else { t.text_muted })
                            .child(info.title.clone())
                            .into_any_element(),
                        None => title_text(info.title.clone().into(), is_current || picked, &t),
                    };
                    match &elsewhere {
                        // Where it ran, after its title: small, muted, mono.
                        Some(branch) => div()
                            .flex()
                            .flex_1()
                            .min_w_0()
                            .items_center()
                            .gap_1p5()
                            .child(text)
                            .child(
                                div()
                                    .flex_none()
                                    .max_w(px(120.))
                                    .truncate()
                                    .font_family(mono())
                                    .text_xs()
                                    .text_color(t.text_placeholder)
                                    .child(branch.clone()),
                            )
                            .into_any_element(),
                        None => text,
                    }
                }
            };
            let title = match picking {
                true => div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap_1p5()
                    .child(self.pick_box(picked, cx))
                    .child(title)
                    .into_any_element(),
                false => title,
            };
            let on_openrouter = self.groups_for(&cwd).provider_of(&id) == kynex_core::provider::Provider::OpenRouter;
            let row = session_row_with(("session", n), title, is_current || picked, &t)
                .when(on_openrouter, |r| r.child(openrouter_mark(("session-provider", n), &t)))
                .group("session-row")
                .tooltip(Tooltip::with_meta(
                    info.title.clone(),
                    match &elsewhere {
                        Some(branch) => format!("{} · on {branch}", ago(info.modified)),
                        None => ago(info.modified),
                    },
                ))
                .when(cursor.as_deref() == Some(id.as_str()), |row| row.bg(t.hover))
                .children(news_lamp(("session-status", n), busy, waiting, finished, cx))
                .on_drag(drag, |d, _, _, cx| cx.new(|_| d.clone()))
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                        this.open_menu(menu.clone(), ev.position, Anchor::TopLeft, cx)
                    }),
                )
                .on_click(cx.listener(move |this, ev: &gpui::ClickEvent, window, cx| {
                    this.confirm_delete = None;
                    this.organize.cursor = Some(open_id.clone());
                    if this.pick_click(&cwd2, &open_id, &ev.modifiers(), cx) {
                        // Picking: the list keeps the keys (Delete, Shift+Delete, Esc).
                        this.sessions_focus.focus(window, cx);
                        return;
                    }
                    this.show_code_view();
                    if !is_current {
                        this.switch_session(kind2, cwd2.clone(), open_id.clone());
                    }
                    this.focus_input(window, cx);
                    cx.notify();
                }))
                .when(!picking, |row| {
                    row.child(
                        delete
                            .when(busy, |d| d.opacity(0.4))
                            .tooltip(Tooltip::text(if busy {
                                "Stop it first to delete it"
                            } else {
                                "Delete this session's transcript"
                            }))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                if busy {
                                    return;
                                }
                                if this.confirm_delete.as_deref() != Some(del_id.as_str()) {
                                    this.confirm_delete = Some(del_id.clone());
                                    return cx.notify();
                                }
                                this.confirm_delete = None;
                                this.forget_past_session(kind2, &cwd3, &del_id);
                                if let Err(e) = session::delete_session(&cwd3, &del_id) {
                                    this.toast(Toast::error(e));
                                }
                                this.sessions = None;
                                cx.notify();
                            })),
                    )
                });
            // A pinned row takes other sessions dropped on it in its place,
            // which reorders the pinned ones.
            let row = match self.groups_for(&cwd).is_pinned(&id) {
                true => {
                    let (pin_id, pin_cwd, hover) = (id.clone(), cwd.clone(), t.hover);
                    row.drag_over::<DraggedSession>(move |s, _, _, _| s.bg(hover)).on_drop(cx.listener(
                        move |this, d: &DraggedSession, _, cx| {
                            cx.stop_propagation();
                            this.edit_groups(&pin_cwd, |g| {
                                d.ids.iter().filter(|i| **i != pin_id).for_each(|i| g.move_pin(i, Some(&pin_id)))
                            });
                            cx.notify();
                        },
                    ))
                }
                false => row,
            };
            past.insert(id, row.into_any_element());
        }

        // Active first, then pinned, the dates, each group, the archive.
        let arranged = self.groups_for(&cwd).arrange_with(items.iter().map(|i| i.id.as_str()), &active);
        // Headers only when there's more than one section to tell apart.
        let headers = arranged.sections() + usize::from(arranged.active.is_empty() && !fresh_active.is_empty()) > 1;
        let mut take = |ids: Vec<String>| ids.iter().filter_map(|id| past.remove(id)).collect::<Vec<_>>();
        // The rows in the order drawn, for Shift+click runs, "All" and the list's keys.
        let mut order: Vec<String> = arranged.active.clone();
        order.extend(arranged.pinned.iter().cloned());
        // A new session, before its first message: above everything.
        rows.extend(fresh);
        let active_rows: Vec<AnyElement> = fresh_active.into_iter().chain(take(arranged.active)).collect();
        if !active_rows.is_empty() {
            if headers {
                rows.push(list_header("Active", rows.is_empty(), cx).into_any_element());
            }
            rows.extend(active_rows);
        }
        let pinned = take(arranged.pinned);
        if !pinned.is_empty() {
            if headers {
                rows.push(list_header("Pinned", rows.is_empty(), cx).into_any_element());
            }
            rows.extend(pinned);
        }
        // The ungrouped sessions under date headers, unless filtering or
        // headers aren't drawn: then a flat list.
        let no_days = HashMap::new();
        let days = self.sessions.as_ref().map_or(&no_days, |l| &l.days);
        let today = dates::today();
        let dated: Vec<(String, dates::Day)> =
            arranged.top.iter().map(|id| (id.clone(), days.get(id).copied().unwrap_or(today))).collect();
        let sections = match headers && query.is_empty() {
            // With other sections about, even a single day gets its header.
            true => dates::sections(&dated, today).or_else(|| dates::lone_section(&dated, today)),
            false => dates::sections(&dated, today).filter(|_| query.is_empty()),
        };
        match sections {
            Some(sections) => {
                for (bucket, ids) in sections {
                    let open = self.groups_for(&cwd).date_open(bucket);
                    let first = rows.is_empty();
                    rows.push(self.date_header(&cwd, bucket, ids.len(), open, today, first, cx));
                    if open {
                        order.extend(ids.iter().cloned());
                    }
                    let shown = take(ids);
                    if open {
                        rows.extend(shown);
                    }
                }
            }
            None => {
                order.extend(arranged.top.iter().cloned());
                rows.extend(take(arranged.top));
            }
        }
        for (i, ids) in arranged.groups.iter().enumerate() {
            if !self.groups_for(&cwd).groups.get(i).is_some_and(|g| g.collapsed) {
                order.extend(ids.iter().cloned());
            }
        }
        if self.organize.show_archived {
            order.extend(arranged.archived.iter().cloned());
        }
        self.organize.shown = order.clone();
        let groups: Vec<Vec<AnyElement>> = arranged.groups.into_iter().map(&mut take).collect();
        let archived = take(arranged.archived);
        for (i, group) in groups.into_iter().enumerate() {
            // Filtering, groups with nothing that matches step aside.
            if !query.is_empty() && group.is_empty() {
                continue;
            }
            rows.push(self.render_group(&cwd, i, group, cx));
        }
        if !archived.is_empty() {
            rows.push(self.render_archived(&cwd, archived, cx));
        }
        let drop_cwd = cwd.clone();
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

        let scroll = scrollbar::handle("session-list");
        let list = div()
            .id("session-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&scroll)
            .px_1p5()
            .pb_1()
            // Dropped outside any group: back to the top level.
            .on_drop(cx.listener(move |this, d: &DraggedSession, _, cx| {
                this.edit_groups(&drop_cwd, |g| d.ids.iter().for_each(|id| g.move_to(id, None)));
                this.organize.selection.clear();
                cx.notify();
            }))
            // Its own keys while it has the focus: ↑ / ↓, Enter, F2, Delete, Shift+Delete.
            .key_context("SessionList")
            .track_focus(&self.sessions_focus)
            .on_action(cx.listener(Self::list_up))
            .on_action(cx.listener(Self::list_down))
            .on_action(cx.listener(Self::list_open))
            .on_action(cx.listener(Self::list_rename))
            .on_action(cx.listener(Self::list_archive))
            .on_action(cx.listener(Self::list_delete))
            .children(rows);
        let picks = self.render_pick_bar(kind, cwd, order, busy, cx);
        div()
            .flex()
            .flex_col()
            .size_full()
            .child(header)
            .children(filter)
            .child(scrollbar::vertical_over("session-list-bar", scroll, list, |d| d.flex_1().min_h_0()))
            .children(picks)
    }

    /// A date header: a section label that folds its sessions away (with
    /// their count) or opens them. `first` is the list's first row.
    fn date_header(
        &self,
        cwd: &std::path::Path,
        bucket: Bucket,
        count: usize,
        open: bool,
        today: dates::Day,
        first: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx).clone();
        let label = bucket.label(today);
        let cwd = cwd.to_path_buf();
        let text = t.text;
        list_header(&if open { label } else { format!("{label} · {count}") }, first, cx)
            .id(SharedString::from(format!("date-{}", bucket.key())))
            .cursor_pointer()
            .hover(move |s| s.text_color(text))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.edit_groups(&cwd, |g| g.toggle_date(bucket));
                cx.notify();
            }))
            .into_any_element()
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

fn session_row(
    id: impl Into<gpui::ElementId>,
    title: SharedString,
    current: bool,
    t: &Palette,
) -> gpui::Stateful<gpui::Div> {
    session_row_with(id, title_text(title, current, t), current, t)
}

/// A session row whose name is `title`: its text, or the field renaming it.
fn session_row_with(
    id: impl Into<gpui::ElementId>,
    title: AnyElement,
    current: bool,
    t: &Palette,
) -> gpui::Stateful<gpui::Div> {
    let hover = t.hover;
    div()
        .id(id)
        .flex()
        .items_center()
        .gap_1()
        .h(px(28.))
        .pl_2()
        .pr_1()
        .rounded_md()
        .cursor_pointer()
        .text_sm()
        .relative()
        .child(row_wash(current, hover, t.selection))
        .child(title)
}

/// A small mark on the row of a session that runs on OpenRouter.
fn openrouter_mark(id: impl Into<gpui::ElementId>, t: &Palette) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex_none()
        .px_1()
        .rounded_sm()
        .border_1()
        .border_color(t.border_variant)
        .font_family(mono())
        .text_xs()
        .text_color(t.text_placeholder)
        .child("OR")
        .tooltip(Tooltip::text("Runs on OpenRouter"))
}

/// A row's name as text, brighter on the shown session.
/// A section header in the list (Pinned, a date): the section label, with
/// less room above the list's first one.
fn list_header(text: &str, first: bool, cx: &App) -> gpui::Div {
    let label = section_label(text, cx).px_2().pb_1();
    if first { label.pt_1p5() } else { label.pt_3() }
}

fn title_text(title: SharedString, current: bool, t: &Palette) -> AnyElement {
    div()
        .flex_1()
        .min_w_0()
        .truncate()
        .text_color(if current { t.text } else { t.text_muted })
        .child(title)
        .into_any_element()
}

/// A session's lamp, only when there's news: it's working, waiting for you,
/// or just finished. Idle rows show none.
fn news_lamp(
    id: impl Into<gpui::ElementId>,
    busy: bool,
    waiting: bool,
    finished: bool,
    cx: &mut Context<KynexApp>,
) -> Option<AnyElement> {
    (busy || waiting || finished).then(|| lamp(id, Lamp::of(busy, waiting, finished), cx).into_any_element())
}
