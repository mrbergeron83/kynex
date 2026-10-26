//! Popup menus: model, theme, permission mode, effort, and a project tab's
//! right-click menu. One is open at a time, anchored where it was clicked.

use gpui::{Anchor, AnyElement, Context, Pixels, Point, anchored, deferred, div, prelude::*, px};
use kynex_core::open::Opener;
use kynex_core::project::{self, ClaudeSettings, Mode};

use super::{KynexApp, models};
use crate::theme::{self, THEMES, theme};
use crate::ui::{Icon, Tooltip, divider, icon, menu_item, popover};

#[derive(Clone, PartialEq)]
pub enum Menu {
    Model,
    Theme,
    Mode,
    Effort,
    Tab(usize),
}

pub struct OpenMenu {
    which: Menu,
    at: Point<Pixels>,
    anchor: Anchor,
}

impl KynexApp {
    /// Opens `which` at `at`, or closes it if it's already open.
    pub(super) fn open_menu(&mut self, which: Menu, at: Point<Pixels>, anchor: Anchor, cx: &mut Context<Self>) {
        self.menu = if self.menu.as_ref().is_some_and(|m| m.which == which) {
            None
        } else {
            Some(OpenMenu { which, at, anchor })
        };
        cx.notify();
    }

    pub(super) fn render_menu(&mut self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let open = self.menu.as_ref()?;
        let (which, at, anchor) = (open.which.clone(), open.at, open.anchor);
        let t = theme(cx).clone();
        let mut items: Vec<AnyElement> = Vec::new();
        match &which {
            Menu::Model => {
                for (n, m) in models::models().into_iter().enumerate() {
                    let id = if m.value == "default" { String::new() } else { m.value.clone() };
                    let selected = self.saved.model == id;
                    let hover = t.hover;
                    items.push(
                        div()
                            .id(("model", n))
                            .flex()
                            .items_center()
                            .gap_2()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .cursor_pointer()
                            .hover(move |s| s.bg(hover))
                            .when(selected, |d| d.bg(t.active))
                            .child(
                                div()
                                    .flex_1()
                                    .flex()
                                    .flex_col()
                                    .child(div().text_sm().text_color(t.text).child(m.name.clone()))
                                    .when(!m.description.is_empty(), |d| {
                                        d.child(div().text_xs().text_color(t.text_muted).child(m.description.clone()))
                                    }),
                            )
                            .when(selected, |d| d.child(icon(Icon::Check, t.text_muted)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.saved.model = id.clone();
                                this.menu = None;
                                this.save();
                                cx.notify();
                            }))
                            .into_any_element(),
                    );
                }
            }
            Menu::Theme => {
                for (n, (th, name)) in THEMES.iter().enumerate() {
                    let th = *th;
                    items.push(
                        menu_item(("theme", n), *name, self.saved.theme == th, cx)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.saved.theme = th;
                                this.menu = None;
                                this.save();
                                theme::set(th, cx);
                                cx.refresh_windows();
                            }))
                            .into_any_element(),
                    );
                }
            }
            Menu::Mode => {
                let current = self.active_session().map(|s| s.mode);
                for (n, m) in Mode::ALL.into_iter().enumerate() {
                    items.push(
                        menu_item(("mode", n), m.label(), current == Some(m), cx)
                            .h_auto()
                            .py_1()
                            .child(div())
                            .tooltip(Tooltip::text(m.help()))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(s) = this.session_mut() {
                                    s.set_mode(m);
                                }
                                this.menu = None;
                                cx.notify();
                            }))
                            .into_any_element(),
                    );
                }
            }
            Menu::Effort => {
                // From the settings modal, or from the composer for the active project.
                let path = self.settings.as_ref().map(|s| s.path.clone()).or_else(|| self.active_project().map(|p| p.path.clone()));
                let current = match (&self.settings, &path) {
                    (Some(s), _) => s.claude.effort.clone(),
                    (None, Some(p)) => ClaudeSettings::load(p).effort,
                    _ => String::new(),
                };
                // Only the levels the selected model takes, once the CLI has said which.
                let allowed = models::current_model(&self.saved.model).map(|m| m.efforts);
                let levels = project::EFFORTS
                    .iter()
                    .filter(|(id, _)| id.is_empty() || allowed.as_ref().is_none_or(|a| a.iter().any(|e| e == id)));
                for (n, (id, name)) in levels.enumerate() {
                    let id = id.to_string();
                    let path = path.clone();
                    items.push(
                        menu_item(("effort", n), *name, current == id, cx)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.menu = None;
                                if let Some(path) = &path {
                                    this.set_effort(path.clone(), id.clone(), cx);
                                }
                                cx.notify();
                            }))
                            .into_any_element(),
                    );
                }
            }
            Menu::Tab(i) => {
                let i = *i;
                let path = self.saved.projects.get(i).map(|p| p.path.clone()).unwrap_or_default();
                for (n, (label, opener)) in [
                    ("Open in VS Code", Opener::Code),
                    ("Open in file manager", Opener::Explorer),
                    ("Open terminal", Opener::Terminal),
                ]
                .into_iter()
                .enumerate()
                {
                    let path = path.clone();
                    items.push(
                        menu_item(("tab-menu", n), label, false, cx)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.open_in(opener, &path);
                                this.menu = None;
                                cx.notify();
                            }))
                            .into_any_element(),
                    );
                }
                items.push(div().my_1().child(divider(cx)).into_any_element());
                items.push(
                    menu_item("tab-close", "Close", false, cx)
                        .text_color(t.error)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.menu = None;
                            this.close_project(i);
                            cx.notify();
                        }))
                        .into_any_element(),
                );
            }
        }
        Some(deferred(
            anchored().position(at).anchor(anchor).snap_to_window_with_margin(px(8.)).child(
                popover(cx)
                    .min_w(px(200.))
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        this.menu = None;
                        cx.notify();
                    }))
                    .children(items),
            ),
        ))
    }
}
