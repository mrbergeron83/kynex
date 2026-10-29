//! The Claude settings modal for a project: default mode, allow / deny rules,
//! extra directories and effort. Edits save at once; the mode applies live,
//! the rest from the project's next process start.

use std::path::{Path, PathBuf};

use gpui::{
    Anchor, AnyElement, ClickEvent, Context, FontWeight, MouseButton, PathPromptOptions, SharedString, Window, div,
    prelude::*, px,
};
use kynex_core::paths::project_name;
use kynex_core::project::{self, ClaudeSettings, Mode, Permissions};

use kynex_core::toasts::Toast;

use super::KynexApp;
use super::menus::Menu;
use crate::theme::{Palette, mono, theme};
use crate::ui::motion::{self, fade, pop};
use crate::ui::{Icon, Tooltip, icon, icon_button, menu_item, outline_button};

#[derive(Clone)]
pub struct SettingsView {
    pub path: PathBuf,
    pub perms: Permissions,
    pub claude: ClaudeSettings,
    section: usize,
}

impl KynexApp {
    pub(super) fn open_settings(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.app_settings = None;
        self.extensions = None;
        self.settings = Some(SettingsView {
            perms: Permissions::load(&path),
            claude: self.claude_settings(&path),
            path,
            section: 0,
        });
        cx.notify();
    }

    /// Applies an edit in the settings modal. Edits save immediately: the mode
    /// applies to the live session, everything else from the next process start.
    fn edit_settings(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut SettingsView)) {
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
        let restart = view.perms != perms_before
            || view.claude.effort != claude_before.effort
            || view.claude.add_dirs != claude_before.add_dirs;
        if restart {
            // The CLI reads these at startup: restart the project's process so it picks them up.
            // `--resume` keeps the conversation going; rules allowed "this session" are dropped.
            self.restart_when_idle.insert(path);
            self.restart_idle();
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

    /// The model the active tab's new sessions start on: the project's own, if
    /// it has one, else the one for every tab.
    pub(super) fn active_model(&self) -> String {
        match self.active_project() {
            Some(p) => self.claude_settings(&p.path).model_or(&self.saved.model).to_string(),
            None => self.saved.model.clone(),
        }
    }

    /// The active project's own model, if it has one.
    pub(super) fn project_model(&self) -> Option<String> {
        self.active_project().and_then(|p| self.claude_settings(&p.path).model)
    }

    /// Picks a model where it applies now: the project's own, if it has one,
    /// else every tab's. Sessions start on it from their next start.
    pub(super) fn set_model(&mut self, id: String, cx: &mut Context<Self>) {
        match self.active_project().filter(|_| self.project_model().is_some()) {
            Some(p) => self.set_project_model(p.path.clone(), Some(id), cx),
            None => {
                self.saved.model = id;
                self.save();
            }
        }
        cx.notify();
    }

    /// Gives a project its own model (`Some`), or has it follow every tab's (`None`).
    pub(super) fn set_project_model(&mut self, path: PathBuf, model: Option<String>, cx: &mut Context<Self>) {
        if self.settings.as_ref().is_some_and(|s| s.path == path) {
            return self.edit_settings(cx, |v| v.claude.model = model);
        }
        let mut claude = self.claude_settings(&path);
        if claude.model != model {
            claude.model = model;
            if let Err(e) = claude.save(&path) {
                self.toast(Toast::error(e));
            }
            self.claude_settings.insert(path, claude);
        }
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
            self.claude_settings.insert(path.clone(), claude);
            // The CLI reads it at startup: restart once idle; `--resume` keeps the conversation.
            self.restart_when_idle.insert(path);
            self.restart_idle();
        }
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
        const SECTIONS: [&str; 3] = ["Permissions", "Model", "Appearance"];
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
            0 => {
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
            1 => {
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
                                "The model and effort are also in the composer. The model applies to every tab, unless its menu gives this project its own.",
                            ),
                    )
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
        .w(px(760.).min(vp.width - px(80.)))
        .h(px(560.).min(vp.height - px(100.)))
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
        .child(
            div().flex().flex_1().min_h_0().child(nav).child(
                div()
                    .id("settings-body")
                    .flex_1()
                    .min_w_0()
                    .overflow_y_scroll()
                    .p_5()
                    .child(content)
                    .child(div().pt_6().text_xs().text_color(t.text_placeholder).child(footer)),
            ),
        );

    fade(
        "settings-fade",
        div()
            .id("modal-backdrop")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::black().opacity(0.45))
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    close(this);
                    cx.notify();
                }),
            )
            .child(pop("settings-in", card, px(8.), leaving)),
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
