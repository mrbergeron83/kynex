//! The custom title bar: app name, one tab per project (Chat first), the
//! theme picker, Settings and the window buttons.

use gpui::{
    Anchor, AnyElement, ClickEvent, Context, FontWeight, Hsla, MouseButton, SharedString, Window, div, prelude::*, px,
};
use kynex_core::live::Live;
use kynex_core::paths::project_name;

use super::KynexApp;
use super::keys::{OpenProject, OpenSettings};
use super::menus::Menu;
use crate::chrome::{self, TITLE_BAR_HEIGHT};
use crate::theme::{THEMES, mix, theme};
use crate::ui::mark::mark;
use crate::ui::{Icon, Lamp, Tooltip, ghost_button, icon, icon_button, lamp, section_label};

impl KynexApp {
    pub(super) fn render_title_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = theme(cx).clone();
        // The mark's lamp shows the shown session.
        let hover = t.hover;
        let update_waiting = self.update_waiting();
        let shown = self.active_session().map_or(Lamp::Idle, |s| Lamp::of(s.busy, s.waiting(), s.finished));
        let status = |l: &Live| (l.busy(), l.waiting(), l.finished());
        let mut tabs = vec![self.render_tab(0, "Chat".into(), None, None, status(&self.chat), cx)];
        for (i, p) in self.saved.projects.iter().enumerate() {
            let state = self.code.get(&p.path).map_or((false, false, false), status);
            let tooltip = p.path.display().to_string();
            let colour = self.project_colour(&p.path, &t);
            tabs.push(self.render_tab(i + 1, project_name(&p.path).into(), colour, Some(tooltip), state, cx));
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
            .pl(chrome::leading_inset() + px(10.))
            // The mark and name open the Kynex menu (About, updates); a dot marks
            // an update waiting.
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
                    .child(mark("title-mark", px(22.), shown, cx))
                    .child(section_label("Kynex", cx).font_weight(FontWeight::EXTRA_BOLD))
                    .when(update_waiting, |d| d.child(div().size(px(6.)).rounded_full().bg(t.accent)))
                    .on_click(cx.listener(|this, ev: &ClickEvent, _, cx| {
                        // Drop the menu from under the bar rather than from the cursor.
                        let at = gpui::point(ev.position().x, TITLE_BAR_HEIGHT);
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
                ghost_button("theme", cx)
                    .mr_2()
                    .text_color(t.text_muted)
                    .child(THEMES.iter().find(|x| x.0 == self.saved.theme).map_or("", |x| x.1))
                    .child(icon(Icon::ChevronDown, t.text_placeholder).size_3())
                    .on_click(cx.listener(|this, ev: &ClickEvent, _, cx| {
                        this.open_menu(Menu::Theme, ev.position(), Anchor::TopRight, cx)
                    })),
            )
            .child(
                icon_button("app-settings", Icon::Settings, cx)
                    .mr_2()
                    .tooltip(Tooltip::for_action("Settings", OpenSettings, ""))
                    .on_click(cx.listener(|this, _, _, cx| this.open_app_settings(cx))),
            )
            .child(chrome::window_controls(window, cx))
    }

    fn render_tab(
        &self,
        pos: usize,
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
        div()
            .id(("tab", pos))
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
                d.on_mouse_down(
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
}
