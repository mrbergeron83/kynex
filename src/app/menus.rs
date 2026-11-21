//! Popup menus: Kynex menu, appearance (theme and fonts), project tab, sessions,
//! workspaces, pane, Agents pane rows, and keyboard pickers.

use std::path::PathBuf;

use crate::ui::px;
use gpui::{Anchor, AnyElement, Context, Pixels, Point, SharedString, anchored, deferred, div, prelude::*};
use kynex_core::layout::PaneId;
use kynex_core::motion::Motion;
use kynex_core::open::Opener;
use kynex_core::update;

use super::KynexApp;
use crate::theme::{self, DEFAULT_CODE_FONT, DEFAULT_UI_FONT, SUGGESTED_CODE_FONTS, SUGGESTED_UI_FONTS, THEMES, theme};
use crate::ui::motion::pop;
use crate::ui::{Icon, divider, icon, menu_item, popover, scrollbar};

#[derive(Clone, Debug, PartialEq)]
pub enum Menu {
    Kynex,
    Theme,
    Font(FontRole),
    Tab(usize),
    /// A past session's right-click menu, by its id.
    Session(String),
    /// A session group's right-click menu: this group of the workspace at this folder.
    Group(PathBuf, kynex_core::groups::GroupId),
    /// The right-click menu for several picked sessions.
    Sessions(Vec<String>),
    /// The sessions panel header's menu.
    SessionsPanel,
    /// The sessions panel header's "Add workspace".
    AddWorkspace,
    /// A workspace's right-click menu, by its folder.
    Workspace(PathBuf),
    /// A pane's right-click menu.
    Pane(PaneId),
    /// A row of the Agents pane's right-click menu.
    Running(PaneId),
    /// The workspace picker (`prefix w`).
    Workspaces,
    /// The older layouts of this workspace (`prefix s`).
    Layouts,
    /// The project picker (`prefix shift-p`).
    Projects,
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
        self.layout_name = None;
        self.menu = if self.menu.as_ref().is_some_and(|m| m.which == which) {
            None
        } else {
            Some(OpenMenu { which, at, anchor })
        };
        cx.notify();
    }

    /// Which picker (workspaces, projects, layouts) is open, for its keys.
    pub(super) fn open_picker_menu(&self) -> Option<Menu> {
        self.menu
            .as_ref()
            .map(|m| m.which.clone())
            .filter(|m| matches!(m, Menu::Workspaces | Menu::Projects | Menu::Layouts))
    }

    /// The open menu; `leaving` while it plays out after closing.
    pub(super) fn render_menu(&mut self, leaving: Option<f32>, cx: &mut Context<Self>) -> Option<AnyElement> {
        let open = self.menu.as_ref()?;
        let (which, at, anchor) = (open.which.clone(), open.at, open.anchor);
        let t = theme(cx).clone();
        let mut items: Vec<AnyElement> = Vec::new();
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
                let custom =
                    self.saved.custom_theme.as_ref().and_then(|id| self.custom_themes.iter().find(|c| &c.id == id));
                let current: String = match custom {
                    Some(c) => c.name.clone(),
                    None => THEMES.iter().find(|x| x.0 == self.saved.theme).map_or("", |x| x.1).to_string(),
                };
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
                let cli_update = self.cli_updates.any_available();
                items.push(
                    menu_item(
                        "kynex-check-cli",
                        if cli_update { "CLI updates available" } else { "Check for CLI updates" },
                        false,
                        cx,
                    )
                    .when(cli_update, |d| d.text_color(t.accent))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.menu = None;
                        this.open_about(cx);
                        if !cli_update {
                            this.check_all_clis_now(cx);
                        }
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
            Menu::Theme => {
                for (n, (th, name)) in THEMES.iter().enumerate() {
                    let th = *th;
                    items.push(
                        menu_item(("theme", n), *name, self.saved.custom_theme.is_none() && self.saved.theme == th, cx)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.menu = None;
                                this.set_theme(th, cx);
                            }))
                            .into_any_element(),
                    );
                }
                for (n, c) in self.custom_themes.iter().enumerate() {
                    let id = c.id.clone();
                    items.push(
                        menu_item(
                            ("custom-theme", n),
                            c.name.clone(),
                            self.saved.custom_theme.as_deref() == Some(&c.id),
                            cx,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.menu = None;
                            this.set_custom_theme(id.clone(), cx);
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
                            this.close_project(i, cx);
                            cx.notify();
                        }))
                        .into_any_element(),
                );
            }
            Menu::Session(id) => items = self.session_menu_items(id, cx),
            Menu::Group(cwd, i) => items = self.group_menu_items(&cwd.clone(), *i, cx),
            Menu::Sessions(ids) => items = self.sessions_menu_items(ids, cx),
            Menu::SessionsPanel => items = self.sessions_panel_menu_items(cx),
            Menu::AddWorkspace => items = self.add_workspace_menu_items(cx),
            Menu::Pane(pane) => items = self.pane_menu_items(*pane, cx),
            Menu::Running(pane) => items = self.running_menu_items(*pane, cx),
            Menu::Workspaces => {
                let list = match self.workspace_project() {
                    Some(project) => self.workspaces_of(&project, cx),
                    None => Vec::new(),
                };
                for (n, (c, _)) in list.into_iter().enumerate() {
                    let on = n == self.menu_pick;
                    let path = c.path.clone();
                    items.push(
                        menu_item(("workspace-pick", n), c.label.clone(), on, cx)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.menu = None;
                                this.select_workspace(&path, cx);
                            }))
                            .into_any_element(),
                    );
                }
            }
            Menu::Layouts => {
                let entries = self.layout_entries();
                if entries.is_empty() {
                    items.push(menu_item("layout-none", "No older layouts yet", false, cx).into_any_element());
                }
                for (n, s) in entries.into_iter().enumerate() {
                    let label = format!("{} · {}", kynex_core::fmt::ago_unix(s.at as i64), s.summary());
                    items.push(
                        menu_item(("layout-pick", n), label, n == self.menu_pick, cx)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.menu = None;
                                this.restore_layout(&s, window, cx);
                            }))
                            .into_any_element(),
                    );
                }
                items.extend(self.named_layout_items(cx));
            }
            Menu::Projects => {
                for (i, p) in self.saved.projects.iter().enumerate() {
                    let on = i == self.menu_pick;
                    items.push(
                        menu_item(("project-pick", i), kynex_core::paths::project_name(&p.path), on, cx)
                            .on_click(cx.listener(move |this, _, window, cx| this.jump_to_project(i, window, cx)))
                            .into_any_element(),
                    );
                }
                items.push(div().my_1().child(divider(cx)).into_any_element());
                items.push(
                    menu_item("project-open", "Open project…", self.menu_pick == self.saved.projects.len(), cx)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.menu = None;
                            this.pick_project(cx);
                        }))
                        .into_any_element(),
                );
            }
            Menu::Workspace(path) => {
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
                        menu_item(("workspace-menu", n), label, false, cx)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.open_in(opener, &path);
                                this.menu = None;
                                cx.notify();
                            }))
                            .into_any_element(),
                    );
                }
                items.push(div().my_1().child(divider(cx)).into_any_element());
                items.extend(self.tidy_menu_items(&path.clone(), cx));
                if self.active_project().is_some_and(|p| !kynex_core::workspace::same_folder(&p.path, path)) {
                    let folder = self
                        .active_project()
                        .and_then(|p| self.workspaces.get(&p.path))
                        .is_some_and(|c| c.is_folder(path));
                    let path = path.clone();
                    let tab_path = path.clone();
                    items.push(
                        menu_item("workspace-new-tab", "Open in a new tab", false, cx)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.menu = None;
                                this.open_workspace_tab(tab_path.clone(), cx);
                            }))
                            .into_any_element(),
                    );
                    items.push(div().my_1().child(divider(cx)).into_any_element());
                    items.push(
                        menu_item(
                            "workspace-remove",
                            if folder { "Remove folder from workspaces" } else { "Remove workspace" },
                            false,
                            cx,
                        )
                        .text_color(t.error)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.menu = None;
                            this.remove_workspace(path.clone(), cx);
                            cx.notify();
                        }))
                        .into_any_element(),
                    );
                }
            }
        }
        Some(
            deferred(
                anchored().position(at).anchor(anchor).snap_to_window_with_margin(gpui::px(8.)).child(pop(
                    SharedString::from(format!("menu-in-{which:?}")),
                    popover(cx)
                        .min_w(px(match which {
                            Menu::Theme => 300.,
                            _ => 200.,
                        }))
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
                    gpui::px(-4.),
                    leaving,
                )),
            )
            .into_any_element(),
        )
    }
}
