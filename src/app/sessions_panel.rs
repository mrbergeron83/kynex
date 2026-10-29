//! The resizable panel left of the transcript: the folder's open and past
//! sessions (in their groups, see `session_groups.rs`), new / switch / delete,
//! and the project's Claude settings button.

use std::collections::HashMap;
use std::path::PathBuf;

use gpui::{
    Anchor, AnyElement, Context, Focusable, MouseButton, MouseDownEvent, SharedString, Window, div, prelude::*, px,
};
use kynex_core::dates::{self, Bucket};
use kynex_core::fmt::ago;
use kynex_core::live::Key;
use kynex_core::session::{self, Item, Session, SessionInfo};

use kynex_core::toasts::Toast;

use super::KynexApp;
use super::keys::{NewSession, ToggleSessions};
use super::menus::Menu;
use super::session_groups::DraggedSession;
use crate::theme::{Palette, theme};
use crate::ui::motion::grow;
use crate::ui::wash::row_wash;
use crate::ui::{Icon, Lamp, Tooltip, icon_button, lamp, section_label};

/// Past sessions for the active conversation's folder, re-read when the folder
/// or the current session changes, or a reply starts or finishes.
pub struct SessionList {
    pub(super) cwd: PathBuf,
    current: Option<String>,
    /// The folder's open sessions and whether each is busy: titles and times
    /// move when a reply starts or finishes.
    live: Vec<(Option<String>, bool)>,
    pub items: Vec<SessionInfo>,
    /// Each session's local day of last activity, for the date headers;
    /// worked out once per read, not every frame.
    days: HashMap<String, dates::Day>,
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
}

impl KynexApp {
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
                .child(self.render_sessions_panel(cx))
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
            self.saved.sessions_width,
            leaving,
        ))
    }

    fn render_sessions_panel(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
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
                waiting: s.waiting(),
                finished: s.finished,
            })
            .collect();
        let live_sig: Vec<(Option<String>, bool)> = open.iter().map(|o| (o.id.clone(), o.busy)).collect();
        // Re-read when the folder or session changes, and when a reply starts or finishes (titles and times move).
        if self.sessions.as_ref().is_none_or(|l| l.cwd != cwd || l.current != current || l.live != live_sig) {
            let items = session::list_sessions(&cwd);
            let days = items.iter().map(|i| (i.id.clone(), dates::local_day(i.modified))).collect();
            self.sessions =
                Some(SessionList { items, days, cwd: cwd.clone(), current: current.clone(), live: live_sig });
        }
        let all = self.sessions.as_ref().unwrap().items.clone();
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
                icon_button("hide-sessions", Icon::PanelLeft, cx)
                    .tooltip(Tooltip::for_action("Hide sessions", ToggleSessions, ""))
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_sessions(&ToggleSessions, window, cx))),
            );

        let mut rows: Vec<AnyElement> = Vec::new();
        // Open sessions with no transcript listed yet: new ones, before or just after their first message.
        let unlisted = open.iter().filter(|o| o.id.as_ref().is_none_or(|id| !all.iter().any(|i| i.id == *id)));
        for o in unlisted {
            let (uid, is_current) = (o.uid, o.uid == current_uid);
            let title = o.first.clone().unwrap_or_else(|| "New session".into());
            if !matches(&title) {
                continue;
            }
            rows.push(
                session_row(("open", uid as usize), title.into(), is_current, &t)
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
            let (menu_id, drag) = (id.clone(), DraggedSession { id: id.clone(), title: info.title.clone().into() });
            // When it was last used is in the tooltip; the row is just its title.
            let row = session_row(("session", n), info.title.clone().into(), is_current, &t)
                .group("session-row")
                .tooltip(Tooltip::with_meta(info.title.clone(), ago(info.modified)))
                .children(news_lamp(("session-status", n), busy, waiting, finished, cx))
                .on_drag(drag, |d, _, _, cx| cx.new(|_| d.clone()))
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                        this.open_menu(Menu::Session(menu_id.clone()), ev.position, Anchor::TopLeft, cx)
                    }),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.confirm_delete = None;
                    this.show_code_view();
                    if !is_current {
                        this.switch_session(kind2, cwd2.clone(), open_id.clone());
                    }
                    this.focus_input(window, cx);
                    cx.notify();
                }))
                .child(
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
                            if is_current {
                                this.open_session(Session::new(kind2, cwd3.clone()));
                            }
                            let open = this.live_mut().and_then(|l| {
                                l.sessions
                                    .iter()
                                    .find(|s| s.session_id.as_deref() == Some(del_id.as_str()))
                                    .map(|s| s.uid)
                            });
                            if let Some(uid) = open {
                                this.close_open(uid);
                            }
                            if let Err(e) = session::delete_session(&cwd3, &del_id) {
                                this.toast(Toast::error(e));
                            }
                            this.drafts.remove(&del_id);
                            if this.saved.drafts.remove(&del_id).is_some() {
                                this.save();
                            }
                            this.edit_groups(&cwd3, |g| g.forget(&del_id));
                            this.sessions = None;
                            cx.notify();
                        })),
                );
            past.insert(id, row.into_any_element());
        }

        // Ungrouped first, then each group, then the archive.
        let arranged = self.groups_for(&cwd).arrange(items.iter().map(|i| i.id.as_str()));
        let mut take = |ids: Vec<String>| ids.iter().filter_map(|id| past.remove(id)).collect::<Vec<_>>();
        // The ungrouped sessions under date headers, unless filtering or they
        // all fall on one: then a flat list.
        let days = &self.sessions.as_ref().unwrap().days;
        let today = dates::today();
        let dated: Vec<(String, dates::Day)> =
            arranged.top.iter().map(|id| (id.clone(), days.get(id).copied().unwrap_or(today))).collect();
        match dates::sections(&dated, today).filter(|_| query.is_empty()) {
            Some(sections) => {
                for (bucket, ids) in sections {
                    let open = self.groups_for(&cwd).date_open(bucket);
                    rows.push(self.date_header(&cwd, bucket, ids.len(), open, today, cx));
                    let shown = take(ids);
                    if open {
                        rows.extend(shown);
                    }
                }
            }
            None => rows.extend(take(arranged.top)),
        }
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

        div().flex().flex_col().size_full().child(header).children(filter).child(
            div()
                .id("session-list")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .px_1p5()
                .pb_1()
                // Dropped outside any group: back to the top level.
                .on_drop(cx.listener(move |this, d: &DraggedSession, _, cx| {
                    this.edit_groups(&drop_cwd, |g| g.move_to(&d.id, None));
                    cx.notify();
                }))
                .children(rows),
        )
    }

    /// A date header: one line of small muted text that folds its sessions
    /// away (with their count) or opens them.
    fn date_header(
        &self,
        cwd: &std::path::Path,
        bucket: Bucket,
        count: usize,
        open: bool,
        today: dates::Day,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx).clone();
        let label = bucket.label(today);
        let cwd = cwd.to_path_buf();
        let muted = t.text_muted;
        div()
            .id(SharedString::from(format!("date-{}", bucket.key())))
            .flex()
            .items_end()
            .h(px(26.))
            .px_2()
            .pb_1()
            .text_xs()
            .text_color(t.text_placeholder)
            .cursor_pointer()
            .hover(move |s| s.text_color(muted))
            .child(if open { label } else { format!("{label} · {count}") })
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
        .child(div().flex_1().min_w_0().truncate().text_color(if current { t.text } else { t.text_muted }).child(title))
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
