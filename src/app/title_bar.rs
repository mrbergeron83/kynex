//! The custom title bar: app name, one tab per project (Chat first), Settings
//! and the window buttons. Usage limits and the update pill are in the footer.
//! The theme lives in Settings and the Kynex menu's Appearance.

use crate::ui::px;
use gpui::{
    Anchor, AnyElement, ClickEvent, Context, DragMoveEvent, FontWeight, Hsla, MouseButton, SharedString, Window,
    canvas, div, prelude::*,
};
use kynex_core::live::Live;
use kynex_core::paths::project_name;
use kynex_core::tab_order;

use super::KynexApp;
use super::keys::{MoveTabLeft, MoveTabRight, OpenProject, OpenSettings};
use super::menus::Menu;
use super::state::Active;
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
        // Drawn outside the window's root, so it doesn't inherit the font.
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
        // The mark's lamp shows the shown session.
        let hover = t.hover;
        let shown = self.active_session().map_or(Lamp::Idle, |s| Lamp::of(s.busy, s.waiting(), s.finished));
        let status = |l: &Live| (l.busy(), l.waiting(), l.finished());
        // A drag let go outside the window, or cancelled, never drops.
        if self.tab_drag.is_some() && !cx.has_active_drag() {
            self.tab_drag = None;
        }
        let n = self.saved.projects.len();
        let order = match self.tab_drag {
            Some(d) => tab_order::preview(n, d.from, d.to),
            None => (0..n).collect(),
        };
        *self.tab_spans.borrow_mut() = vec![(0., 0.); n];
        let (chat, chief) = (status(&self.chat), status(&self.chief.live));
        let chat = (chat.0 || chief.0, chat.1 || chief.1, chat.2 || chief.2);
        let mut tabs = vec![self.render_tab(0, 0, "Chat".into(), None, None, chat, cx)];
        for (slot, i) in order.into_iter().enumerate() {
            let p = &self.saved.projects[i];
            let state = self.code.get(&p.path).map_or((false, false, false), status);
            let tooltip = p.path.display().to_string();
            let colour = self.project_colour(&p.path, &t);
            tabs.push(self.render_tab(i + 1, slot, project_name(&p.path).into(), colour, Some(tooltip), state, cx));
        }

        // The shown project's colour tints the bar, with a line of it along the bottom.
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
            // The mark and name open the Kynex menu (About, updates).
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
                        // Drop the menu from under the bar rather than from the cursor.
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
            // The empty middle of the bar is the window's title bar.
            .child(chrome::drag_area(div().id("drag").flex_1().h_full()))
            .child(
                icon_button("app-settings", Icon::Settings, cx)
                    .mr_2()
                    .tooltip(Tooltip::for_action("Settings", OpenSettings, ""))
                    .on_click(cx.listener(|this, _, _, cx| this.open_app_settings(cx))),
            )
            .child(chrome::window_controls(window, cx))
    }

    /// `pos` is the tab's place in the bar (Chat is 0), `slot` where a project
    /// tab is shown while one is dragged.
    #[allow(clippy::too_many_arguments)]
    fn render_tab(
        &self,
        pos: usize,
        slot: usize,
        label: SharedString,
        colour: Option<Hsla>,
        tooltip: Option<String>,
        (busy, waiting, finished): (bool, bool, bool),
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx).clone();
        let active = self.tab_pos() == pos;
        let hover = t.hover;
        let fg = if active { t.text } else { t.text_muted };
        let lifted = self.tab_drag.is_some_and(|d| d.from + 1 == pos);
        let drag = DraggedTab { from: pos.saturating_sub(1), label: label.clone(), colour };
        let spans = self.tab_spans.clone();
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
            .child(icon(if pos == 0 { Icon::Message } else { Icon::Folder }, colour.unwrap_or(fg)).size_3p5())
            .child(label)
            .map(|d| match Lamp::of(busy, waiting, finished) {
                Lamp::Idle => d,
                state => d.child(lamp(("tab-lamp", pos), state, cx)),
            })
            .when_some(tooltip, |d, tip| {
                d.tooltip(Tooltip::with_meta(tip, "Middle-click to close · right-click for more"))
            })
            .on_click(cx.listener(move |this, _, window, cx| this.jump(pos, window, cx)))
            .when(pos > 0, |d| {
                // Where it is, for working out which slot a dragged tab is over.
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
                // Its own spot stays faint while the dragged copy follows the pointer.
                d.child(record)
                    .when(lifted, |d| d.opacity(0.35))
                    .on_drag(drag, |d, _, _, cx| cx.new(|_| d.clone()))
                    .on_mouse_down(
                        MouseButton::Middle,
                        cx.listener(move |this, _, _, cx| {
                            this.close_project(pos - 1);
                            cx.notify();
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, ev: &gpui::MouseDownEvent, _, cx| {
                            this.open_menu(Menu::Tab(pos - 1), ev.position, Anchor::TopLeft, cx)
                        }),
                    )
            })
            .into_any_element()
    }

    /// The pointer moved with a project tab held: show it in the slot it's over.
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

    /// Ctrl+Shift+PageUp / PageDown: the shown project tab one place over.
    pub(super) fn move_tab_left(&mut self, _: &MoveTabLeft, _: &mut Window, cx: &mut Context<Self>) {
        if let Active::Project(i) = self.saved.active
            && i > 0
        {
            self.move_project(i, i - 1);
            cx.notify();
        }
    }

    pub(super) fn move_tab_right(&mut self, _: &MoveTabRight, _: &mut Window, cx: &mut Context<Self>) {
        if let Active::Project(i) = self.saved.active {
            self.move_project(i, i + 1);
            cx.notify();
        }
    }

    /// Moves project tab `from` to `to`; the shown tab stays shown.
    fn move_project(&mut self, from: usize, to: usize) {
        if from == to || from >= self.saved.projects.len() || to >= self.saved.projects.len() {
            return;
        }
        tab_order::move_item(&mut self.saved.projects, from, to);
        if let Active::Project(a) = self.saved.active {
            self.saved.active = Active::Project(tab_order::moved(a, from, to));
        }
        // It named a tab by its old place.
        self.confirm_close = None;
        self.save();
    }
}
