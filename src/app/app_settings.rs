//! The app's Settings modal (Ctrl+, / Cmd+,): General (CLI path, worktrees, shell,
//! background notifications, keep awake), Providers (`providers.rs`), Appearance,
//! Keyboard, and Storage (`storage.rs`). Edits save to `state.json` immediately.

use std::path::PathBuf;

use crate::ui::px;
use gpui::{
    Anchor, AnyElement, ClickEvent, Context, Entity, FontWeight, MouseButton, PathPromptOptions, Size, Window, div,
    prelude::*,
};
use kynex_core::awake::KeepAwake;
use kynex_core::cli;
use kynex_core::motion::Motion;
use kynex_core::open::Opener;
use kynex_core::paths::data_dir;
use kynex_core::scale;
use kynex_core::terminal::ShellKind;
use kynex_core::tile_style::{self, TileTweaks};
use kynex_core::toasts::{Corner, Toast};

use super::KynexApp;
use super::menus::{FontRole, Menu};
use crate::theme::{self, Palette, THEMES, Theme, mono, theme};
use crate::ui::input::{InputEvent, TextInput};
use crate::ui::motion::{self, fade, pop};
use crate::ui::{Icon, Tooltip, icon, icon_button, outline_button, scrollbar};

#[derive(Clone)]
pub struct AppSettingsView {
    pub(super) section: usize,
    cli: Option<CliInfo>,
    pub(super) providers: super::providers::ProvidersView,
    shortcuts_filter: Entity<TextInput>,
}

#[derive(Clone)]
struct CliInfo {
    program: PathBuf,
    missing: bool,
    version: Result<String, String>,
}

const SECTIONS: [&str; 5] = ["General", "Providers", "Appearance", "Keyboard", "Storage"];
pub(super) const PROVIDERS_SECTION: usize = 1;
pub(super) const APPEARANCE_SECTION: usize = 2;
pub(super) const KEYBOARD_SECTION: usize = 3;
pub(super) const STORAGE_SECTION: usize = 4;

impl KynexApp {
    pub(super) fn open_app_settings(&mut self, cx: &mut Context<Self>) {
        if self.app_settings.take().is_some() {
            return cx.notify();
        }
        let providers = self.new_providers_view(cx);
        let shortcuts_filter = cx.new(|cx| TextInput::new(cx).placeholder("Search shortcuts"));
        cx.subscribe(&shortcuts_filter, |_, _, ev, cx| {
            if let InputEvent::Changed = ev {
                cx.notify();
            }
        })
        .detach();
        self.app_settings = Some(AppSettingsView { section: 0, cli: None, providers, shortcuts_filter });
        if let Some(s) = &mut self.storage {
            s.stale = true;
        }
        self.read_cli_version(cx);
        cx.notify();
    }

    pub(super) fn open_settings_on(&mut self, section: usize, cx: &mut Context<Self>) {
        if self.app_settings.is_none() {
            self.open_app_settings(cx);
        }
        if let Some(s) = &mut self.app_settings {
            s.section = section;
        }
        cx.notify();
    }

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

    fn set_cli_path(&mut self, path: Option<PathBuf>, cx: &mut Context<Self>) {
        cli::set_program(path.clone());
        self.saved.cli_path = path;
        self.save();
        if let Some(s) = &mut self.app_settings {
            s.cli = None;
        }
        self.read_cli_version(cx);
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

    fn set_worktree_root(&mut self, root: Option<PathBuf>, cx: &mut Context<Self>) {
        kynex_core::workspace::set_worktree_root(root.clone());
        self.saved.worktree_root = root;
        self.save();
        cx.notify();
    }

    fn pick_worktree_root(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Put worktrees here".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(mut paths))) = paths.await
                && let Some(path) = paths.pop()
            {
                this.update(cx, |this, cx| this.set_worktree_root(Some(path), cx)).ok();
            }
        })
        .detach();
    }

    fn set_terminal_history(&mut self, on: bool, cx: &mut Context<Self>) {
        self.saved.terminal_history = on;
        self.save();
        if !on {
            cx.background_spawn(async { kynex_core::scrollback::clear_all() }).detach();
        }
        cx.notify();
    }

    fn set_shell(&mut self, shell: Option<ShellKind>, cx: &mut Context<Self>) {
        ShellKind::set_default(shell);
        self.saved.shell = shell;
        self.save();
        cx.notify();
    }

    pub(super) fn set_theme(&mut self, th: Theme, cx: &mut Context<Self>) {
        self.saved.theme = th;
        self.saved.custom_theme = None;
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
    ) -> Option<AnyElement> {
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
            1 => self.render_providers(&t, cx),
            2 => self.render_appearance(&t, cx),
            3 => self.render_keyboard(&t, cx),
            _ => self.render_storage(&t, cx),
        };
        let footer = format!("Stored in {}", data_dir().join("state.json").display());
        let close = |this: &mut Self| this.app_settings = None;
        Some(settings_modal(
            "Settings",
            nav.into_any_element(),
            content.into_any_element(),
            footer,
            close,
            window.viewport_size(),
            leaving,
            cx,
        ))
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
                "Found on PATH, else in ~/.local/bin. Pick another for unusual installs; panes started from now on use it.",
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

        let root = self.saved.worktree_root.clone();
        let shown = root.clone().unwrap_or_else(|| data_dir().join("worktrees"));
        let worktrees = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(heading(
                "Worktrees folder",
                "New git worktrees go here, in a folder per project; existing ones stay where they are.",
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
                    .child(div().font_family(mono()).text_sm().truncate().child(shown.display().to_string()))
                    .when(root.is_none(), |d| d.child(div().text_xs().text_color(t.text_muted).child("Default"))),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        outline_button("pick-worktrees", "Choose…", cx)
                            .on_click(cx.listener(|this, _, _, cx| this.pick_worktree_root(cx))),
                    )
                    .when(root.is_some(), |d| {
                        d.child(
                            outline_button("default-worktrees", "Use default", cx)
                                .on_click(cx.listener(|this, _, _, cx| this.set_worktree_root(None, cx))),
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
                "What a new plain terminal pane opens. The + menu in a terminal pane opens either.",
                t,
            ))
            .children(shells.into_iter().enumerate().map(|(n, (label, kind))| {
                choice(("shell", n), current == kind, label, t)
                    .on_click(cx.listener(move |this, _, _, cx| this.set_shell(kind, cx)))
            }));

        let keep = self.saved.terminal_history;
        let history = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(heading(
                "Terminal scrollback",
                "Whether a plain terminal pane shows what was on it before Kynex last quit, above a fresh shell.",
                t,
            ))
            .children([(true, "Bring it back"), (false, "Start empty")].into_iter().enumerate().map(
                |(n, (on, label))| {
                    choice(("terminal-history", n), keep == on, label.to_string(), t)
                        .on_click(cx.listener(move |this, _, _, cx| this.set_terminal_history(on, cx)))
                },
            ));

        let notify = self.saved.background_notifications;
        let notifications = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(heading(
                "Background notifications",
                "Show an OS notification when an agent finishes or needs your input while Kynex is in the background.",
                t,
            ))
            .children([(true, "On"), (false, "Off")].into_iter().enumerate().map(|(n, (on, label))| {
                choice(("bg-notify", n), notify == on, label.to_string(), t).on_click(cx.listener(
                    move |this, _, _, cx| {
                        this.saved.background_notifications = on;
                        this.save();
                        cx.notify();
                    },
                ))
            }));

        let keep = self.saved.keep_awake;
        let awake = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(heading(
                "Keep awake",
                "Stops idle sleep while agents are working. The screen still turns off and locks.",
                t,
            ))
            .children(KeepAwake::ALL.into_iter().enumerate().map(|(n, k)| {
                choice(("keep-awake", n), keep == k, k.label().into(), t)
                    .on_click(cx.listener(move |this, _, _, cx| this.set_keep_awake(k, cx)))
            }));

        div()
            .flex()
            .flex_col()
            .gap_6()
            .child(cli)
            .child(worktrees)
            .child(shell)
            .child(history)
            .child(notifications)
            .child(awake)
    }

    fn render_appearance(&self, t: &Palette, cx: &mut Context<Self>) -> gpui::Div {
        let themes = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(heading(
                "Theme",
                "Colours for the whole window. Your own themes are JSON files in the themes folder; a change to one shows at once.",
                t,
            ))
            .children(THEMES.iter().enumerate().map(|(n, (th, name))| {
                let th = *th;
                choice(("app-theme", n), self.saved.custom_theme.is_none() && self.saved.theme == th, name.to_string(), t)
                    .on_click(cx.listener(move |this, _, _, cx| this.set_theme(th, cx)))
            }))
            .children(self.custom_themes.iter().enumerate().map(|(n, c)| {
                let id = c.id.clone();
                choice(("app-custom-theme", n), self.saved.custom_theme.as_deref() == Some(&c.id), c.name.clone(), t)
                    .on_click(cx.listener(move |this, _, _, cx| this.set_custom_theme(id.clone(), cx)))
            }))
            .children(
                self.theme_errors
                    .iter()
                    .map(|e| div().px_2().text_xs().text_color(t.error).child(format!("Theme not used: {e}"))),
            )
            .child(
                outline_button("app-themes-folder", "Open themes folder", cx).on_click(cx.listener(|this, _, _, _| {
                    let dir = kynex_core::themes::dir();
                    let _ = std::fs::create_dir_all(&dir);
                    this.open_in(Opener::Explorer, &dir)
                })),
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
                    Motion::System => "As the system (reduce motion turns them off; reduced without a GPU)",
                    Motion::On => "On",
                    Motion::Reduced => "Reduced (spinners and pulses tick slower: light on a PC without a GPU)",
                    Motion::Off => "Off",
                };
                choice(("app-motion", n), self.saved.motion == m, label.to_string(), t)
                    .on_click(cx.listener(move |this, _, _, cx| this.set_motion(m, cx)))
            }));

        let style = self.saved.tiles.resolve(t.tile);
        let stepper = |id: &'static str,
                       label: &'static str,
                       value: f32,
                       max: f32,
                       set: fn(&mut TileTweaks, f32),
                       cx: &mut Context<Self>| {
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(div().w(px(110.)).text_sm().child(label))
                .child(
                    outline_button((id, 0usize), "−", cx)
                        .on_click(cx.listener(move |this, _, _, cx| this.tune_tiles(set, (value - 1.).max(0.), cx))),
                )
                .child(div().w(px(56.)).flex().justify_center().text_sm().child(format!("{value} px")))
                .child(
                    outline_button((id, 1usize), "+", cx)
                        .on_click(cx.listener(move |this, _, _, cx| this.tune_tiles(set, (value + 1.).min(max), cx))),
                )
        };
        let tiles = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(heading(
                "Pane tiles",
                "Side-by-side panes are tiles: the gap between and around them (0 for flush panes with thin dividers), the border and the corner radius.",
                t,
            ))
            .child(stepper("app-tile-gap", "Gap", style.gap, tile_style::MAX_GAP, |s, v| s.gap = Some(v), cx))
            .child(stepper("app-tile-border", "Border", style.border, tile_style::MAX_BORDER, |s, v| s.border = Some(v), cx))
            .child(stepper("app-tile-radius", "Corner radius", style.radius, tile_style::MAX_RADIUS, |s, v| s.radius = Some(v), cx))
            .child(
                outline_button("app-tile-reset", "Use the theme's", cx)
                    .when(self.saved.tiles.is_default(), |b| b.opacity(0.5))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.saved.tiles = TileTweaks::default();
                        this.save();
                        cx.notify();
                    })),
            );

        let corner = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(heading("Toast position", "Where brief status notices show.", t))
            .children(Corner::ALL.into_iter().enumerate().map(|(n, c)| {
                choice(("app-toast-corner", n), self.saved.toast_corner == c, c.label().to_string(), t).on_click(
                    cx.listener(move |this, _, _, cx| {
                        this.saved.toast_corner = c;
                        this.save();
                        cx.notify();
                    }),
                )
            }));

        use super::keys::{ZoomIn, ZoomOut, ZoomReset};
        let zoom = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(heading("Zoom", "Text, icons and spacing in the whole window, as in a browser.", t))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        outline_button("app-zoom-out", "−", cx)
                            .tooltip(Tooltip::for_action("Smaller", ZoomOut, ""))
                            .on_click(cx.listener(|this, _, _, cx| this.zoom_by(-1, cx))),
                    )
                    .child(div().w(px(48.)).flex().justify_center().text_sm().child(scale::label(self.saved.zoom)))
                    .child(
                        outline_button("app-zoom-in", "+", cx)
                            .tooltip(Tooltip::for_action("Bigger", ZoomIn, ""))
                            .on_click(cx.listener(|this, _, _, cx| this.zoom_by(1, cx))),
                    )
                    .child(
                        outline_button("app-zoom-reset", "Reset", cx)
                            .tooltip(Tooltip::for_action("Normal size", ZoomReset, ""))
                            .on_click(cx.listener(|this, _, _, cx| this.zoom_by(0, cx))),
                    ),
            );

        let footer = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(heading(
                "Status footer",
                "Kynex's CPU and memory, OpenRouter credit and updates, along the bottom.",
                t,
            ))
            .children([(true, "Show"), (false, "Hide")].into_iter().map(|(on, label)| {
                choice(("app-footer", usize::from(on)), self.saved.footer == on, label.to_string(), t).on_click(
                    cx.listener(move |this, _, _, cx| {
                        this.saved.footer = on;
                        if on {
                            let _ = this.resources_wake.try_send(());
                        }
                        this.save();
                        cx.notify();
                    }),
                )
            }));

        div()
            .flex()
            .flex_col()
            .gap_6()
            .child(themes)
            .child(zoom)
            .child(fonts)
            .child(tiles)
            .child(corner)
            .child(footer)
            .child(motion)
    }

    pub(super) fn zoom_by(&mut self, dir: i32, cx: &mut Context<Self>) {
        self.saved.zoom = if dir == 0 { scale::NORMAL } else { scale::step(self.saved.zoom, dir) };
        self.save();
        cx.refresh_windows();
    }

    fn tune_tiles(&mut self, set: fn(&mut TileTweaks, f32), value: f32, cx: &mut Context<Self>) {
        set(&mut self.saved.tiles, value);
        self.save();
        cx.notify();
    }

    fn render_keyboard(&self, t: &Palette, cx: &mut Context<Self>) -> gpui::Div {
        let path = data_dir().join("keymap.json");
        let filter = self.app_settings.as_ref().map(|s| s.shortcuts_filter.clone());
        let query = filter.as_ref().map(|f| f.read(cx).text().to_string()).unwrap_or_default();
        let rows: Vec<_> = super::keys::in_effect()
            .iter()
            .filter(|(name, keys)| kynex_core::shortcuts::matches(name, &display_keys(keys), &query))
            .map(|(name, keys)| {
                let shown = display_keys(keys);
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .px_2()
                    .py_1()
                    .child(div().text_sm().child(kynex_core::shortcuts::label(name)))
                    .child(
                        div()
                            .font_family(mono())
                            .text_xs()
                            .text_color(if shown.is_empty() { t.text_muted } else { t.text })
                            .child(if shown.is_empty() { "unbound".to_string() } else { shown }),
                    )
            })
            .collect();
        let empty = rows.is_empty();
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(heading(
                "Shortcuts",
                "Rebind Kynex's shortcuts in keymap.json by name, e.g. { \"interrupt\": \"ctrl-c\" }; an empty string unbinds one. Shortcuts that start with `prefix` (Ctrl+B, then a key) follow the \"prefix\" entry. Changes apply from the next start.",
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
            .children(filter.map(|f| {
                div().px_2().py_1().rounded_md().border_1().border_color(t.border).text_sm().child(f)
            }))
            .child(div().flex().flex_col().children(rows))
            .when(empty, |d| d.child(div().px_2().text_sm().text_color(t.text_muted).child("No shortcut matches.")))
    }
}

fn display_keys(keys: &str) -> String {
    kynex_core::shortcuts::display(keys, cfg!(target_os = "macos"))
}

/// Section heading inside Settings.
pub(super) fn heading(title: &'static str, body: &'static str, t: &Palette) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap_0p5()
        .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(title))
        .child(div().text_xs().text_color(t.text_muted).child(body))
}

/// A radio button circle.
pub(super) fn radio(selected: bool, t: &Palette) -> gpui::Div {
    div()
        .flex_none()
        .mt(px(3.))
        .size(px(14.))
        .rounded_full()
        .border_1()
        .border_color(if selected { t.accent } else { t.border })
        .flex()
        .items_center()
        .justify_center()
        .when(selected, |d| d.child(div().size(px(6.)).rounded_full().bg(t.accent)))
}

/// A radio row: a label with a dot when it's the current choice.
pub(super) fn choice(
    id: impl Into<gpui::ElementId>,
    selected: bool,
    label: String,
    t: &Palette,
) -> gpui::Stateful<gpui::Div> {
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

/// Left sidebar navigation inside the Settings modal.
fn settings_nav(
    sections: &[&'static str],
    active: usize,
    pick: impl Fn(&mut KynexApp, usize) + Copy + 'static,
    cx: &mut Context<KynexApp>,
) -> gpui::Div {
    let t = theme(cx).clone();
    let hover = t.hover;
    div()
        .flex()
        .flex_col()
        .gap_0p5()
        .w(px(156.))
        .flex_none()
        .p_2()
        .border_r_1()
        .border_color(t.border_variant)
        .children(sections.iter().enumerate().map(|(i, label)| {
            let on = i == active;
            div()
                .id(("settings-nav", i))
                .px_2p5()
                .py_1p5()
                .rounded_md()
                .text_sm()
                .cursor_pointer()
                .text_color(if on { t.text } else { t.text_muted })
                .when(on, |d| d.bg(t.active))
                .when(!on, |d| d.hover(move |s| s.bg(hover)))
                .child(*label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    pick(this, i);
                    cx.notify();
                }))
        }))
}

/// Centered Settings modal shell.
#[allow(clippy::too_many_arguments)]
fn settings_modal(
    title: &'static str,
    nav: AnyElement,
    content: AnyElement,
    footer: String,
    close: fn(&mut KynexApp),
    vp: Size<gpui::Pixels>,
    leaving: Option<f32>,
    cx: &mut Context<KynexApp>,
) -> AnyElement {
    let t = theme(cx).clone();
    let scroll = scrollbar::handle("settings-scroll");
    let body = div()
        .id("settings-scroll")
        .flex_1()
        .min_w_0()
        .min_h_0()
        .overflow_y_scroll()
        .track_scroll(&scroll)
        .p_5()
        .child(content);
    let card = div()
        .id("settings-modal")
        .occlude()
        .flex()
        .flex_col()
        .w(gpui::px(760.).min(vp.width - gpui::px(80.)))
        .h(gpui::px(600.).min(vp.height - gpui::px(100.)))
        .rounded_lg()
        .overflow_hidden()
        .border_1()
        .border_color(t.border)
        .bg(t.panel)
        .shadow_lg()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(
            div()
                .flex()
                .flex_none()
                .items_center()
                .h(px(44.))
                .px_4()
                .border_b_1()
                .border_color(t.border_variant)
                .child(div().flex_1().text_base().font_weight(FontWeight::SEMIBOLD).child(title))
                .child(
                    icon_button("settings-close", Icon::Close, cx)
                        .tooltip(Tooltip::with_meta("Close", "Esc"))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            close(this);
                            cx.notify();
                        })),
                ),
        )
        .child(div().flex().flex_1().min_h_0().child(nav).child(scrollbar::vertical_over(
            "settings-bar",
            scroll,
            body,
            |d| d.flex_1().min_w_0().min_h_0(),
        )))
        .child(
            div()
                .flex_none()
                .px_4()
                .py_2()
                .border_t_1()
                .border_color(t.border_variant)
                .text_xs()
                .font_family(mono())
                .text_color(t.text_placeholder)
                .truncate()
                .child(footer),
        );
    fade(
        "settings-fade",
        div()
            .id("settings-backdrop")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(t.scrim())
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
