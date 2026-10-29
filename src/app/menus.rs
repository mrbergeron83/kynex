//! Popup menus: model, appearance (theme and fonts), permission mode, effort, and the
//! right-click menus of project tabs, sessions and session groups. One is open at a time, anchored where it was clicked.

use gpui::{Anchor, AnyElement, Context, Pixels, Point, SharedString, anchored, deferred, div, prelude::*, px};
use kynex_core::motion::Motion;
use kynex_core::open::Opener;
use kynex_core::project::{self, Mode};
use kynex_core::update;

use super::{KynexApp, models};
use crate::theme::{self, DEFAULT_CODE_FONT, DEFAULT_UI_FONT, SUGGESTED_CODE_FONTS, SUGGESTED_UI_FONTS, THEMES, theme};
use crate::ui::motion::pop;
use crate::ui::{Icon, Tooltip, divider, icon, menu_item, popover, scrollbar};

#[derive(Clone, Debug, PartialEq)]
pub enum Menu {
    Model,
    Kynex,
    Theme,
    Font(FontRole),
    Mode,
    Effort,
    Tab(usize),
    /// A past session's right-click menu, by its id.
    Session(String),
    /// A session group's right-click menu.
    Group(usize),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FontRole {
    Ui,
    Code,
}

#[derive(Clone)]
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

    /// The open menu; `leaving` while it plays out after closing.
    pub(super) fn render_menu(&mut self, leaving: Option<f32>, cx: &mut Context<Self>) -> Option<AnyElement> {
        let open = self.menu.as_ref()?;
        let (which, at, anchor) = (open.which.clone(), open.at, open.anchor);
        let t = theme(cx).clone();
        let mut items: Vec<AnyElement> = Vec::new();
        // Font lists are long: they scroll inside the menu.
        let mut scroll = false;
        match &which {
            Menu::Kynex => {
                let update = self.update_waiting();
                items.push(
                    menu_item("kynex-about", "About Kynex", false, cx)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.menu = None;
                            this.open_about(cx);
                        }))
                        .into_any_element(),
                );
                items.push(
                    menu_item("kynex-check", if update { "Update available" } else { "Check for updates" }, false, cx)
                        .when(update, |d| d.text_color(t.accent))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.menu = None;
                            this.open_about(cx);
                            if !update {
                                this.check_for_update(cx);
                            }
                        }))
                        .into_any_element(),
                );
                items.push(
                    menu_item("kynex-settings", "Settings", false, cx)
                        .children(
                            crate::ui::shortcut(&super::keys::OpenSettings, cx)
                                .map(|k| div().flex_none().text_xs().text_color(t.text_placeholder).child(k)),
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.menu = None;
                            this.open_app_settings(cx);
                        }))
                        .into_any_element(),
                );
                // Theme, fonts and motion, a step away without a picker in the bar.
                let current = THEMES.iter().find(|x| x.0 == self.saved.theme).map_or("", |x| x.1);
                items.push(
                    menu_item("kynex-appearance", "Appearance", false, cx)
                        .child(div().flex_none().text_color(t.text_placeholder).child(current))
                        .child(icon(Icon::ChevronRight, t.text_placeholder).size_3())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.menu = Some(OpenMenu { which: Menu::Theme, at, anchor });
                            cx.notify();
                        }))
                        .into_any_element(),
                );
                items.push(
                    menu_item("kynex-extensions", "Extensions", false, cx)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.menu = None;
                            this.open_extensions(0, cx);
                        }))
                        .into_any_element(),
                );
                items.push(
                    menu_item("kynex-check-cli", "Check for Claude Code updates", false, cx)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.menu = None;
                            this.open_about(cx);
                            this.check_cli_now(cx);
                        }))
                        .into_any_element(),
                );
                items.push(div().my_1().child(divider(cx)).into_any_element());
                for (n, (label, url)) in [
                    ("Release notes", format!("https://github.com/{}/releases", update::REPO)),
                    ("Report an issue", format!("https://github.com/{}/issues/new", update::REPO)),
                ]
                .into_iter()
                .enumerate()
                {
                    items.push(
                        menu_item(("kynex-link", n), label, false, cx)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.menu = None;
                                cx.open_url(&url);
                                cx.notify();
                            }))
                            .into_any_element(),
                    );
                }
            }
            Menu::Model => {
                // In a project: whether picks apply everywhere or only here (#136).
                // Switching keeps the menu open; "This project" starts from the model in use.
                if let Some(path) = self.active_project().map(|p| p.path.clone()) {
                    let own = self.project_model().is_some();
                    let current = self.active_model();
                    let hover = t.hover;
                    let scope = |n: usize, label: &'static str, on: bool, cx: &mut Context<Self>| {
                        let path = path.clone();
                        let pin = (n == 1).then(|| current.clone());
                        div()
                            .id(("model-scope", n))
                            .flex_1()
                            .flex()
                            .justify_center()
                            .py_0p5()
                            .rounded_md()
                            .text_xs()
                            .cursor_pointer()
                            .text_color(if on { t.text } else { t.text_muted })
                            .when(on, |d| d.bg(t.active))
                            .when(!on, |d| d.hover(move |s| s.bg(hover)))
                            .child(label)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.set_project_model(path.clone(), pin.clone(), cx);
                                cx.notify();
                            }))
                    };
                    items.push(
                        div()
                            .flex()
                            .gap_1()
                            .p_0p5()
                            .mb_1()
                            .rounded_md()
                            .border_1()
                            .border_color(t.border_variant)
                            .child(scope(0, "Everywhere", !own, cx))
                            .child(scope(1, "This project", own, cx))
                            .into_any_element(),
                    );
                }
                let current = self.active_model();
                for (n, m) in models::models().into_iter().enumerate() {
                    let id = if m.value == "default" { String::new() } else { m.value.clone() };
                    let selected = current == id;
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
                                this.menu = None;
                                this.set_model(id.clone(), cx);
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
                                this.menu = None;
                                this.set_theme(th, cx);
                            }))
                            .into_any_element(),
                    );
                }
                items.push(div().my_1().child(divider(cx)).into_any_element());
                for (n, (role, label, current)) in
                    [(FontRole::Ui, "Interface font", theme::ui_font()), (FontRole::Code, "Code font", theme::mono())]
                        .into_iter()
                        .enumerate()
                {
                    items.push(
                        menu_item(("font-role", n), label, false, cx)
                            .child(div().flex_none().text_color(t.text_placeholder).child(current))
                            .child(icon(Icon::ChevronRight, t.text_placeholder).size_3())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.menu = Some(OpenMenu { which: Menu::Font(role), at, anchor });
                                cx.notify();
                            }))
                            .into_any_element(),
                    );
                }
                items.push(div().my_1().child(divider(cx)).into_any_element());
                for (n, m) in Motion::ALL.into_iter().enumerate() {
                    items.push(
                        menu_item(("motion", n), m.label(), self.saved.motion == m, cx)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.menu = None;
                                this.set_motion(m, cx);
                            }))
                            .into_any_element(),
                    );
                }
            }
            Menu::Font(role) => {
                let role = *role;
                scroll = true;
                let (current, default, suggested) = match role {
                    FontRole::Ui => (theme::ui_font(), DEFAULT_UI_FONT, &SUGGESTED_UI_FONTS[..]),
                    FontRole::Code => (theme::mono(), DEFAULT_CODE_FONT, &SUGGESTED_CODE_FONTS[..]),
                };
                let all = theme::font_families(cx);
                let suggested: Vec<String> = suggested
                    .iter()
                    .filter(|f| **f == default || all.iter().any(|a| a == *f))
                    .map(|f| f.to_string())
                    .collect();
                let rest = all.into_iter().filter(|f| !suggested.contains(f));
                let row = |n: usize, name: String, cx: &mut Context<Self>| {
                    let label = if name == default { format!("{name} (built in)") } else { name.clone() };
                    // Suggested fonts are shown in themselves; the long list stays plain.
                    menu_item(("font", n), label, *current == *name, cx)
                        .when(n < suggested.len(), |d| d.font_family(name.clone()))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let chosen = if name == default { String::new() } else { name.clone() };
                            match role {
                                FontRole::Ui => this.saved.ui_font = chosen,
                                FontRole::Code => this.saved.code_font = chosen,
                            }
                            theme::set_fonts(&this.saved.ui_font, &this.saved.code_font);
                            this.menu = None;
                            this.save();
                            cx.refresh_windows();
                        }))
                        .into_any_element()
                };
                for (n, name) in suggested.iter().enumerate() {
                    items.push(row(n, name.clone(), cx));
                }
                items.push(div().my_1().child(divider(cx)).into_any_element());
                for (n, name) in rest.enumerate() {
                    items.push(row(suggested.len() + n, name, cx));
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
                let path = self
                    .settings
                    .as_ref()
                    .map(|s| s.path.clone())
                    .or_else(|| self.active_project().map(|p| p.path.clone()));
                let current = match (&self.settings, &path) {
                    (Some(s), _) => s.claude.effort.clone(),
                    (None, Some(p)) => self.claude_settings(p).effort,
                    _ => String::new(),
                };
                // Only the levels the selected model takes, once the CLI has said which.
                let allowed = models::current_model(&self.active_model()).map(|m| m.efforts);
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
                    ("Open in an external terminal", Opener::Terminal),
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
                items.extend(self.tab_colour_items(&path, cx));
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
            Menu::Session(id) => items = self.session_menu_items(id, cx),
            Menu::Group(i) => items = self.group_menu_items(*i, cx),
        }
        Some(
            deferred(
                anchored().position(at).anchor(anchor).snap_to_window_with_margin(px(8.)).child(pop(
                    SharedString::from(format!("menu-in-{which:?}")),
                    popover(cx)
                        .min_w(px(if which == Menu::Theme { 300. } else { 200. }))
                        // Clicks stop here: the row under "Rename" mustn't take them
                        // (and the focus) too.
                        .occlude()
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                            this.menu = None;
                            cx.notify();
                        }))
                        .map(|d| {
                            if scroll {
                                let scroll = scrollbar::handle("menu-scroll");
                                let area = div()
                                    .id("menu-scroll")
                                    .max_h(px(420.))
                                    .overflow_y_scroll()
                                    .track_scroll(&scroll)
                                    .children(items);
                                d.w(px(280.)).child(scrollbar::vertical_over("menu-bar", scroll, area, |d| d))
                            } else {
                                d.children(items)
                            }
                        }),
                    // Drops down into place.
                    px(-4.),
                    leaving,
                )),
            )
            .into_any_element(),
        )
    }
}
