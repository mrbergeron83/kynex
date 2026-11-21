//! The Agents pane: every active `agent_cli` pane across all open projects that is
//! working, waiting for input, or finished. Clicking a row jumps directly to that pane.

use std::path::Path;

use gpui::{AnyElement, Context, MouseButton, MouseDownEvent, Window, div, prelude::*};
use kynex_core::layout::PaneId;
use kynex_core::running::{self, Row, State};
use kynex_core::tiling::{Content, Tool};
use kynex_core::workspace::same_folder;

use super::KynexApp;
use super::menus::Menu;
use crate::theme::{mono, theme};
use crate::ui::pane_drag::pane_handle;
use crate::ui::{Icon, Lamp, Tooltip, icon_button, lamp, menu_item, px, section_label};

fn folder_name(p: &Path) -> String {
    p.file_name().map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into_owned())
}

fn lamp_of(state: State) -> Lamp {
    match state {
        State::Waiting => Lamp::NeedsYou,
        State::Finished => Lamp::Done,
        State::Working => Lamp::Working,
    }
}

impl KynexApp {
    fn place(&self, project: &Path, cwd: &Path) -> String {
        let shown = self.active_project().is_some_and(|p| same_folder(&p.path, project));
        let (ws, proj) = (folder_name(cwd), folder_name(project));
        match (shown, same_folder(project, cwd)) {
            (true, _) | (false, true) => ws,
            (false, false) => format!("{ws} · {proj}"),
        }
    }

    fn running_rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        for inst in self.agent_instances.values() {
            let Some(provider_id) = inst.meta.provider.as_deref() else { continue };
            let Some(state) = State::of_agent(inst.tracker.state()) else { continue };
            let project = self.tile_projects.get(&inst.cwd).cloned().unwrap_or_else(|| inst.cwd.clone());
            let detail = inst.tracker.detail();
            let doing = if !detail.is_empty() && state == State::Working {
                detail.to_string()
            } else {
                match state {
                    State::Waiting => "Needs you".into(),
                    State::Finished => "Finished".into(),
                    State::Working => "Working".into(),
                }
            };
            rows.push(Row {
                pane: inst.id,
                state,
                name: self.agent_title(&inst.cwd, &inst.meta),
                provider_badge: self.provider_badge(provider_id),
                place: self.place(&project, &inst.cwd),
                doing,
            });
        }
        running::arrange(rows)
    }

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
                    .child("No active agents")
                    .into_any_element(),
            ]
        } else {
            rows.into_iter().enumerate().map(|(n, row)| self.running_row(n, row, cx)).collect()
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

    fn running_row(&mut self, n: usize, row: Row, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx).clone();
        let pane = row.pane;
        div()
            .id(("agents-row", n))
            .flex()
            .items_center()
            .gap_2()
            .min_h(px(28.))
            .py_0p5()
            .px_2()
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
                            .child(
                                div()
                                    .flex_none()
                                    .px_1()
                                    .rounded_sm()
                                    .border_1()
                                    .border_color(t.border_variant)
                                    .font_family(mono())
                                    .text_size(px(10.))
                                    .text_color(t.text_muted)
                                    .child(row.provider_badge),
                            )
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
                    this.open_menu(Menu::Running(pane), ev.position, gpui::Anchor::TopLeft, cx)
                }),
            )
            .on_click(cx.listener(move |this, _, window, cx| this.go_to_agent(pane, window, cx)))
            .into_any_element()
    }

    pub(super) fn running_menu_items(&mut self, agent_id: PaneId, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let t = theme(cx).clone();
        let mut items = vec![
            menu_item("running-go", "Go to session", false, cx)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.menu = None;
                    this.go_to_agent(agent_id, window, cx);
                }))
                .into_any_element(),
        ];
        if self.instance_lamp(agent_id) == Lamp::Working {
            items.push(
                menu_item("running-stop", "Send Ctrl+C", false, cx)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.menu = None;
                        if let Some(view) = this.agent_instances.get(&agent_id).and_then(|a| a.view.clone()) {
                            view.update(cx, |v, _| v.write(vec![0x03]));
                        }
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        }
        items.push(
            menu_item("running-close", "Stop session", false, cx)
                .text_color(t.error)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.menu = None;
                    this.stop_agent(agent_id, window, cx);
                }))
                .into_any_element(),
        );
        items
    }

    pub(super) fn show_agents_pane(&mut self, target: PaneId, window: &mut Window, cx: &mut Context<Self>) {
        self.open_tool(target, Tool::Agents, 0.7, window, cx);
    }

    pub(super) fn agents_pane_open(&self) -> bool {
        self.tiling_scope()
            .and_then(|(_, ws)| self.tiles.get(&ws))
            .is_some_and(|t| !t.panes_of(Content::Agents).is_empty())
    }

    pub(super) fn toggle_agents(&mut self, _: &super::keys::ToggleAgents, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, ws)) = self.tiling_scope() else { return };
        let Some(t) = self.tiles.get(&ws) else { return };
        match (t.panes_of(Content::Agents).first().copied(), t.agent_focus().or_else(|| t.focus())) {
            (Some(p), _) => self.close_tool_pane(p, window, cx),
            (None, Some(target)) => self.show_agents_pane(target, window, cx),
            (None, None) => {}
        }
        cx.notify();
    }
}
