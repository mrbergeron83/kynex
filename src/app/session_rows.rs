//! A session's row in the Sessions tree: active `agent_cli` pane or past session on disk.

use std::path::{Path, PathBuf};

use crate::ui::px;
use gpui::{Anchor, AnyElement, App, Context, MouseButton, MouseDownEvent, SharedString, Window, div, prelude::*};
use kynex_core::agent_cli;
use kynex_core::fmt::ago;
use kynex_core::sessions::{self, SessionInfo};
use kynex_core::toasts::Toast;

use super::KynexApp;
use super::agent_cli::AgentId;
use super::menus::Menu;
use super::session_groups::DraggedSession;
use crate::theme::{Palette, mono};
use crate::ui::wash::row_wash;
use crate::ui::{Icon, Lamp, Tooltip, icon_button, lamp, section_label};

/// A live `agent_cli` instance (visible or parked in the background) as the Sessions tree shows it.
pub(super) struct OpenSession {
    pub agent_id: AgentId,
    pub id: Option<String>,
    pub title: String,
    pub provider_badge: String,
    pub provider_name: String,
    pub busy: bool,
    pub lamp: Lamp,
}

impl OpenSession {
    pub fn key(&self) -> String {
        self.id.clone().unwrap_or_else(|| new_key(self.agent_id))
    }

    pub fn has_news(&self) -> bool {
        self.lamp != Lamp::Idle
    }
}

pub(super) fn new_key(agent_id: AgentId) -> String {
    format!("agent:{agent_id}")
}

pub(super) fn key_agent(key: &str) -> Option<AgentId> {
    key.strip_prefix("agent:").or_else(|| key.strip_prefix("pane:"))?.parse().ok()
}

pub(super) fn session_row(
    id: impl Into<gpui::ElementId>,
    title: SharedString,
    current: bool,
    t: &Palette,
) -> gpui::Stateful<gpui::Div> {
    session_row_with(id, title_text(title, current, t), current, t)
}

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

fn provider_mark(id: impl Into<gpui::ElementId>, text: String, tip: String, t: &Palette) -> gpui::Stateful<gpui::Div> {
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

pub(super) fn list_header(text: &str, first: bool, cx: &App) -> gpui::Div {
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

fn live_lamp(
    id: impl Into<gpui::ElementId>,
    live: bool,
    state: Lamp,
    cx: &mut Context<KynexApp>,
) -> Option<AnyElement> {
    (live || state != Lamp::Idle).then(|| lamp(id, state, cx).into_any_element())
}

pub(super) struct RowCtx<'a> {
    pub cwd: PathBuf,
    pub here: bool,
    pub current: Option<String>,
    pub current_agent: Option<AgentId>,
    pub picking: bool,
    pub cursor: Option<String>,
    pub picked_ids: Vec<String>,
    pub default_branch: Option<String>,
    pub open: &'a [OpenSession],
    pub t: Palette,
}

impl RowCtx<'_> {
    fn open_match(&self, id: &str) -> Option<&OpenSession> {
        self.open.iter().find(|o| o.id.as_deref() == Some(id))
    }
}

impl KynexApp {
    pub(super) fn show_in_workspace(&mut self, cwd: &Path, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.select_workspace(cwd, cx);
        self.sync_tiles(window, cx);
        self.switch_session(cwd.to_path_buf(), id.to_string(), window, cx);
    }

    pub(super) fn fresh_row(&mut self, o: &OpenSession, ctx: &RowCtx, cx: &mut Context<Self>) -> AnyElement {
        let t = &ctx.t;
        let agent_id = o.agent_id;
        let is_current = ctx.here && ctx.current_agent == Some(agent_id);
        let key = o.key();
        let show_badge = o.provider_badge != "Claude";
        let n = agent_id as usize;
        session_row(("open", n), o.title.clone().into(), is_current, t)
            .group("session-row")
            .when(ctx.cursor.as_deref() == Some(key.as_str()), |row| row.bg(t.hover))
            .when(show_badge, |r| {
                r.child(provider_mark(
                    ("open-provider", n),
                    o.provider_badge.clone(),
                    format!("Runs on {}", o.provider_name),
                    t,
                ))
            })
            .children(live_lamp(("open-status", n), true, o.lamp, cx))
            .child(
                icon_button(("open-stop", n), Icon::Close, cx)
                    .size(px(18.))
                    .invisible()
                    .group_hover("session-row", |s| s.visible())
                    .tooltip(Tooltip::text("Stop live session"))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.stop_agent(agent_id, window, cx);
                    })),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.open_menu(Menu::Running(agent_id), ev.position, Anchor::TopLeft, cx)
                }),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                this.confirm_delete = None;
                this.organize.cursor = Some(key.clone());
                this.go_to_agent(agent_id, window, cx);
            }))
            .into_any_element()
    }

    pub(super) fn past_row(
        &mut self,
        n: usize,
        info: &SessionInfo,
        ctx: &RowCtx,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let RowCtx { cwd, here, current, picking, cursor, picked_ids, default_branch, t, .. } = ctx;
        let (picking, here) = (*picking, *here);
        let is_current = here && current.as_deref() == Some(info.id.as_str());
        let confirming = self.confirm_delete.as_deref() == Some(info.id.as_str());
        let id = info.id.clone();
        let (open_id, del_id) = (id.clone(), id.clone());
        let (cwd2, cwd3) = (cwd.clone(), cwd.clone());
        let picked = self.organize.selection.contains(&id);
        let open_match = ctx.open_match(&id);
        let is_live = open_match.is_some();
        let state = open_match.map_or(Lamp::Idle, |o| o.lamp);
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
        let with_picks = picked && picked_ids.len() > 1;
        let group_ids = if with_picks { picked_ids.clone() } else { vec![id.clone()] };
        let drag = DraggedSession { cwd: cwd.clone(), ids: group_ids.clone(), title: info.title.clone().into() };
        let menu = if with_picks { Menu::Sessions(group_ids) } else { Menu::Session(id.clone()) };
        let elsewhere = info.ran_elsewhere(cwd, default_branch.as_deref()).map(str::to_string);

        let title = match self.session_name_field(&id, cx) {
            Some(field) => field.flex_1().into_any_element(),
            None => {
                let text = match &elsewhere {
                    Some(_) => div()
                        .min_w_0()
                        .truncate()
                        .text_color(if is_current || picked { t.text } else { t.text_muted })
                        .child(info.title.clone())
                        .into_any_element(),
                    None => title_text(info.title.clone().into(), is_current || picked, t),
                };
                match &elsewhere {
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
        let provider_id = self
            .groups_for(cwd)
            .providers
            .get(&id)
            .map(|p| agent_cli::normalize_provider_id(p))
            .unwrap_or_else(|| info.provider.clone());
        let badge = self.provider_badge(&provider_id);
        let provider_name = self.provider_display_name(&provider_id);
        let show_badge = provider_id != agent_cli::CLAUDE;
        let row = session_row_with(("session", n), title, is_current || picked, t)
            .when(show_badge, |r| {
                r.child(provider_mark(("session-provider", n), badge, format!("Runs on {provider_name}"), t))
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
            .children(live_lamp(("session-status", n), is_live, state, cx))
            .on_drag(drag, |d, _, _, cx| cx.new(|_| d.clone()))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.open_menu(menu.clone(), ev.position, Anchor::TopLeft, cx)
                }),
            )
            .on_click(cx.listener(move |this, ev: &gpui::ClickEvent, window, cx| {
                this.confirm_delete = None;
                this.organize.cursor = Some(open_id.clone());
                if this.pick_click(&cwd2, &open_id, &ev.modifiers(), cx) {
                    this.sessions_focus.focus(window, cx);
                    return;
                }
                if !here {
                    return this.show_in_workspace(&cwd2, &open_id, window, cx);
                }
                this.switch_session(cwd2.clone(), open_id.clone(), window, cx);
                cx.notify();
            }))
            .when(!picking, |row| {
                row.child(
                    delete
                        .when(busy, |d| d.opacity(0.4))
                        .tooltip(Tooltip::text(if busy {
                            "Stop it first to delete it"
                        } else {
                            "Delete this session's files"
                        }))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            if busy {
                                return;
                            }
                            if this.confirm_delete.as_deref() != Some(del_id.as_str()) {
                                this.confirm_delete = Some(del_id.clone());
                                return cx.notify();
                            }
                            this.confirm_delete = None;
                            this.forget_past_session(&cwd3, &del_id, window, cx);
                            this.relist_sessions(std::slice::from_ref(&del_id));
                            let (cwd, id) = (cwd3.clone(), del_id.clone());
                            cx.spawn(async move |this, cx| {
                                let deleted =
                                    cx.background_spawn(async move { sessions::delete_session(&cwd, &id) }).await;
                                this.update(cx, |this, cx| {
                                    if let Err(e) = deleted {
                                        this.toast(Toast::error(e));
                                    }
                                    this.reread_sessions();
                                    cx.notify();
                                })
                                .ok();
                            })
                            .detach();
                            cx.notify();
                        })),
                )
            });
        let row = match self.groups_for(cwd).is_pinned(&id) {
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
