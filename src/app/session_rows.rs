//! A session's row in the sidebar's tree: its name (or the field renaming it), where it
//! ran, its lamp, its menu, delete, drag, and picking. One look for every session,
//! listed or new, in whichever workspace it is.

use std::path::{Path, PathBuf};

use crate::ui::px;
use gpui::{Anchor, AnyElement, App, Context, MouseButton, MouseDownEvent, SharedString, Window, div, prelude::*};
use kynex_core::fmt::ago;
use kynex_core::live::Key;
use kynex_core::session::{self, Item, Kind, Session, SessionInfo};
use kynex_core::toasts::Toast;

use super::KynexApp;
use super::menus::Menu;
use super::session_groups::DraggedSession;
use crate::theme::{Palette, mono};
use crate::ui::wash::row_wash;
use crate::ui::{Icon, Lamp, Tooltip, icon_button, lamp, section_label};

/// An open session as the tree shows it.
pub(super) struct OpenSession {
    pub uid: Key,
    /// The CLI's session id, once it has one.
    pub id: Option<String>,
    /// The first thing the user said, for a title before the transcript lists it.
    pub first: Option<String>,
    pub busy: bool,
    pub waiting: bool,
    pub finished: bool,
    /// It runs on OpenRouter (the row shows so).
    pub openrouter: bool,
    /// It runs on Grok.
    pub grok: bool,
}

impl OpenSession {
    pub fn of(s: &Session) -> Self {
        OpenSession {
            uid: s.uid,
            id: s.session_id.clone(),
            first: s.items.iter().find_map(|i| match i {
                Item::User { text, .. } if !text.is_empty() => {
                    Some(text.split_whitespace().collect::<Vec<_>>().join(" "))
                }
                _ => None,
            }),
            busy: s.busy,
            openrouter: s.provider().provider().is_openrouter(),
            grok: s.provider().provider() == kynex_core::provider::Provider::Grok,
            waiting: s.waiting(),
            finished: s.finished,
        }
    }

    /// Its key in the tree's rows: its id, or before it has one, its place in Kynex.
    pub fn key(&self) -> String {
        self.id.clone().unwrap_or_else(|| new_key(self.uid))
    }

    pub fn lamp(&self) -> Lamp {
        Lamp::of(self.busy, self.waiting, self.finished)
    }

    /// Working, waiting for you, or finished and unseen.
    pub fn has_news(&self) -> bool {
        self.busy || self.waiting || self.finished
    }
}

/// The row key of an open session with no id yet.
pub(super) fn new_key(uid: Key) -> String {
    format!("new:{uid}")
}

/// The open session a `new:` row key stands for.
pub(super) fn key_uid(key: &str) -> Option<Key> {
    key.strip_prefix("new:")?.parse().ok()
}

pub(super) fn session_row(
    id: impl Into<gpui::ElementId>,
    title: SharedString,
    current: bool,
    t: &Palette,
) -> gpui::Stateful<gpui::Div> {
    session_row_with(id, title_text(title, current, t), current, t)
}

/// A session row whose name is `title`: its text, or the field renaming it.
fn session_row_with(id: impl Into<gpui::ElementId>, title: AnyElement, current: bool, t: &Palette) -> gpui::Stateful<gpui::Div> {
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

/// A small mark on a session's row: the provider it runs on when it isn't Anthropic.
fn provider_mark(id: impl Into<gpui::ElementId>, text: &'static str, tip: &'static str, t: &Palette) -> gpui::Stateful<gpui::Div> {
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
        .child(text)
        .tooltip(Tooltip::text(tip))
}

/// A section header in the tree (Pinned, Recent, Older): the section label, with less
/// room above the first one.
pub(super) fn list_header(text: &str, first: bool, cx: &App) -> gpui::Div {
    let label = section_label(text, cx).px_2().pb_1();
    if first { label.pt_1p5() } else { label.pt_3() }
}

/// A row's name as text, brighter on the shown session.
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
fn news_lamp(id: impl Into<gpui::ElementId>, state: Lamp, cx: &mut Context<KynexApp>) -> Option<AnyElement> {
    (state != Lamp::Idle).then(|| lamp(id, state, cx).into_any_element())
}

/// What a session's row needs from the workspace it's under.
pub(super) struct RowCtx<'a> {
    pub kind: Kind,
    /// The workspace's folder.
    pub cwd: PathBuf,
    /// It's the workspace on show: a click switches the shown session, rather than
    /// bringing the workspace up first.
    pub here: bool,
    /// The shown session's id.
    pub current: Option<String>,
    pub current_uid: Key,
    pub picking: bool,
    pub cursor: Option<String>,
    pub picked_ids: Vec<String>,
    pub default_branch: Option<String>,
    pub open: &'a [OpenSession],
    pub t: Palette,
}

impl RowCtx<'_> {
    fn open_state(&self, id: &str) -> Lamp {
        self.open.iter().find(|o| o.id.as_deref() == Some(id)).map_or(Lamp::Idle, OpenSession::lamp)
    }
}

impl KynexApp {
    /// Shows past session `id` of the workspace at `cwd`: in its pane if it's open,
    /// else the workspace comes up with it.
    pub(super) fn show_in_workspace(&mut self, cwd: &Path, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let open = self.live_mut().and_then(|l| l.sessions.iter().find(|s| s.session_id.as_deref() == Some(id)).map(|s| s.uid));
        if let Some(key) = open {
            return self.focus_open_session(key, window, cx);
        }
        if let Some(project) = self.workspace_project() {
            self.saved_workspaces(&project).remember(cwd, Some(id));
        }
        self.select_workspace(cwd, cx);
        let shown = self.live_mut().and_then(|l| l.current()).and_then(|s| s.session_id.clone());
        if shown.as_deref() != Some(id) {
            self.switch_session(Kind::Code, cwd.to_path_buf(), id.to_string());
        }
        self.focus_input(window, cx);
        cx.notify();
    }

    /// The row of an open session not listed yet: a new one, before or just after its
    /// first message.
    pub(super) fn fresh_row(&mut self, o: &OpenSession, ctx: &RowCtx, cx: &mut Context<Self>) -> AnyElement {
        let t = &ctx.t;
        let (uid, is_current, here) = (o.uid, ctx.here && o.uid == ctx.current_uid, ctx.here);
        let title = o.first.clone().unwrap_or_else(|| "New session".into());
        let key = o.key();
        session_row(("open", uid as usize), title.into(), is_current, t)
            .when(ctx.cursor.as_deref() == Some(key.as_str()), |row| row.bg(t.hover))
            .when(o.openrouter, |r| r.child(provider_mark(("open-provider", uid as usize), "OR", "Runs on OpenRouter", t)))
            .when(o.grok, |r| r.child(provider_mark(("open-grok", uid as usize), "Grok", "Runs on Grok", t)))
            .children(news_lamp(("open-status", uid as usize), o.lamp(), cx))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.confirm_delete = None;
                this.organize.cursor = Some(key.clone());
                if !here {
                    return this.focus_open_session(uid, window, cx);
                }
                if !is_current {
                    this.show_open(|s| s.uid == uid);
                }
                this.focus_input(window, cx);
                cx.notify();
            }))
            .into_any_element()
    }

    /// The row of listed session `info`, number `n` of its workspace's list.
    pub(super) fn past_row(&mut self, n: usize, info: &SessionInfo, ctx: &RowCtx, cx: &mut Context<Self>) -> AnyElement {
        let this = self;
        let RowCtx { kind, cwd, here, current, picking, cursor, picked_ids, default_branch, t, .. } = ctx;
        let (kind, picking, here) = (*kind, *picking, *here);
        let is_current = here && current.as_deref() == Some(info.id.as_str());
        let confirming = this.confirm_delete.as_deref() == Some(info.id.as_str());
        let id = info.id.clone();
        let (open_id, del_id) = (id.clone(), id.clone());
        let (cwd2, cwd3) = (cwd.clone(), cwd.clone());
        let picked = this.organize.selection.contains(&id);
        let state = ctx.open_state(&id);
        let busy = state == Lamp::Working;
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
        let drag = DraggedSession { cwd: cwd.clone(), ids: group_ids.clone(), title: info.title.clone().into() };
        let menu = if with_picks { Menu::Sessions(group_ids) } else { Menu::Session(id.clone()) };
        let elsewhere = info.ran_elsewhere(cwd, default_branch.as_deref()).map(str::to_string);
        // When it was last used is in the tooltip; the row is just its title,
        // or the field renaming it.
        let title = match this.session_name_field(&id, false, cx) {
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
                    None => title_text(info.title.clone().into(), is_current || picked, t),
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
                .child(this.pick_box(picked, cx))
                .child(title)
                .into_any_element(),
            false => title,
        };
        let provider = this.groups_for(cwd).provider_of(&id);
        let row = session_row_with(("session", n), title, is_current || picked, t)
            .when(provider.is_openrouter(), |r| {
                r.child(provider_mark(("session-provider", n), "OR", "Runs on OpenRouter", t))
            })
            .when(provider == kynex_core::provider::Provider::Grok, |r| {
                r.child(provider_mark(("session-grok", n), "Grok", "Runs on Grok", t))
            })
            .group("session-row")
            .tooltip(Tooltip::with_meta(
                info.title.clone(),
                match &elsewhere {
                    Some(branch) => format!("{} · on {branch}", ago(info.modified)),
                    None => ago(info.modified),
                },
            ))
            .when(cursor.as_deref() == Some(id.as_str()), |row| row.bg(t.hover))
            .children(news_lamp(("session-status", n), state, cx))
            .on_drag(drag, |d, _, _, cx| cx.new(|_| d.clone()))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                    // Not the pane's menu as well.
                    cx.stop_propagation();
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
                if !here {
                    return this.show_in_workspace(&cwd2, &open_id, window, cx);
                }
                if !is_current {
                    this.switch_session(kind, cwd2.clone(), open_id.clone());
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
                            this.forget_past_session(kind, &cwd3, &del_id);
                            if let Err(e) = session::delete_session(&cwd3, &del_id) {
                                this.toast(Toast::error(e));
                            }
                            this.relist_sessions(std::slice::from_ref(&del_id));
                            cx.notify();
                        })),
                )
            });
        // A pinned row takes other sessions dropped on it in its place,
        // which reorders the pinned ones.
        let row = match this.groups_for(cwd).is_pinned(&id) {
            true => {
                let (pin_id, pin_cwd, hover) = (id.clone(), cwd.clone(), t.hover);
                row.drag_over::<DraggedSession>(move |s, _, _, _| s.bg(hover)).on_drop(cx.listener(
                    move |this, d: &DraggedSession, _, cx| {
                        cx.stop_propagation();
                        if d.cwd != pin_cwd {
                            return;
                        }
                        this.edit_groups(&pin_cwd, |g| {
                            d.ids.iter().filter(|i| **i != pin_id).for_each(|i| g.move_pin(i, Some(&pin_id)))
                        });
                        cx.notify();
                    },
                ))
            }
            false => row,
        };
        row.into_any_element()
    }
}
