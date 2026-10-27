//! The custom title bar: app name, one tab per project (Chat first), the
//! theme picker and the window buttons.

use gpui::{
    Anchor, AnyElement, ClickEvent, Context, FontWeight, MouseButton, SharedString, Window, div, prelude::*, px,
};
use kynex_core::live::Live;
use kynex_core::paths::project_name;

use super::KynexApp;
use super::menus::Menu;
use crate::chrome::{self, TITLE_BAR_HEIGHT};
use crate::theme::{THEMES, theme};
use crate::ui::mark::mark;
use crate::ui::{Icon, Lamp, Tooltip, ghost_button, icon, icon_button, lamp, section_label};

impl KynexApp {
    pub(super) fn render_title_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = theme(cx).clone();
        // The mark's lamp shows the shown session.
        let shown = self.active_session().map_or(Lamp::Idle, |s| Lamp::of(s.busy, s.waiting(), s.finished));
        let status = |l: &Live| (l.busy(), l.waiting(), l.finished());
        let mut tabs = vec![self.render_tab(0, "Chat".into(), Icon::Message, None, status(&self.chat), cx)];
        for (i, p) in self.saved.projects.iter().enumerate() {
            let state = self.code.get(&p.path).map_or((false, false, false), status);
            let tooltip = p.path.display().to_string();
            tabs.push(self.render_tab(i + 1, project_name(&p.path).into(), Icon::Folder, Some(tooltip), state, cx));
        }

        div()
            .id("title-bar")
            .flex()
            .flex_none()
            .items_center()
            .h(TITLE_BAR_HEIGHT)
            .bg(t.title_bar)
            .border_b_1()
            .border_color(t.border)
            .pl(chrome::leading_inset() + px(10.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .pr_3()
                    .child(mark("title-mark", px(20.), shown, cx))
                    .child(section_label("Kynex", cx).font_weight(FontWeight::EXTRA_BOLD)),
            )
            .child(div().flex().items_center().gap_0p5().children(tabs))
            .child(
                icon_button("add-project", Icon::Plus, cx)
                    .ml_1()
                    .tooltip(Tooltip::with_meta("Open project folder", "Ctrl+O"))
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
            .child(chrome::window_controls(window, cx))
    }

    fn render_tab(
        &self,
        pos: usize,
        label: SharedString,
        which: Icon,
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
            .child(icon(which, fg).size_3p5())
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
