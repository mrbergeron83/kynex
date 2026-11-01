//! The Claude settings modal for a project: default mode, allow / deny rules,
//! extra directories and effort. Edits save at once and reach running sessions
//! live, except a rule or folder taken away, which waits for a restart.

use std::path::{Path, PathBuf};

use crate::ui::px;
use gpui::{
    Anchor, AnyElement, ClickEvent, Context, FontWeight, MouseButton, PathPromptOptions, SharedString, Window, div,
    prelude::*,
};
use kynex_core::paths::project_name;
use kynex_core::project::{self, ClaudeSettings, Mode, Permissions};

use kynex_core::toasts::Toast;

use super::KynexApp;
use super::menus::Menu;
use crate::theme::{Palette, mono, theme};
use crate::ui::motion::{self, fade, pop};
use crate::ui::{Icon, Tooltip, icon, icon_button, menu_item, outline_button, scrollbar};

#[derive(Clone)]
pub struct SettingsView {
    pub path: PathBuf,
    pub perms: Permissions,
    pub claude: ClaudeSettings,
    /// The rules in the user's, the repo's and its local settings files, read
    /// when the modal opened, and what couldn't be read.
    pub files: Vec<kynex_core::rules::Rule>,
    pub file_errors: Vec<String>,
    /// Starter rules for what's in the folder, found when the modal opened.
    pub starters: Vec<kynex_core::starters::Starter>,
    section: usize,
}

/// The Setup section, where a project's settings open the first time.
pub(super) const SETUP_SECTION: usize = 0;
/// Permissions, where they open otherwise.
const PERMISSIONS_SECTION: usize = 1;

impl KynexApp {
    pub(super) fn open_settings(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.open_project_settings_on(path, PERMISSIONS_SECTION, cx);
    }

    /// Opens a project's Claude settings on `section`. Blocks briefly, reading
    /// the settings files and the folder's top level.
    pub(super) fn open_project_settings_on(&mut self, path: PathBuf, section: usize, cx: &mut Context<Self>) {
        self.app_settings = None;
        self.extensions = None;
        let (files, file_errors) = kynex_core::rules::load_files(&path);
        self.settings = Some(SettingsView {
            perms: Permissions::load(&path),
            claude: self.claude_settings(&path),
            files,
            file_errors,
            starters: kynex_core::starters::detect(&path),
            path,
            section,
        });
        cx.notify();
    }

    /// Applies an edit in the settings modal. Edits save immediately and reach
    /// the project's running sessions ([`Self::apply_project_settings`]).
    pub(super) fn edit_settings(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut SettingsView)) {
        let Some(view) = &mut self.settings else { return };
        let (perms_before, claude_before) = (view.perms.clone(), view.claude.clone());
        f(view);
        let path = view.path.clone();
        let mut errors = Vec::new();
        if view.perms != perms_before
            && let Err(e) = view.perms.save(&path)
        {
            errors.push(e);
        }
        if view.claude != claude_before {
            if let Err(e) = view.claude.save(&path) {
                errors.push(e);
            }
            self.claude_settings.insert(path.clone(), view.claude.clone());
            if view.claude.mode != claude_before.mode {
                for s in self.code.get_mut(&path).into_iter().flat_map(|l| l.sessions.iter_mut()) {
                    s.set_mode(view.claude.mode);
                }
            }
        }
        if view.perms != perms_before
            || view.claude.effort != claude_before.effort
            || view.claude.add_dirs != claude_before.add_dirs
        {
            let (perms, claude) = (view.perms.clone(), view.claude.clone());
            self.apply_project_settings(&path, &perms, &claude);
        }
        if let Some(e) = errors.pop() {
            self.toast(Toast::error(e));
        }
        cx.notify();
    }

    /// A project's Claude settings, from the cache; read from disk only for a
    /// project that isn't open.
    pub(super) fn claude_settings(&self, path: &Path) -> ClaudeSettings {
        self.claude_settings.get(path).cloned().unwrap_or_else(|| ClaudeSettings::load(path))
    }

    /// The model the shown session runs on: its own, if one is pinned to it,
    /// else its folder's (the project's, or chat's).
    pub(super) fn active_model(&self) -> String {
        self.session_model().unwrap_or_else(|| self.folder_model())
    }

    /// The model pinned to the shown session, if any.
    pub(super) fn session_model(&self) -> Option<String> {
        self.active_session().and_then(|s| s.own_model.clone())
    }

    /// The model the active folder's sessions start on unless pinned
    /// otherwise, on the shown session's provider.
    pub(super) fn folder_model(&self) -> String {
        self.folder_model_on(&self.active_dir(), self.active_provider())
    }

    /// The shown session's model, named as the composer shows it.
    pub(super) fn active_model_name(&self) -> String {
        let m = self.active_model();
        let m = if m.is_empty() { self.folder_model() } else { m };
        match self.active_provider() {
            kynex_core::provider::Provider::OpenRouter => super::models::openrouter_label(&m),
            kynex_core::provider::Provider::Anthropic => super::models::model_label(&m),
        }
    }

    /// Who answers the shown session, or the active folder's new ones.
    pub(super) fn active_provider(&self) -> kynex_core::provider::Provider {
        self.active_session().map_or_else(|| self.folder_provider(&self.active_dir()), |s| s.provider().provider())
    }

    /// The model dir's sessions start on: its own, or Haiku for the chief.
    pub(super) fn dir_model(&self, dir: &Path) -> String {
        let model = self.claude_settings(dir).model;
        if model.is_empty() && dir == kynex_core::paths::chief_dir() {
            return kynex_core::chief::DEFAULT_MODEL.into();
        }
        model
    }

    /// Picks a model where it applies now: the shown session's own, if it has
    /// one, else its folder's. A session starts on it from its next start.
    pub(super) fn set_model(&mut self, id: String, cx: &mut Context<Self>) {
        if self.session_model().is_some() {
            self.set_session_model(Some(id), cx);
        } else if self.active_provider() == kynex_core::provider::Provider::OpenRouter {
            self.set_project_openrouter_model(self.active_dir(), id, cx);
        } else {
            self.set_project_model(self.active_dir(), id, cx);
        }
        cx.notify();
    }

    /// Pins a model to the shown session (`Some`), or has it follow its folder's
    /// (`None`). Saved with the folder's sessions once the session has an id.
    pub(super) fn set_session_model(&mut self, model: Option<String>, cx: &mut Context<Self>) {
        let folder = self.folder_model();
        let Some(s) = self.session_mut() else { return };
        s.own_model = model.clone();
        s.use_model(model.clone().unwrap_or(folder));
        let (cwd, id) = (s.cwd.clone(), s.session_id.clone());
        if let Some(id) = id {
            self.edit_groups(&cwd, |g| g.set_model(&id, model.as_deref()));
        }
        cx.notify();
    }

    /// Sets a folder's model, from the modal or the composer's picker. Its
    /// sessions without a pin start on it from their next start.
    pub(super) fn set_project_model(&mut self, path: PathBuf, model: String, cx: &mut Context<Self>) {
        if self.settings.as_ref().is_some_and(|s| s.path == path) {
            return self.edit_settings(cx, |v| v.claude.model = model);
        }
        let mut claude = self.claude_settings(&path);
        if claude.model != model {
            claude.model = model.clone();
            if let Err(e) = claude.save(&path) {
                self.toast(Toast::error(e));
            }
            self.claude_settings.insert(path, claude);
        }
        if let Some(s) = self.session_mut().filter(|s| s.own_model.is_none()) {
            s.use_model(model);
        }
    }

    /// Sets the OpenRouter model a folder's new OpenRouter sessions start on;
    /// the shown session, if it follows its folder, from its next start.
    pub(super) fn set_project_openrouter_model(&mut self, path: PathBuf, model: String, cx: &mut Context<Self>) {
        let mut claude = self.claude_settings(&path);
        if claude.openrouter_model != model {
            claude.openrouter_model = model.clone();
            if let Err(e) = claude.save(&path) {
                self.toast(Toast::error(e));
            }
            self.claude_settings.insert(path, claude);
        }
        if let Some(s) = self.session_mut().filter(|s| s.own_model.is_none()) {
            s.use_model(model);
        }
        cx.notify();
    }

    /// Picks who answers the shown new session, and its folder's next ones.
    /// A session that has started keeps its provider.
    pub(super) fn set_provider(&mut self, provider: kynex_core::provider::Provider, cx: &mut Context<Self>) {
        let path = self.active_dir();
        let mut claude = self.claude_settings(&path);
        if claude.provider != provider {
            claude.provider = provider;
            if let Err(e) = claude.save(&path) {
                self.toast(Toast::error(e));
            }
            self.claude_settings.insert(path, claude);
        }
        // A pinned model belongs to the provider it was picked on.
        if let Some(s) = self.session_mut().filter(|s| s.is_new()) {
            s.own_model = None;
        }
        self.refresh_new_sessions();
        cx.notify();
    }

    /// Sets a project's effort, from the modal or the composer's picker.
    pub(super) fn set_effort(&mut self, path: PathBuf, id: String, cx: &mut Context<Self>) {
        if self.settings.is_some() {
            return self.edit_settings(cx, |v| v.claude.effort = id);
        }
        let mut claude = self.claude_settings(&path);
        if claude.effort != id {
            claude.effort = id;
            if let Err(e) = claude.save(&path) {
                self.toast(Toast::error(e));
            }
            self.claude_settings.insert(path.clone(), claude.clone());
            self.apply_project_settings(&path, &Permissions::load(&path), &claude);
        }
    }

    /// Brings a project's running sessions up to its changed rules, extra folders
    /// or effort: live where the CLI takes them, else each starts again once
    /// idle (`--resume` keeps the conversation; "this session" rules are dropped).
    fn apply_project_settings(&mut self, path: &Path, perms: &Permissions, claude: &ClaudeSettings) {
        for s in self.code.get_mut(path).into_iter().flat_map(|l| l.sessions.iter_mut()) {
            s.apply_settings(perms, claude);
        }
        self.restart_idle();
    }

    pub(super) fn add_rule(&mut self, deny: bool, cx: &mut Context<Self>) {
        let rule = self.new_rule.read(cx).text().trim().to_string();
        if rule.is_empty() {
            return;
        }
        self.edit_settings(cx, |v| {
            let list = if deny { &mut v.perms.deny } else { &mut v.perms.allow };
            if !list.contains(&rule) {
                list.push(rule);
            }
        });
        self.new_rule.update(cx, |i, cx| i.set_text("", cx));
    }

    fn add_dir(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Add folder".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = paths.await {
                this.update(cx, |this, cx| {
                    for dir in paths {
                        this.edit_settings(cx, |v| {
                            if !v.claude.add_dirs.contains(&dir) {
                                v.claude.add_dirs.push(dir);
                            }
                        });
                    }
                })
                .ok();
            }
        })
        .detach();
    }

    pub(super) fn render_settings(
        &mut self,
        leaving: Option<f32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let view = self.settings.as_ref()?;
        let t = theme(cx).clone();
        const SECTIONS: [&str; 5] = ["Setup", "Permissions", "Rules in effect", "Model", "Appearance"];
        let section = view.section.min(SECTIONS.len() - 1);
        let path = view.path.clone();
        let vp = window.viewport_size();

        let nav = settings_nav(
            &SECTIONS,
            section,
            |this, i| {
                if let Some(s) = &mut this.settings {
                    s.section = i;
                }
            },
            cx,
        );

        let content = match section {
            SETUP_SECTION => self.render_project_setup(view, &t, cx),
            PERMISSIONS_SECTION => {
                let modes = Mode::ALL.into_iter().enumerate().map(|(n, m)| {
                    let selected = view.claude.mode == m;
                    let hover = t.hover;
                    div()
                        .id(("default-mode", n))
                        .flex()
                        .items_start()
                        .gap_2()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .cursor_pointer()
                        .hover(move |s| s.bg(hover))
                        .child(radio(selected, &t))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .child(div().text_sm().child(m.label()))
                                .child(div().text_xs().text_color(t.text_muted).child(m.help())),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| this.edit_settings(cx, |v| v.claude.mode = m)))
                });
                let rule_list = |deny: bool, rules: &[String], cx: &mut Context<Self>| {
                    let title = if deny { "Deny" } else { "Allow" };
                    div()
                        .flex()
                        .flex_col()
                        .gap_0p5()
                        .child(div().text_xs().text_color(t.text_muted).child(title))
                        .when(rules.is_empty(), |d| {
                            d.child(div().pl_2().text_sm().text_color(t.text_placeholder).child("None"))
                        })
                        .children(rules.iter().enumerate().map(|(i, rule)| {
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .pl_2()
                                .rounded_md()
                                .hover(|s| s.bg(t.hover))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .font_family(mono())
                                        .text_sm()
                                        .child(rule.clone()),
                                )
                                .child(
                                    icon_button(SharedString::from(format!("rm-{title}-{i}")), Icon::Close, cx)
                                        .tooltip(Tooltip::text("Remove this rule"))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.edit_settings(cx, |v| {
                                                let list = if deny { &mut v.perms.deny } else { &mut v.perms.allow };
                                                if i < list.len() {
                                                    list.remove(i);
                                                }
                                            })
                                        })),
                                )
                        }))
                };
                let (allow, deny, dirs) =
                    (view.perms.allow.clone(), view.perms.deny.clone(), view.claude.add_dirs.clone());
                div()
                    .flex()
                    .flex_col()
                    .gap_5()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(heading("Default mode", "What new sessions start in. Switch a running session from the composer.", &t))
                            .children(modes),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(heading(
                                "Rules",
                                "In Claude Code's syntax: Bash(cargo test:*), Edit, WebFetch(domain:docs.rs). They add to your own Claude settings; a deny anywhere wins.",
                                &t,
                            ))
                            .child(rule_list(false, &allow, cx))
                            .child(rule_list(true, &deny, cx))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .flex_1()
                                            .px_2()
                                            .py_1()
                                            .rounded_md()
                                            .border_1()
                                            .border_color(t.border)
                                            .bg(t.editor)
                                            .text_sm()
                                            .child(self.new_rule.clone()),
                                    )
                                    .child(outline_button("add-allow", "Allow", cx).on_click(cx.listener(|this, _, _, cx| this.add_rule(false, cx))))
                                    .child(outline_button("add-deny", "Deny", cx).on_click(cx.listener(|this, _, _, cx| this.add_rule(true, cx)))),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(heading("Additional directories", "Folders outside the project Claude may read and edit (--add-dir).", &t))
                            .children(dirs.iter().enumerate().map(|(i, d)| {
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .pl_2()
                                    .rounded_md()
                                    .hover(|s| s.bg(t.hover))
                                    .child(div().flex_1().min_w_0().truncate().font_family(mono()).text_sm().child(d.display().to_string()))
                                    .child(
                                        icon_button(("rm-dir", i), Icon::Close, cx).tooltip(Tooltip::text("Remove")).on_click(cx.listener(
                                            move |this, _, _, cx| {
                                                this.edit_settings(cx, |v| {
                                                    if i < v.claude.add_dirs.len() {
                                                        v.claude.add_dirs.remove(i);
                                                    }
                                                })
                                            },
                                        )),
                                    )
                            }))
                            .child(
                                div().pt_1().child(
                                    outline_button("add-dir", "Add folder…", cx).on_click(cx.listener(|this, _, _, cx| this.add_dir(cx))),
                                ),
                            ),
                    )
            }
            2 => self.render_rules_in_effect(view, &t, cx),
            3 => {
                let effort = project::EFFORTS.iter().find(|e| e.0 == view.claude.effort).map_or("Default", |e| e.1);
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(heading(
                        "Effort",
                        "How hard Claude thinks (--effort). Default uses your Claude settings.",
                        &t,
                    ))
                    .child(
                        div().flex().child(
                            outline_button("effort", effort, cx)
                                .gap_1()
                                .child(icon(Icon::ChevronDown, t.text_placeholder).size_3())
                                .on_click(cx.listener(|this, ev: &ClickEvent, _, cx| {
                                    this.open_menu(Menu::Effort, ev.position(), Anchor::TopLeft, cx)
                                })),
                        ),
                    )
                    .child(
                        div()
                            .pt_2()
                            .text_xs()
                            .text_color(t.text_muted)
                            .child(
                                "The model and effort are also in the composer, where a session can pin a model of its own over the project's.",
                            ),
                    )
                    .child(div().pt_3().child(heading(
                        "KNX Ops workers",
                        "This project's worker runs the economy flow: a Haiku coordinator, Opus planning and reviewing larger tasks, Haiku executing. It starts on a worker's next start.",
                        &t,
                    )))
                    .child({
                        let on = !view.claude.economy_off;
                        let hover = t.hover;
                        div()
                            .id("economy")
                            .flex()
                            .items_start()
                            .gap_2()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .cursor_pointer()
                            .hover(move |s| s.bg(hover))
                            .child(radio(on, &t))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .child(div().text_sm().child("Opus plans, Haiku executes"))
                                    .child(
                                        div().text_xs().text_color(t.text_muted).child(
                                            "Cheaper and faster on bigger tasks, and the reporter shows what Opus took. Anthropic only.",
                                        ),
                                    ),
                            )
                            .on_click(
                                cx.listener(|this, _, _, cx| this.edit_settings(cx, |v| v.claude.economy_off = !v.claude.economy_off)),
                            )
                    })
            }
            _ => self.render_colour_picker(&path, &t, cx),
        };

        let footer = format!("Stored in {}", project::dir(&path).display());
        let close = |this: &mut Self| this.settings = None;
        let (nav, content) = (nav.into_any_element(), content.into_any_element());
        let title = ("Claude settings", Some(project_name(&path)));
        Some(settings_modal(title, nav, content, footer, close, vp, leaving, cx))
    }
}

/// The frame both settings modals share: a title bar with a close button,
/// sections down the side, the section's content and where it's stored.
/// Clicking outside or the close button runs `close`; `leaving` while it plays out.
#[allow(clippy::too_many_arguments)]
pub(super) fn settings_modal(
    (title, subtitle): (&'static str, Option<String>),
    nav: gpui::AnyElement,
    content: gpui::AnyElement,
    footer: String,
    close: fn(&mut KynexApp),
    vp: gpui::Size<gpui::Pixels>,
    leaving: Option<f32>,
    cx: &mut Context<KynexApp>,
) -> AnyElement {
    let t = theme(cx).clone();
    let card = div()
        .id("settings-card")
        .occlude()
        .flex()
        .flex_col()
        .w(gpui::px(760.).min(vp.width - gpui::px(80.)))
        .h(gpui::px(560.).min(vp.height - gpui::px(100.)))
        .rounded_lg()
        .border_1()
        .border_color(t.border)
        .bg(t.elevated)
        .shadow_lg()
        .overflow_hidden()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap_2()
                .h(px(44.))
                .pl_4()
                .pr_2()
                .border_b_1()
                .border_color(t.border_variant)
                .child(icon(Icon::Settings, t.text_muted))
                .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(title))
                .children(subtitle.map(|s| div().text_sm().text_color(t.text_muted).child(s)))
                .child(div().flex_1())
                .child(
                    icon_button("close-settings", Icon::Close, cx)
                        .tooltip(Tooltip::with_meta("Close", "Esc"))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            close(this);
                            cx.notify();
                        })),
                ),
        )
        .child(div().flex().flex_1().min_h_0().child(nav).child({
            let scroll = scrollbar::handle("settings-body");
            let body = div()
                .id("settings-body")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .track_scroll(&scroll)
                .p_5()
                .child(content)
                .child(div().pt_6().text_xs().text_color(t.text_placeholder).child(footer));
            scrollbar::vertical_over("settings-bar", scroll, body, |d| d.flex_1().min_w_0())
        }));

    fade(
        "settings-fade",
        div()
            .id("modal-backdrop")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(crate::theme::theme(cx).scrim())
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    close(this);
                    cx.notify();
                }),
            )
            .child(pop("settings-in", card, gpui::px(8.), leaving)),
        motion::QUICK,
        leaving,
    )
}

/// The sections down the side of a settings modal; clicking one runs `pick`.
pub(super) fn settings_nav(
    sections: &[&'static str],
    current: usize,
    pick: fn(&mut KynexApp, usize),
    cx: &mut Context<KynexApp>,
) -> impl IntoElement + use<> {
    let t = theme(cx).clone();
    div()
        .flex()
        .flex_col()
        .gap_0p5()
        .w(px(170.))
        .flex_none()
        .p_2()
        .border_r_1()
        .border_color(t.border_variant)
        .children(sections.iter().enumerate().map(|(i, name)| {
            menu_item(("settings-section", i), *name, false, cx).when(i == current, |d| d.bg(t.active)).on_click(
                cx.listener(move |this, _, _, cx| {
                    pick(this, i);
                    cx.notify();
                }),
            )
        }))
}

pub(super) fn heading(title: &'static str, help: &'static str, t: &Palette) -> impl IntoElement + use<> {
    div()
        .flex()
        .flex_col()
        .gap_0p5()
        .pb_1()
        .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(title))
        .child(div().text_xs().text_color(t.text_muted).child(help))
}

pub(super) fn radio(selected: bool, t: &Palette) -> impl IntoElement + use<> {
    div()
        .mt_0p5()
        .size(px(14.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .border_1()
        .border_color(if selected { t.accent } else { t.border })
        .when(selected, |d| d.child(div().size(px(6.)).rounded_full().bg(t.accent)))
}
