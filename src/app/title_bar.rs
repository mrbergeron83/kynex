//! The custom title bar: app name, one tab per project, Settings, and window buttons.
//! OpenRouter credit and the update pill are in the footer.

use crate::ui::px;
use gpui::{
    Anchor, AnyElement, ClickEvent, Context, DragMoveEvent, FontWeight, Hsla, MouseButton, SharedString, Window,
    canvas, div, prelude::*,
};
use kynex_core::paths::project_name;
use kynex_core::tab_order;

use super::KynexApp;
use super::keys::{MoveTabLeft, MoveTabRight, OpenProject, OpenSettings, PROJECTS_AT};
use super::menus::Menu;
use crate::chrome::{self, TITLE_BAR_HEIGHT};
use crate::theme::{mix, theme, ui_font};
use crate::ui::mark::mark;
use crate::ui::{Icon, Lamp, Tooltip, icon, icon_button, lamp, popover, section_label};

/// A project tab on the move: the one picked up and the slot it's shown in.
#[derive(Clone, Copy)]
pub(super) struct TabDrag {
    from: usize,
    to: usize,
}

/// What follows the pointer while a project tab is dragged.
#[derive(Clone)]
pub(super) struct DraggedTab {
    from: usize,
    label: SharedString,
    colour: Option<Hsla>,
}

impl Render for DraggedTab {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        popover(cx)
            .font_family(ui_font())
            .flex_row()
            .items_center()
            .gap_1p5()
            .h(px(26.))
            .px_2p5()
            .text_sm()
            .text_color(t.text)
            .child(icon(Icon::Folder, self.colour.unwrap_or(t.text)).size_3p5())
            .child(self.label.clone())
    }
}

impl KynexApp {
    pub(super) fn render_title_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = theme(cx).clone();
        let hover = t.hover;
        // The mark's lamp reflects the active project's most urgent agent state.
        let shown = self.active_project().map_or(Lamp::Idle, |p| self.project_lamp(&p.path));
        if self.tab_drag.is_some() && !cx.has_active_drag() {
            self.tab_drag = None;
        }
        let n = self.saved.projects.len();
        let order = match self.tab_drag {
            Some(d) => tab_order::preview(n, d.from, d.to),
            None => (0..n).collect(),
        };
        *self.tab_spans.borrow_mut() = vec![(0., 0.); n];
        let mut tabs = Vec::with_capacity(n);
        for (slot, i) in order.into_iter().enumerate() {
            let p = &self.saved.projects[i];
            let state = self.project_lamp(&p.path);
            let tooltip = p.path.display().to_string();
            let colour = self.project_colour(&p.path, &t);
            tabs.push(self.render_tab(
                i + PROJECTS_AT,
                slot,
                project_name(&p.path).into(),
                colour,
                Some(tooltip),
                state,
                cx,
            ));
        }

        let colour = self.active_colour(&t);
        div()
            .id("title-bar")
            .flex()
            .flex_none()
            .items_center()
            .h(TITLE_BAR_HEIGHT)
            .map(|d| match colour {
                Some(c) => {
                    d.bg(mix(t.title_bar, c, if t.is_light() { 0.22 } else { 0.16 })).border_b_2().border_color(c)
                }
                None => d.bg(t.title_bar).border_b_1().border_color(t.border),
            })
            .pl(chrome::leading_inset() + gpui::px(10.))
            .child(
                div()
                    .id("kynex-menu")
                    .flex()
                    .items_center()
                    .gap_2()
                    .mr_2()
                    .pl_1()
                    .pr_2()
                    .h(px(28.))
                    .rounded_md()
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover))
                    .child(mark("title-mark", gpui::px(22.), shown, cx))
                    .child(section_label("Kynex", cx).font_weight(FontWeight::EXTRA_BOLD))
                    .on_click(cx.listener(|this, ev: &ClickEvent, window, cx| {
                        let at = gpui::point(ev.position().x, TITLE_BAR_HEIGHT.to_pixels(window.rem_size()));
                        this.open_menu(Menu::Kynex, at, Anchor::TopLeft, cx)
                    })),
            )
            .child(div().flex().items_center().gap_0p5().children(tabs))
            .child(
                icon_button("add-project", Icon::Plus, cx)
                    .ml_1()
                    .tooltip(Tooltip::for_action("Open project folder", OpenProject, ""))
                    .on_click(cx.listener(|this, _, _, cx| this.pick_project(cx))),
            )
            .child(chrome::drag_area(div().id("drag").flex_1().h_full()))
            .child(
                icon_button("app-settings", Icon::Settings, cx)
                    .mr_2()
                    .tooltip(Tooltip::for_action("Settings", OpenSettings, ""))
                    .on_click(cx.listener(|this, _, _, cx| this.open_app_settings(cx))),
            )
            .child(chrome::window_controls(window, cx))
    }

    #[allow(clippy::too_many_arguments)]
    fn render_tab(
        &self,
        pos: usize,
        slot: usize,
        label: SharedString,
        colour: Option<Hsla>,
        tooltip: Option<String>,
        state: Lamp,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx).clone();
        let active = self.tab_pos() == Some(pos);
        let hover = t.hover;
        let fg = if active { t.text } else { t.text_muted };
        let lifted = self.tab_drag.is_some_and(|d| d.from + PROJECTS_AT == pos);
        let drag = DraggedTab { from: pos.saturating_sub(PROJECTS_AT), label: label.clone(), colour };
        let spans = self.tab_spans.clone();
        let record = canvas(
            move |b, _, _| {
                if let Some(span) = spans.borrow_mut().get_mut(slot) {
                    *span = (f32::from(b.left()), f32::from(b.right()));
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0();
        div()
            .id(("tab", pos))
            .relative()
            .flex()
            .items_center()
            .gap_1p5()
            .h(px(26.))
            .px_2p5()
            .rounded_md()
            .text_sm()
            .text_color(fg)
            .cursor_pointer()
            .when(active, |d| d.bg(t.active))
            .when(!active, |d| d.hover(move |s| s.bg(hover)))
            .child(icon(Icon::Folder, colour.unwrap_or(fg)).size_3p5())
            .child(label)
            .map(|d| match state {
                Lamp::Idle => d,
                s => d.child(lamp(("tab-lamp", pos), s, cx)),
            })
            .when_some(tooltip, |d, tip| {
                d.tooltip(Tooltip::with_meta(tip, "Middle-click to close · right-click for more"))
            })
            .on_click(cx.listener(move |this, _, window, cx| this.jump(pos, window, cx)))
            .child(record)
            .when(lifted, |d| d.opacity(0.35))
            .on_drag(drag, |d, _, _, cx| cx.new(|_| d.clone()))
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(move |this, _, _, cx| {
                    this.close_project(pos - PROJECTS_AT, cx);
                    cx.notify();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, ev: &gpui::MouseDownEvent, _, cx| {
                    this.open_menu(Menu::Tab(pos - PROJECTS_AT), ev.position, Anchor::TopLeft, cx)
                }),
            )
            .into_any_element()
    }

    pub(super) fn drag_tab(&mut self, ev: &DragMoveEvent<DraggedTab>, _: &mut Window, cx: &mut Context<Self>) {
        let from = ev.drag(cx).from;
        let spans = self.tab_spans.borrow().clone();
        let started = self.tab_drag.is_none();
        let drag = self.tab_drag.get_or_insert(TabDrag { from, to: from });
        let to = tab_order::slot(drag.to, &spans, f32::from(ev.event.position.x));
        if started || to != drag.to {
            drag.to = to;
            self.menu = None;
            cx.notify();
        }
    }

    pub(super) fn drop_tab(&mut self, _: &DraggedTab, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(d) = self.tab_drag.take() {
            self.move_project(d.from, d.to);
            cx.notify();
        }
    }

    pub(super) fn move_tab_left(&mut self, _: &MoveTabLeft, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(i) = self.saved.active_project
            && i > 0
        {
            self.move_project(i, i - 1);
            cx.notify();
        }
    }

    pub(super) fn move_tab_right(&mut self, _: &MoveTabRight, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(i) = self.saved.active_project {
            self.move_project(i, i + 1);
            cx.notify();
        }
    }

    fn move_project(&mut self, from: usize, to: usize) {
        if from == to || from >= self.saved.projects.len() || to >= self.saved.projects.len() {
            return;
        }
        tab_order::move_item(&mut self.saved.projects, from, to);
        if let Some(a) = self.saved.active_project {
            self.saved.active_project = Some(tab_order::moved(a, from, to));
        }
        self.confirm_close = None;
        self.save();
    }
}
