//! The Extensions modal, from the Kynex menu or the session header's MCP dot:
//! what extends Claude in the folder, where each piece is configured and how
//! it's doing. MCP servers come with the open session's live status, or
//! `claude mcp list`'s when you check; plugins with their version and whether
//! they're on. Servers are added (`extensions_add.rs`) and removed through the
//! CLI; the files are read by `kynex_core::extensions`.

use std::path::{Path, PathBuf};

use crate::ui::px;
use gpui::{AnyElement, Context, Entity, FontWeight, Window, div, prelude::*};
use kynex_core::extensions::{self, Approval, Config, Plugin, Row, Source};
use kynex_core::mcp::{self, Status};
use kynex_core::mcp_config::{self, Browser, Scope};
use kynex_core::paths;

use super::KynexApp;
use super::settings::{heading, settings_modal, settings_nav};
use crate::theme::{Palette, mono, theme};
use crate::ui::input::{InputEvent, TextInput};
use crate::ui::{Tooltip, outline_button, spinner};

/// Where claude.ai connectors are connected and authorised.
const CONNECTORS_URL: &str = "https://claude.ai/settings/connectors";

const SECTIONS: [&str; 2] = ["MCP servers", "Plugins"];

#[derive(Clone)]
pub struct ExtensionsView {
    pub(super) cwd: PathBuf,
    section: usize,
    /// `None` while the files are read.
    pub(super) config: Option<Config>,
    /// Every server's health from `claude mcp list`, once checked.
    checked: Option<Vec<mcp::Server>>,
    checking: bool,
    /// What's being changed through the CLI right now ("Adding edge…").
    pub(super) busy: Option<String>,
    /// The last check or change that failed, as the CLI said it.
    pub(super) error: Option<String>,
    /// The server whose Remove was clicked once, waiting for the second click.
    confirm_remove: Option<String>,
    /// The Add server panel, while open.
    pub(super) adding: Option<AddForm>,
}

/// The Add server panel: the browser preset, or a server of your own.
#[derive(Clone)]
pub(super) struct AddForm {
    pub(super) browser: Browser,
    /// Attach to your own browser through its extension, keeping your logins.
    pub(super) own_profile: bool,
    pub(super) name: Entity<TextInput>,
    /// A command line, or an http(s) URL.
    pub(super) what: Entity<TextInput>,
    pub(super) scope: Scope,
    /// Looked up once when the panel opens, not every frame.
    pub(super) npx: bool,
    pub(super) installed: Vec<Browser>,
}

impl KynexApp {
    /// Opens the Extensions modal for the active tab's folder, on `section`.
    pub(super) fn open_extensions(&mut self, section: usize, cx: &mut Context<Self>) {
        let cwd = self.active_project().map_or_else(paths::chat_dir, |p| p.path.clone());
        self.settings = None;
        self.app_settings = None;
        self.extensions = Some(ExtensionsView {
            cwd,
            section,
            config: None,
            checked: None,
            checking: false,
            busy: None,
            error: None,
            confirm_remove: None,
            adding: None,
        });
        self.load_extensions(cx);
        cx.notify();
    }

    /// Opens the Add server panel, on the first browser that's installed.
    pub(super) fn open_add_server(&mut self, cx: &mut Context<Self>) {
        let input = |placeholder: &'static str, cx: &mut Context<Self>| {
            let input = cx.new(|cx| TextInput::new(cx).mono().placeholder(placeholder));
            cx.subscribe(&input, |this, _, ev, cx| {
                if let InputEvent::Submit = ev {
                    this.add_custom_server(cx);
                }
            })
            .detach();
            input
        };
        let installed: Vec<Browser> = Browser::ALL.into_iter().filter(|b| b.installed()).collect();
        let form = AddForm {
            browser: installed.first().copied().unwrap_or(Browser::Edge),
            own_profile: true,
            name: input("my-server", cx),
            what: input("npx some-mcp-server, or https://…", cx),
            scope: Scope::Local,
            npx: mcp_config::npx_found(),
            installed,
        };
        if let Some(view) = &mut self.extensions {
            view.adding = Some(form);
            view.error = None;
        }
        cx.notify();
    }

    /// Reads the folder's config again, off the UI thread.
    fn load_extensions(&mut self, cx: &mut Context<Self>) {
        let Some(view) = &mut self.extensions else { return };
        view.config = None;
        let cwd = view.cwd.clone();
        cx.spawn(async move |this, cx| {
            let config = cx
                .background_spawn({
                    let cwd = cwd.clone();
                    async move { extensions::load(&cwd) }
                })
                .await;
            this.update(cx, |this, cx| {
                if let Some(view) = this.extensions.as_mut().filter(|v| v.cwd == cwd) {
                    view.config = Some(config);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Asks `claude mcp list` how every server is doing, which starts each one.
    fn check_servers(&mut self, cx: &mut Context<Self>) {
        let Some(view) = self.extensions.as_mut().filter(|v| !v.checking) else { return };
        view.checking = true;
        let cwd = view.cwd.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn({
                    let cwd = cwd.clone();
                    async move { mcp_config::check(&cwd) }
                })
                .await;
            this.update(cx, |this, cx| {
                if let Some(view) = this.extensions.as_mut().filter(|v| v.cwd == cwd) {
                    view.checking = false;
                    match result {
                        Ok(servers) => view.checked = Some(servers),
                        Err(e) => view.error = Some(e),
                    }
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Runs a change through the CLI (`busy` says what), then reads the config
    /// again and checks the servers, so a new one shows whether it works.
    pub(super) fn change_servers(
        &mut self,
        busy: String,
        change: impl FnOnce(&Path) -> Result<(), String> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        let Some(view) = self.extensions.as_mut().filter(|v| v.busy.is_none()) else { return };
        view.busy = Some(busy);
        view.error = None;
        view.confirm_remove = None;
        let cwd = view.cwd.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn({
                    let cwd = cwd.clone();
                    async move { change(&cwd) }
                })
                .await;
            this.update(cx, |this, cx| {
                let Some(view) = this.extensions.as_mut().filter(|v| v.cwd == cwd) else { return };
                view.busy = None;
                match result {
                    Ok(()) => {
                        view.adding = None;
                        this.load_extensions(cx);
                        this.check_servers(cx);
                    }
                    Err(e) => view.error = Some(e),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn remove_server(&mut self, name: String, scope: Scope, cx: &mut Context<Self>) {
        let Some(view) = &mut self.extensions else { return };
        if view.confirm_remove.as_ref() != Some(&name) {
            view.confirm_remove = Some(name);
            cx.notify();
            return;
        }
        let busy = format!("Removing {name}…");
        self.change_servers(busy, move |cwd| mcp_config::remove(cwd, &name, scope), cx);
    }

    pub(super) fn render_extensions(
        &mut self,
        leaving: Option<f32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let view = self.extensions.as_ref()?;
        let t = theme(cx).clone();
        let section = view.section.min(SECTIONS.len() - 1);
        let subtitle = Some(paths::project_name(&view.cwd));
        let nav = settings_nav(
            &SECTIONS,
            section,
            |this, i| {
                if let Some(v) = &mut this.extensions {
                    v.section = i;
                }
            },
            cx,
        );
        let content = match &view.config {
            None => div()
                .flex()
                .items_center()
                .gap_2()
                .text_sm()
                .text_color(t.text_muted)
                .child(spinner("extensions-loading", t.accent, px(14.)))
                .child("Reading Claude Code's config…"),
            Some(_) if section == 0 => self.render_mcp_servers(&t, cx),
            Some(config) => render_plugins(&config.plugins, &t),
        };
        let content = div()
            .flex()
            .flex_col()
            .gap_4()
            .children(view.config.as_ref().map(|c| render_errors(&c.errors, &t)))
            .child(content);
        let footer = "Read from ~/.claude.json, the folder's .mcp.json and settings, and ~/.claude/plugins; \
                      changed through claude mcp."
            .into();
        let close = |this: &mut Self| this.extensions = None;
        let (nav, content) = (nav.into_any_element(), content.into_any_element());
        Some(settings_modal(("Extensions", subtitle), nav, content, footer, close, window.viewport_size(), leaving, cx))
    }

    fn render_mcp_servers(&self, t: &Palette, cx: &mut Context<Self>) -> gpui::Div {
        let Some(view) = &self.extensions else { return div() };
        let Some(config) = &view.config else { return div() };
        // The open session's servers if it runs in this folder, then what a check found.
        let mut live = self.active_session().filter(|s| s.cwd == view.cwd && !s.mcp.is_empty()).map(|s| s.mcp.clone());
        let has_session = live.is_some();
        for s in view.checked.iter().flatten() {
            let list = live.get_or_insert_default();
            if !list.iter().any(|l| l.name == s.name) {
                list.push(s.clone());
            }
        }
        let rows = extensions::rows(&config.servers, &live.unwrap_or_default());
        let help = if has_session {
            "What sessions in this folder start with, and how the open session's are doing. \
             Sessions pick up changes when they start."
        } else {
            "What sessions in this folder start with. Check status to start each one and see how it's doing."
        };
        let check = if view.checking {
            div()
                .flex()
                .items_center()
                .gap_1()
                .text_xs()
                .text_color(t.text_muted)
                .child(spinner("extensions-checking", t.accent, px(12.)))
                .child("Checking…")
                .into_any_element()
        } else {
            outline_button("extensions-check", "Check status", cx)
                .tooltip(Tooltip::text("Start every server with `claude mcp list` and see how it's doing"))
                .on_click(cx.listener(|this, _, _, cx| this.check_servers(cx)))
                .into_any_element()
        };
        let buttons = div()
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .child(check)
            .child(
                outline_button("extensions-reload", "Reload", cx)
                    .tooltip(Tooltip::text("Read the config files again"))
                    .on_click(cx.listener(|this, _, _, cx| this.load_extensions(cx))),
            )
            .when(view.adding.is_none(), |d| {
                d.child(
                    outline_button("extensions-add", "Add server", cx)
                        .on_click(cx.listener(|this, _, _, cx| this.open_add_server(cx))),
                )
            });
        let busy = view.busy.clone().map(|b| {
            div()
                .flex()
                .items_center()
                .gap_2()
                .text_xs()
                .text_color(t.text_muted)
                .child(spinner("extensions-busy", t.accent, px(12.)))
                .child(b)
        });
        let error = view.error.clone().map(|e| div().text_xs().text_color(t.error).child(e));
        let confirm = view.confirm_remove.clone();
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap_2()
                    .child(div().flex_1().min_w_0().child(heading("MCP servers", help, t)))
                    .child(buttons),
            )
            .children(busy)
            .children(error)
            .children(view.adding.as_ref().map(|form| self.render_add_server(form, t, cx)))
            .when(rows.is_empty(), |d| {
                d.child(
                    div().text_sm().text_color(t.text_muted).child("No MCP servers are configured for this folder."),
                )
            })
            .children(rows.iter().enumerate().map(|(n, r)| {
                let confirming = confirm.as_ref() == Some(&r.name);
                server_row(n, r, confirming, t, cx)
            }))
    }
}

/// One server: a status dot, its name, where it's configured, what it runs or
/// where it connects, and what's in the way when it isn't working.
fn server_row(n: usize, r: &Row, confirming: bool, t: &Palette, cx: &mut Context<KynexApp>) -> AnyElement {
    let (color, state) = match (&r.status, r.approval) {
        (_, Approval::Rejected) => (t.text_placeholder, "turned off here".to_string()),
        (None, Approval::Pending) => (t.warning, "waiting for you to allow it".to_string()),
        (None, _) => (t.text_placeholder, "not started".to_string()),
        (Some(s), _) => (status_color(s, t), s.label().to_string()),
    };
    let detail = r.transport.as_ref().map(|tr| (tr.label().to_string(), tr.detail()));
    let browser = r.transport.as_ref().is_some_and(|tr| tr.detail().contains("--extension"));
    let note = match (&r.status, r.approval, &r.source) {
        (Some(Status::NeedsAuth), _, Source::ClaudeAi) => {
            Some("Connect it on claude.ai; sessions pick it up when they start.")
        }
        (Some(Status::NeedsAuth), _, _) => {
            Some("It needs you to sign in. Run /mcp in a terminal session to authenticate.")
        }
        (Some(Status::Failed), _, _) if browser => Some(
            "It didn't start. Check that Node.js is installed; the browser's Playwright extension is only \
             needed once Claude uses it.",
        ),
        (Some(Status::Failed), _, _) => {
            Some("It didn't start. Check its command, or that the program it runs is installed.")
        }
        (Some(Status::Connected), _, _) if browser => {
            Some("Uses your own browser: the Playwright extension asks you to allow each connection.")
        }
        (_, Approval::Pending, _) => Some("A project server: Claude Code asks you to allow it when a session starts."),
        _ => None,
    };
    let connect = matches!((&r.status, &r.source), (Some(Status::NeedsAuth), Source::ClaudeAi)).then(|| {
        outline_button(("extensions-connect", n), "Open claude.ai", cx).on_click(|_, _, cx| cx.open_url(CONNECTORS_URL))
    });
    let remove = Scope::of(&r.source).map(|scope| {
        let name = r.name.clone();
        let label = if confirming { "Remove?" } else { "Remove" };
        let tip = format!("Remove it from {}", scope.label().to_lowercase());
        outline_button(("extensions-remove", n), label, cx)
            .when(confirming, |b| b.text_color(t.error))
            .tooltip(Tooltip::text(if confirming { "Click again to remove it".to_string() } else { tip }))
            .on_click(cx.listener(move |this, _, _, cx| this.remove_server(name.clone(), scope, cx)))
    });
    div()
        .flex()
        .items_start()
        .gap_3()
        .px_3()
        .py_2()
        .rounded_md()
        .border_1()
        .border_color(t.border_variant)
        .bg(t.panel)
        .child(div().mt(px(6.)).size(px(7.)).flex_none().rounded_full().bg(color))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap_0p5()
                .child(
                    div()
                        .flex()
                        .items_baseline()
                        .gap_2()
                        .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(r.label()))
                        .when(r.label() != r.name, |d| {
                            d.child(
                                div()
                                    .text_xs()
                                    .font_family(mono())
                                    .text_color(t.text_placeholder)
                                    .child(r.name.clone()),
                            )
                        })
                        .child(div().flex_1())
                        .child(div().flex_none().text_xs().text_color(color).child(state)),
                )
                .child(
                    div().flex().gap_2().text_xs().text_color(t.text_muted).child(chip(r.source.label(), t)).children(
                        detail.map(|(kind, what)| {
                            div()
                                .flex()
                                .min_w_0()
                                .gap_2()
                                .child(chip(kind, t))
                                .child(div().min_w_0().truncate().font_family(mono()).child(what))
                        }),
                    ),
                )
                .children(note.map(|n| div().text_xs().text_color(t.text_muted).child(n))),
        )
        .children(connect)
        .children(remove)
        .into_any_element()
}

fn render_plugins(plugins: &[Plugin], t: &Palette) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(heading(
            "Plugins",
            "Installed for you, or for this folder. Plugins bring MCP servers, skills, commands, agents and hooks.",
            t,
        ))
        .when(plugins.is_empty(), |d| d.child(div().text_sm().text_color(t.text_muted).child("No plugins installed.")))
        .children(plugins.iter().map(|p| {
            let (color, state) = if p.enabled { (t.success, "on") } else { (t.text_placeholder, "off") };
            div()
                .flex()
                .items_center()
                .gap_3()
                .px_3()
                .py_2()
                .rounded_md()
                .border_1()
                .border_color(t.border_variant)
                .bg(t.panel)
                .child(div().size(px(7.)).flex_none().rounded_full().bg(color))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap_0p5()
                        .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(p.name().to_string()))
                        .child(
                            div()
                                .flex()
                                .gap_2()
                                .text_xs()
                                .text_color(t.text_muted)
                                .child(chip(scope_label(&p.scope), t))
                                .child(p.marketplace().to_string())
                                .when(!p.version.is_empty(), |d| {
                                    d.child(div().font_family(mono()).child(short_version(&p.version)))
                                }),
                        ),
                )
                .child(div().flex_none().text_xs().text_color(color).child(state))
        }))
}

/// Files that couldn't be read, so a broken config isn't silently empty.
fn render_errors(errors: &[String], t: &Palette) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .children(errors.iter().map(|e| div().text_xs().text_color(t.error).child(e.clone())))
        .into_any_element()
}

fn status_color(s: &Status, t: &Palette) -> gpui::Hsla {
    match s {
        Status::Connected => t.success,
        Status::Pending | Status::NeedsAuth => t.warning,
        Status::Failed | Status::Other(_) => t.error,
        Status::Disabled => t.text_placeholder,
    }
}

fn scope_label(scope: &str) -> String {
    match scope {
        "user" => "User".into(),
        "project" => "Project".into(),
        "local" => "Local".into(),
        other => other.into(),
    }
}

/// Git-hash versions are long; the first 7 characters are enough.
fn short_version(v: &str) -> String {
    if v.len() > 10 && v.chars().all(|c| c.is_ascii_hexdigit()) { v[..7].into() } else { v.into() }
}

pub(super) fn chip(text: impl Into<gpui::SharedString>, t: &Palette) -> gpui::Div {
    div()
        .flex_none()
        .px_1()
        .rounded_sm()
        .border_1()
        .border_color(t.border_variant)
        .text_color(t.text_muted)
        .child(text.into())
}
