//! The Agents pane: everything running or wanting you, in every open project, from
//! `kynex_core::running`. A row goes to what it lists; its menu stops a session, opens it
//! to the side, or goes to the prompt it waits on. Built only while the pane is drawn.

use std::path::Path;

use gpui::{AnyElement, Context, MouseButton, MouseDownEvent, Window, div, prelude::*};
use kynex_core::layout::{Direction, PaneId};
use kynex_core::running::{self, Row, State, Who};
use kynex_core::tiling::{Content, Tool};
use kynex_core::workspace::same_folder;

use super::KynexApp;
use super::menus::Menu;
use super::state::Active;
use crate::theme::theme;
use crate::ui::pane_drag::pane_handle;
use crate::ui::{Icon, Lamp, Tooltip, icon_button, lamp, menu_item, px, section_label};

/// A folder's last part, for a workspace or project name.
fn folder_name(p: &Path) -> String {
    p.file_name().map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into_owned())
}

/// What a row's menu item does.
type RowAction = Box<dyn Fn(&mut KynexApp, &mut Window, &mut Context<KynexApp>)>;

fn lamp_of(state: State) -> Lamp {
    match state {
        State::Waiting => Lamp::NeedsYou,
        State::Finished => Lamp::Done,
        State::Working => Lamp::Working,
    }
}

impl KynexApp {
    /// Where something runs, for its row: the workspace, and the project when it isn't the
    /// one shown.
    fn place(&self, project: &Path, cwd: &Path) -> String {
        let shown = self.active_project().is_some_and(|p| same_folder(&p.path, project));
        let (ws, proj) = (folder_name(cwd), folder_name(project));
        match (shown, same_folder(project, cwd)) {
            (true, _) | (false, true) => ws,
            (false, false) => format!("{ws} · {proj}"),
        }
    }

    /// An open session's name: the one given or listed, else its first message.
    fn running_name(&self, s: &kynex_core::session::Session) -> String {
        let listed = s.session_id.as_deref().and_then(|id| {
            self.sessions.get(&s.cwd).and_then(|l| l.items.iter().find(|i| i.id == id)).map(|i| i.title.clone())
        });
        listed.or_else(|| self.session_title(s.uid)).unwrap_or_else(|| "New session".into())
    }

    /// What runs or wants you, in the order shown.
    fn running_rows(&self) -> Vec<(usize, Row)> {
        let mut rows = Vec::new();
        let mut add = |s: &kynex_core::session::Session, place: String| {
            if let Some(state) = State::of_session(s) {
                rows.push(Row {
                    who: Who::Session(s.uid),
                    state,
                    order: s.uid,
                    name: self.running_name(s),
                    place,
                    doing: running::doing(s),
                    // Until sessions know who started them (#399).
                    parent: None,
                });
            }
        };
        for s in &self.chat.sessions {
            add(s, "Chat".into());
        }
        for (project, live) in &self.code {
            for s in &live.sessions {
                add(s, self.place(project, &s.cwd));
            }
        }
        for (cwd, t) in &self.tiles {
            let project = self.tile_projects.get(cwd).cloned().unwrap_or_else(|| cwd.clone());
            for (pane, id) in t.agents() {
                let state = match self.agent_lamp(pane) {
                    Lamp::NeedsYou => State::Waiting,
                    Lamp::Done => State::Finished,
                    Lamp::Working => State::Working,
                    _ => continue,
                };
                let doing = match state {
                    State::Waiting => "Wants you",
                    State::Finished => "Finished",
                    State::Working => "Working",
                };
                rows.push(Row {
                    who: Who::Agent(pane),
                    state,
                    order: pane,
                    name: self.agent_name(&id),
                    place: self.place(&project, cwd),
                    doing: doing.into(),
                    parent: None,
                });
            }
        }
        running::arrange(rows)
    }

    /// The Agents pane `pane`.
    pub(super) fn render_agents_pane(&mut self, pane: PaneId, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx).clone();
        let header = div()
            .id("agents-header")
            .flex()
            .flex_none()
            .items_center()
            .h(px(32.))
            .pl_3()
            .pr_1()
            .border_b_1()
            .border_color(t.border_variant)
            .child(section_label("Agents", cx).flex_1())
            .child(
                icon_button("close-agents", Icon::Close, cx)
                    .tooltip(Tooltip::for_action("Close the Agents pane", super::keys::ToggleAgents, ""))
                    .on_click(cx.listener(move |this, _, window, cx| this.close_tool_pane(pane, window, cx))),
            );
        let rows = self.running_rows();
        let body: Vec<AnyElement> = if rows.is_empty() {
            vec![
                div()
                    .px_3()
                    .py_2()
                    .text_sm()
                    .text_color(t.text_placeholder)
                    .child("Nothing running")
                    .into_any_element(),
            ]
        } else {
            rows.into_iter().enumerate().map(|(n, (depth, row))| self.running_row(n, depth, row, cx)).collect()
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(t.panel)
            .child(pane_handle(header, Some(pane), "Agents"))
            .child(div().id("agents-list").flex_1().min_h_0().overflow_y_scroll().p_1().children(body))
            .into_any_element()
    }

    fn running_row(&mut self, n: usize, depth: usize, row: Row, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx).clone();
        let who = row.who;
        div()
            .id(("agents-row", n))
            .flex()
            .items_center()
            .gap_2()
            .min_h(px(28.))
            .py_0p5()
            .pl(px(8. + 14. * depth as f32))
            .pr_2()
            .rounded_md()
            .cursor_pointer()
            .text_sm()
            .hover(|s| s.bg(t.hover))
            .child(lamp(("agents-lamp", n), lamp_of(row.state), cx))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .min_w_0()
                            .child(div().min_w_0().truncate().text_color(t.text).child(row.name))
                            .child(div().flex_none().text_xs().text_color(t.text_placeholder).child(row.place)),
                    )
                    .when(!row.doing.is_empty(), |d| {
                        d.child(div().truncate().text_xs().text_color(t.text_muted).child(row.doing))
                    }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.open_menu(Menu::Running(who), ev.position, gpui::Anchor::TopLeft, cx)
                }),
            )
            .on_click(cx.listener(move |this, _, window, cx| this.go_to_running(who, window, cx)))
            .into_any_element()
    }

    /// Goes to what a row lists: its project and workspace come up, and its pane takes the
    /// focus (a session in no pane opens in the focused one).
    fn go_to_running(&mut self, who: Who, window: &mut Window, cx: &mut Context<Self>) {
        match who {
            Who::Session(key) => self.go_to_session(key, window, cx),
            Who::Agent(pane) => {
                let Some(cwd) = self.tiles.iter().find(|(_, t)| t.has(pane)).map(|(c, _)| c.clone()) else { return };
                let project = self.tile_projects.get(&cwd).cloned().unwrap_or_else(|| cwd.clone());
                if let Some(i) = self.saved.projects.iter().position(|p| same_folder(&p.path, &project)) {
                    self.select(Active::Project(i), window, cx);
                }
                self.select_workspace(&cwd, cx);
                self.sync_tiles(window, cx);
                self.focus_pane(pane, window, cx);
                self.focus_input(window, cx);
                cx.notify();
            }
        }
    }

    /// A row's right-click menu.
    pub(super) fn running_menu_items(&mut self, who: Who, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut items: Vec<(&'static str, RowAction)> = Vec::new();
        match who {
            Who::Session(key) => {
                let Some(s) = self.session_by_key(key) else { return Vec::new() };
                let (busy, state, id) = (s.busy, State::of_session(s), s.session_id.clone());
                let project = self.code.iter().find(|(_, l)| l.sessions.iter().any(|s| s.uid == key)).map(|(p, _)| p);
                let here = project.is_some_and(|p| self.active_project().is_some_and(|a| same_folder(&a.path, p)));
                let in_pane = self.tiles.values().any(|t| t.pane_of(key).is_some());
                let go = if state == Some(State::Waiting) { "Go to its prompt" } else { "Go to it" };
                items.push((go, Box::new(move |this, window, cx| this.go_to_session(key, window, cx))));
                if let Some(id) = id.filter(|_| here && !in_pane) {
                    items.push((
                        "Open to the side",
                        Box::new(move |this, window, cx| {
                            this.open_beside(id.clone(), None, Some(Direction::Right), window, cx)
                        }),
                    ));
                }
                if busy {
                    items.push((
                        "Stop",
                        Box::new(move |this, _, _| {
                            if let Some(s) = this.session_for(key) {
                                s.interrupt();
                            }
                        }),
                    ));
                }
            }
            Who::Agent(_) => {
                items.push(("Go to it", Box::new(move |this, window, cx| this.go_to_running(who, window, cx))));
            }
        }
        items
            .into_iter()
            .enumerate()
            .map(|(i, (label, f))| {
                menu_item(("running-menu", i), label, false, cx)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.menu = None;
                        f(this, window, cx);
                        cx.notify();
                    }))
                    .into_any_element()
            })
            .collect()
    }

    /// The Agents pane on the right of pane `target`, or focused if the workspace has it.
    pub(super) fn show_agents_pane(&mut self, target: PaneId, window: &mut Window, cx: &mut Context<Self>) {
        self.open_tool(target, Tool::Agents, 0.7, window, cx);
    }

    /// The key and the palette: the tab's Agents pane away, or beside the session.
    pub(super) fn toggle_agents(&mut self, _: &super::keys::ToggleAgents, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, ws, _)) = self.tiling_scope() else { return };
        let Some(t) = self.tiles.get(&ws) else { return };
        match (t.tab_panes_of(Content::Agents).first().copied(), t.session_focus()) {
            (Some(p), _) => self.close_tool_pane(p, window, cx),
            (None, Some(target)) => self.show_agents_pane(target, window, cx),
            (None, None) => {}
        }
        cx.notify();
    }
}
