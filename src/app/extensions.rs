//! The Extensions modal, from the Kynex menu or the session header's MCP dot:
//! what extends Claude in the folder, where each piece is configured and how
//! it's doing. MCP servers (`extensions_servers.rs`) come with the open
//! session's live status and tools, or `claude mcp list`'s when you check;
//! plugins (`extensions_plugins.rs`) with what they add, and marketplaces to
//! install from; skills, commands, agents and hooks (`extensions_catalog.rs`)
//! read-only, with where each is defined. Servers are added
//! (`extensions_add.rs`) and everything changed through the CLI; the files are
//! read by `kynex_core::extensions` and `kynex_core::catalog`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::ui::px;
use gpui::{AnyElement, Context, Entity, Window, div, prelude::*};
use kynex_core::catalog::{self, Hook, Item};
use kynex_core::extensions::{self, Config};
use kynex_core::mcp;
use kynex_core::mcp_config::{self, Browser, Scope};
use kynex_core::plugins::{self, Available, Contributes, Marketplace};
use kynex_core::{mcp_logs, paths};

use super::KynexApp;
use super::settings::{settings_modal, settings_nav};
use crate::theme::{Palette, theme};
use crate::ui::input::{InputEvent, TextInput};
use crate::ui::spinner;

const SECTIONS: [&str; 5] = ["MCP servers", "Plugins", "Skills & commands", "Agents", "Hooks"];

#[derive(Clone)]
pub struct ExtensionsView {
    pub(super) cwd: PathBuf,
    section: usize,
    /// `None` while the files are read.
    pub(super) config: Option<Config>,
    /// Skills, agents, hooks, what plugins add and servers' last errors;
    /// `None` while read.
    pub(super) extra: Option<Extra>,
    /// Every server's health from `claude mcp list`, once checked.
    pub(super) checked: Option<Vec<mcp::Server>>,
    pub(super) checking: bool,
    /// What's being changed through the CLI right now ("Adding edge…").
    pub(super) busy: Option<String>,
    /// The last check or change that failed, as the CLI said it.
    pub(super) error: Option<String>,
    /// The server whose Remove was clicked once, waiting for the second click.
    pub(super) confirm_remove: Option<String>,
    /// The Add server panel, while open.
    pub(super) adding: Option<AddForm>,
    /// Servers whose tools are shown.
    pub(super) open_tools: HashSet<String>,
    /// Servers being signed in to or tested.
    pub(super) working: HashSet<String>,
    /// How a server's last sign-in, test or restart went.
    pub(super) results: HashMap<String, Result<String, String>>,
    /// The marketplace browser, once opened.
    pub(super) market: Option<Market>,
    /// The plugin whose Uninstall was clicked once.
    pub(super) confirm_plugin: Option<String>,
    /// A marketplace command waiting for the person's yes before it runs.
    pub(super) confirm_command: Option<PendingCommand>,
}

/// What's read besides the CLI's MCP config, off the UI thread.
#[derive(Clone, Default)]
pub(super) struct Extra {
    pub(super) items: Vec<Item>,
    pub(super) agents: Vec<Item>,
    pub(super) hooks: Vec<Hook>,
    /// By plugin id.
    pub(super) contributes: HashMap<String, Contributes>,
    /// Each configured server's last logged error, by session name.
    pub(super) logs: HashMap<String, String>,
}

/// The Add server panel: the browser preset, or a server of your own.
#[derive(Clone)]
pub(super) struct AddForm {
    pub(super) browser: Browser,
    /// Attach to your own browser through its extension, keeping your logins.
    pub(super) own_profile: bool,
    /// The extension's token, so it connects without asking each time.
    pub(super) token: Entity<TextInput>,
    pub(super) name: Entity<TextInput>,
    /// A command line, or an http(s) URL.
    pub(super) what: Entity<TextInput>,
    pub(super) scope: Scope,
    /// Looked up once when the panel opens, not every frame.
    pub(super) npx: bool,
    pub(super) installed: Vec<Browser>,
}

/// Plugins the added marketplaces offer, and adding another marketplace.
#[derive(Clone)]
pub(super) struct Market {
    /// `None` while the CLI lists them.
    pub(super) available: Option<Result<Vec<Available>, String>>,
    pub(super) marketplaces: Vec<Marketplace>,
    pub(super) filter: Entity<TextInput>,
    pub(super) source: Entity<TextInput>,
    /// Where installs go: `user`, `project` or `local`.
    pub(super) scope: &'static str,
}

/// An install or update whose marketplace wants to run a command first.
#[derive(Clone)]
pub(super) struct PendingCommand {
    pub(super) id: String,
    pub(super) scope: String,
    pub(super) action: plugins::Action,
    pub(super) command: String,
    pub(super) sha256: String,
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
            extra: None,
            checked: None,
            checking: false,
            busy: None,
            error: None,
            confirm_remove: None,
            adding: None,
            open_tools: HashSet::new(),
            working: HashSet::new(),
            results: HashMap::new(),
            market: None,
            confirm_plugin: None,
            confirm_command: None,
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
        let token = cx.new(|cx| TextInput::new(cx).mono().placeholder("Paste the token (optional)"));
        cx.subscribe(&token, |this, _, ev, cx| {
            if let InputEvent::Submit = ev {
                this.add_browser(cx);
            }
        })
        .detach();
        let installed: Vec<Browser> = Browser::ALL.into_iter().filter(|b| b.installed()).collect();
        let form = AddForm {
            browser: installed.first().copied().unwrap_or(Browser::Edge),
            own_profile: true,
            token,
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

    /// Reads the folder's config, and what else extends Claude there, again,
    /// off the UI thread.
    pub(super) fn load_extensions(&mut self, cx: &mut Context<Self>) {
        let Some(view) = &mut self.extensions else { return };
        view.config = None;
        let cwd = view.cwd.clone();
        cx.spawn(async move |this, cx| {
            let (config, extra) = cx
                .background_spawn({
                    let cwd = cwd.clone();
                    async move {
                        let config = extensions::load(&cwd);
                        let extra = read_extra(&cwd, &config);
                        (config, extra)
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                if let Some(view) = this.extensions.as_mut().filter(|v| v.cwd == cwd) {
                    view.config = Some(config);
                    view.extra = Some(extra);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Asks `claude mcp list` how every server is doing, which starts each one.
    pub(super) fn check_servers(&mut self, cx: &mut Context<Self>) {
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
                // A server that failed may have logged why.
                this.load_extensions(cx);
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
        let content = match (&view.config, &view.extra) {
            (Some(_), Some(_)) => match section {
                0 => self.render_mcp_servers(&t, cx),
                1 => self.render_plugins(&t, cx),
                2 => self.render_skills(&t),
                3 => self.render_agents(&t),
                _ => self.render_hooks(&t),
            },
            _ => div()
                .flex()
                .items_center()
                .gap_2()
                .text_sm()
                .text_color(t.text_muted)
                .child(spinner("extensions-loading", t.accent, px(14.)))
                .child("Reading Claude Code's config…"),
        };
        let content = div()
            .flex()
            .flex_col()
            .gap_4()
            .children(view.config.as_ref().map(|c| render_errors(&c.errors, &t)))
            .child(content);
        let footer = match section {
            0 => "Read from ~/.claude.json and the folder's .mcp.json and settings; changed through claude mcp.",
            1 => "Read from ~/.claude/plugins; changed through claude plugin.",
            4 => {
                "Read from your settings, the folder's .claude/settings.json and settings.local.json, and plugins. \
                  Change hooks there, or with /hooks."
            }
            _ => "Read from ~/.claude, the folder's .claude and plugins. Change them in those files.",
        }
        .into();
        let close = |this: &mut Self| this.extensions = None;
        let (nav, content) = (nav.into_any_element(), content.into_any_element());
        Some(settings_modal(("Extensions", subtitle), nav, content, footer, close, window.viewport_size(), leaving, cx))
    }
}

/// Skills, agents and hooks for the folder, what each plugin adds, and each
/// configured server's last logged error. Blocks reading files.
fn read_extra(cwd: &Path, config: &Config) -> Extra {
    let Some(paths) = extensions::Paths::detect() else { return Extra::default() };
    let plugins = &config.plugins;
    Extra {
        items: catalog::skills_and_commands(&paths, cwd, plugins),
        agents: catalog::agents(&paths, cwd, plugins),
        hooks: catalog::hooks(&paths, cwd, plugins),
        contributes: plugins.iter().map(|p| (p.id.clone(), plugins::contributes(p))).collect(),
        logs: config
            .servers
            .iter()
            .filter_map(|s| {
                let name = s.live_name();
                mcp_logs::last_error(cwd, &name).map(|e| (name, e))
            })
            .collect(),
    }
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

/// The busy line and the last error, under a section's heading.
pub(super) fn status_lines(view: &ExtensionsView, t: &Palette) -> impl Iterator<Item = gpui::Div> + use<> {
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
    busy.into_iter().chain(error)
}

/// A bordered row, as every list here uses.
pub(super) fn card(t: &Palette) -> gpui::Div {
    div().flex().gap_3().px_3().py_2().rounded_md().border_1().border_color(t.border_variant).bg(t.panel)
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
