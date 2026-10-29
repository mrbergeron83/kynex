//! The app's own Settings modal (Ctrl+, / Cmd+,), apart from a project's Claude
//! settings: the `claude` CLI and terminal shell, theme, fonts and animations,
//! and the shortcuts file. Edits save to `state.json` at once.

use std::path::PathBuf;

use gpui::{Anchor, ClickEvent, Context, PathPromptOptions, Window, div, prelude::*, px};
use kynex_core::cli;
use kynex_core::column::ColumnWidth;
use kynex_core::motion::Motion;
use kynex_core::open::Opener;
use kynex_core::paths::data_dir;
use kynex_core::terminal::ShellKind;

use kynex_core::toasts::{Corner, Toast};

use super::KynexApp;
use super::menus::{FontRole, Menu};
use super::settings::{heading, radio, settings_modal, settings_nav};
use crate::theme::{self, Palette, THEMES, Theme, mono, theme};
use crate::ui::{Icon, icon, outline_button};

#[derive(Clone)]
pub struct AppSettingsView {
    section: usize,
    /// The `claude` in use and what it says its version is, once asked.
    cli: Option<CliInfo>,
}

/// Found off the UI thread when the modal opens, so no frame searches PATH.
#[derive(Clone)]
struct CliInfo {
    program: PathBuf,
    /// The picked `claude` is gone, so the one found is used.
    missing: bool,
    version: Result<String, String>,
}

const SECTIONS: [&str; 3] = ["General", "Appearance", "Keyboard"];

impl KynexApp {
    pub(super) fn open_app_settings(&mut self, cx: &mut Context<Self>) {
        if self.app_settings.take().is_some() {
            return cx.notify();
        }
        self.settings = None;
        self.extensions = None;
        self.app_settings = Some(AppSettingsView { section: 0, cli: None });
        self.read_cli_version(cx);
        cx.notify();
    }

    /// Finds the `claude` in use and asks it for its version, off the UI thread.
    fn read_cli_version(&mut self, cx: &mut Context<Self>) {
        let picked = self.saved.cli_path.clone();
        cx.spawn(async move |this, cx| {
            let info = cx
                .background_spawn(async move {
                    let program = cli::program();
                    let missing = picked.is_some_and(|p| !p.is_file());
                    let version = cli::version_of(&program);
                    CliInfo { program, missing, version }
                })
                .await;
            this.update(cx, |this, cx| {
                if let Some(s) = &mut this.app_settings {
                    s.cli = Some(info);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Runs `claude` from `path`, or with `None` the one found on its own.
    /// Sessions started from now on use it.
    fn set_cli_path(&mut self, path: Option<PathBuf>, cx: &mut Context<Self>) {
        cli::set_program(path.clone());
        self.saved.cli_path = path;
        self.save();
        if let Some(s) = &mut self.app_settings {
            s.cli = None;
        }
        self.read_cli_version(cx);
        self.check_setup(cx);
        cx.notify();
    }

    fn pick_cli(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Use this claude".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(mut paths))) = paths.await
                && let Some(path) = paths.pop()
            {
                this.update(cx, |this, cx| this.set_cli_path(Some(path), cx)).ok();
            }
        })
        .detach();
    }

    fn set_shell(&mut self, shell: Option<ShellKind>, cx: &mut Context<Self>) {
        ShellKind::set_default(shell);
        self.saved.shell = shell;
        self.save();
        cx.notify();
    }

    pub(super) fn set_theme(&mut self, th: Theme, cx: &mut Context<Self>) {
        self.saved.theme = th;
        self.save();
        theme::set(th, cx);
        cx.refresh_windows();
    }

    pub(super) fn set_motion(&mut self, m: Motion, cx: &mut Context<Self>) {
        self.saved.motion = m;
        self.save();
        crate::ui::motion::apply(m, cx);
        cx.notify();
    }

    fn open_keymap(&mut self, cx: &mut Context<Self>) {
        // Kynex writes it on first start; it may have been deleted since.
        let path = data_dir().join("keymap.json");
        if !path.exists()
            && let Err(e) = std::fs::write(&path, "{}\n")
        {
            self.toast(Toast::error(format!("Couldn't create {}: {e}", path.display())));
            return cx.notify();
        }
        cx.open_with_system(&path);
    }

    pub(super) fn render_app_settings(
        &mut self,
        leaving: Option<f32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let view = self.app_settings.as_ref()?;
        let t = theme(cx).clone();
        let section = view.section.min(SECTIONS.len() - 1);
        let nav = settings_nav(
            &SECTIONS,
            section,
            |this, i| {
                if let Some(s) = &mut this.app_settings {
                    s.section = i;
                }
            },
            cx,
        );
        let content = match section {
            0 => self.render_general(&t, cx),
            1 => self.render_appearance(&t, cx),
            _ => self.render_keyboard(&t, cx),
        };
        let footer = format!("Stored in {}", data_dir().join("state.json").display());
        let close = |this: &mut Self| this.app_settings = None;
        let (nav, content) = (nav.into_any_element(), content.into_any_element());
        Some(settings_modal(("Settings", None), nav, content, footer, close, window.viewport_size(), leaving, cx))
    }

    fn render_general(&self, t: &Palette, cx: &mut Context<Self>) -> gpui::Div {
        let view = self.app_settings.as_ref();
        let picked = self.saved.cli_path.clone();
        let info = view.and_then(|v| v.cli.as_ref());
        let program = info.map(|i| i.program.display().to_string()).unwrap_or_else(|| "Looking for claude…".into());
        let missing = info.is_some_and(|i| i.missing);
        let version = match info.map(|i| &i.version) {
            None => ("Checking…".to_string(), t.text_placeholder),
            Some(Ok(v)) => (format!("Claude Code {v}"), t.text_muted),
            Some(Err(e)) => (e.clone(), t.error),
        };
        let cli = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(heading(
                "Claude Code CLI",
                "Found on PATH, else in ~/.local/bin. Pick another for unusual installs; sessions started from now on use it.",
                t,
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .px_2()
                    .py_1p5()
                    .rounded_md()
                    .border_1()
                    .border_color(t.border_variant)
                    .bg(t.editor)
                    .child(div().font_family(mono()).text_sm().truncate().child(program))
                    .child(div().text_xs().text_color(version.1).child(version.0))
                    .when(missing, |d| {
                        let p = picked.clone().unwrap_or_default();
                        d.child(
                            div()
                                .text_xs()
                                .text_color(t.warning)
                                .child(format!("{} isn't there any more, so the one found is used.", p.display())),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(outline_button("pick-cli", "Choose…", cx).on_click(cx.listener(|this, _, _, cx| this.pick_cli(cx))))
                    .when(picked.is_some(), |d| {
                        d.child(
                            outline_button("auto-cli", "Find automatically", cx)
                                .on_click(cx.listener(|this, _, _, cx| this.set_cli_path(None, cx))),
                        )
                    }),
            );

        let current = self.saved.shell;
        let auto_label = format!("Automatic ({})", ShellKind::default_for_os().label());
        let shells = [
            (auto_label, None),
            (ShellKind::PowerShell.label().to_string(), Some(ShellKind::PowerShell)),
            (ShellKind::Bash.label().to_string(), Some(ShellKind::Bash)),
        ];
        let shell = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(heading(
                "Terminal shell",
                "What a new terminal opens. The + menu in the terminal panel opens either.",
                t,
            ))
            .children(shells.into_iter().enumerate().map(|(n, (label, kind))| {
                choice(("shell", n), current == kind, label, t)
                    .on_click(cx.listener(move |this, _, _, cx| this.set_shell(kind, cx)))
            }));

        let past = self.saved.name_past_sessions;
        let recent = format!("Also the {} most recent unnamed ones", super::titles::NAME_RECENT);
        let naming = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(heading(
                "Session names",
                "Claude Haiku names sessions from their first message, one short call each on your plan.",
                t,
            ))
            .children([(false, "Only the session you open".to_string()), (true, recent)].into_iter().map(
                |(on, label)| {
                    choice(("name-past", usize::from(on)), past == on, label, t)
                        .on_click(cx.listener(move |this, _, _, cx| this.set_name_past_sessions(on, cx)))
                },
            ));

        let notify = self.saved.notify;
        let notifications = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(heading(
                "Notifications",
                "While Kynex is in the background. Its icon shows a dot whenever a session needs you.",
                t,
            ))
            .children([(false, "Off"), (true, "Notify when a session finishes or needs you")].into_iter().map(
                |(on, label)| {
                    choice(("notify", usize::from(on)), notify == on, label.into(), t).on_click(cx.listener(
                        move |this, _, _, cx| {
                            this.saved.notify = on;
                            this.save();
                            cx.notify();
                        },
                    ))
                },
            ));

        div().flex().flex_col().gap_6().child(cli).child(shell).child(naming).child(notifications)
    }

    fn set_name_past_sessions(&mut self, on: bool, cx: &mut Context<Self>) {
        self.saved.name_past_sessions = on;
        self.save();
        // Re-read the list so naming picks up (or stops after the current one).
        self.reread_sessions();
        cx.notify();
    }

    fn render_appearance(&self, t: &Palette, cx: &mut Context<Self>) -> gpui::Div {
        let themes =
            div().flex().flex_col().gap_1().child(heading("Theme", "Colours for the whole window.", t)).children(
                THEMES.iter().enumerate().map(|(n, (th, name))| {
                    let th = *th;
                    choice(("app-theme", n), self.saved.theme == th, name.to_string(), t)
                        .on_click(cx.listener(move |this, _, _, cx| this.set_theme(th, cx)))
                }),
            );

        let font_row = |role: FontRole, label: &'static str, current: String, cx: &mut Context<Self>| {
            div().flex().items_center().gap_3().child(div().w(px(110.)).text_sm().child(label)).child(
                outline_button(("app-font", role as usize), current, cx)
                    .gap_1()
                    .child(icon(Icon::ChevronDown, t.text_placeholder).size_3())
                    .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                        this.open_menu(Menu::Font(role), ev.position(), Anchor::TopLeft, cx)
                    })),
            )
        };
        let fonts = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(heading("Fonts", "The interface, and code, diffs and terminals.", t))
            .child(font_row(FontRole::Ui, "Interface", theme::ui_font().to_string(), cx))
            .child(font_row(FontRole::Code, "Code", theme::mono().to_string(), cx));

        let motion = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(heading("Animations", "Transitions and smooth scrolling.", t))
            .children(Motion::ALL.into_iter().enumerate().map(|(n, m)| {
                let label = match m {
                    Motion::System => "As the system (reduce motion turns them off)",
                    Motion::On => "On",
                    Motion::Off => "Off",
                };
                choice(("app-motion", n), self.saved.motion == m, label.to_string(), t)
                    .on_click(cx.listener(move |this, _, _, cx| this.set_motion(m, cx)))
            }));

        let column = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(heading("Transcript width", "How wide the conversation and the message box are.", t))
            .children(ColumnWidth::ALL.into_iter().enumerate().map(|(n, w)| {
                choice(("app-column", n), self.saved.column == w, format!("{} · {}", w.label(), w.help()), t)
                    .on_click(cx.listener(move |this, _, _, cx| this.set_column(w, cx)))
            }));

        let corner = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(heading("Notification position", "Where notes like a task ending or an update show.", t))
            .children(Corner::ALL.into_iter().enumerate().map(|(n, c)| {
                choice(("app-toast-corner", n), self.saved.toast_corner == c, c.label().to_string(), t).on_click(
                    cx.listener(move |this, _, _, cx| {
                        this.saved.toast_corner = c;
                        this.save();
                        cx.notify();
                    }),
                )
            }));

        div().flex().flex_col().gap_6().child(themes).child(fonts).child(column).child(corner).child(motion)
    }

    fn set_column(&mut self, w: ColumnWidth, cx: &mut Context<Self>) {
        self.saved.column = w;
        self.save();
        // Row heights change with their width.
        for th in self.threads.values() {
            th.list.remeasure();
        }
        cx.notify();
    }

    fn render_keyboard(&self, t: &Palette, cx: &mut Context<Self>) -> gpui::Div {
        let path = data_dir().join("keymap.json");
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(heading(
                "Shortcuts",
                "Rebind Kynex's shortcuts in keymap.json by name, e.g. { \"interrupt\": \"ctrl-c\" }; an empty string unbinds one. Changes apply from the next start.",
                t,
            ))
            .child(div().font_family(mono()).text_sm().text_color(t.text_muted).child(path.display().to_string()))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        outline_button("open-keymap", "Open keymap.json", cx)
                            .on_click(cx.listener(|this, _, _, cx| this.open_keymap(cx))),
                    )
                    .child(outline_button("show-keymap", "Show in folder", cx).on_click(cx.listener(|this, _, _, _| {
                        this.open_in(Opener::Explorer, &data_dir())
                    }))),
            )
    }
}

/// A radio row: a label with a dot when it's the current choice.
fn choice(id: impl Into<gpui::ElementId>, selected: bool, label: String, t: &Palette) -> gpui::Stateful<gpui::Div> {
    let hover = t.hover;
    div()
        .id(id)
        .flex()
        .items_start()
        .gap_2()
        .px_2()
        .py_1()
        .rounded_md()
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .child(radio(selected, t))
        .child(div().text_sm().child(label))
}
