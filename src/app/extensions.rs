//! The Extensions modal, from the Kynex menu or the session header's MCP dot:
//! what extends Claude in the folder, where each piece is configured and how
//! it's doing. MCP servers come with the open session's live status; plugins
//! with their version and whether they're on. Read-only for now; the files are
//! read by `kynex_core::extensions`.

use std::path::PathBuf;

use gpui::{AnyElement, Context, FontWeight, Window, div, prelude::*, px};
use kynex_core::extensions::{self, Approval, Config, Plugin, Row, Source};
use kynex_core::mcp::{self, Status};
use kynex_core::paths;

use super::KynexApp;
use super::settings::{heading, settings_modal, settings_nav};
use crate::theme::{Palette, mono, theme};
use crate::ui::{Tooltip, outline_button, spinner};

/// Where claude.ai connectors are connected and authorised.
const CONNECTORS_URL: &str = "https://claude.ai/settings/connectors";

const SECTIONS: [&str; 2] = ["MCP servers", "Plugins"];

#[derive(Clone)]
pub struct ExtensionsView {
    cwd: PathBuf,
    section: usize,
    /// `None` while the files are read.
    config: Option<Config>,
}

impl KynexApp {
    /// Opens the Extensions modal for the active tab's folder, on `section`.
    pub(super) fn open_extensions(&mut self, section: usize, cx: &mut Context<Self>) {
        let cwd = self.active_project().map_or_else(paths::chat_dir, |p| p.path.clone());
        self.settings = None;
        self.app_settings = None;
        self.extensions = Some(ExtensionsView { cwd, section, config: None });
        self.load_extensions(cx);
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
            Some(config) if section == 0 => {
                // The open session's servers, if it runs in this folder.
                let live = self.active_session().filter(|s| s.cwd == view.cwd).map(|s| s.mcp.clone());
                self.render_mcp_servers(config, live, &t, cx)
            }
            Some(config) => render_plugins(&config.plugins, &t),
        };
        let content = div()
            .flex()
            .flex_col()
            .gap_4()
            .children(view.config.as_ref().map(|c| render_errors(&c.errors, &t)))
            .child(content);
        let footer = "Read from ~/.claude.json, the folder's .mcp.json and settings, and ~/.claude/plugins.".into();
        let close = |this: &mut Self| this.extensions = None;
        let (nav, content) = (nav.into_any_element(), content.into_any_element());
        Some(settings_modal(("Extensions", subtitle), nav, content, footer, close, window.viewport_size(), leaving, cx))
    }

    fn render_mcp_servers(
        &self,
        config: &Config,
        live: Option<Vec<mcp::Server>>,
        t: &Palette,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let has_live = live.is_some();
        let rows = extensions::rows(&config.servers, &live.unwrap_or_default());
        let help = if has_live {
            "What sessions in this folder start with, and how the open session's are doing."
        } else {
            "What sessions in this folder start with. Open a session to see how they're doing."
        };
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div().flex().items_start().gap_2().child(div().flex_1().child(heading("MCP servers", help, t))).child(
                    outline_button("extensions-reload", "Reload", cx)
                        .tooltip(Tooltip::text("Read the config files again"))
                        .on_click(cx.listener(|this, _, _, cx| this.load_extensions(cx))),
                ),
            )
            .when(rows.is_empty(), |d| {
                d.child(
                    div().text_sm().text_color(t.text_muted).child("No MCP servers are configured for this folder."),
                )
            })
            .children(rows.iter().enumerate().map(|(n, r)| server_row(n, r, t, cx)))
    }
}

/// One server: a status dot, its name, where it's configured, what it runs or
/// where it connects, and what's in the way when it isn't working.
fn server_row(n: usize, r: &Row, t: &Palette, cx: &mut Context<KynexApp>) -> AnyElement {
    let (color, state) = match (&r.status, r.approval) {
        (_, Approval::Rejected) => (t.text_placeholder, "turned off here".to_string()),
        (None, Approval::Pending) => (t.warning, "waiting for you to allow it".to_string()),
        (None, _) => (t.text_placeholder, "not started".to_string()),
        (Some(s), _) => (status_color(s, t), s.label().to_string()),
    };
    let detail = r.transport.as_ref().map(|tr| (tr.label().to_string(), tr.detail()));
    let note = match (&r.status, r.approval, &r.source) {
        (Some(Status::NeedsAuth), _, Source::ClaudeAi) => {
            Some("Connect it on claude.ai; sessions pick it up when they start.")
        }
        (Some(Status::NeedsAuth), _, _) => {
            Some("It needs you to sign in. Run /mcp in a terminal session to authenticate.")
        }
        (Some(Status::Failed), _, _) => {
            Some("It didn't start. Check its command, or that the program it runs is installed.")
        }
        (_, Approval::Pending, _) => Some("A project server: Claude Code asks you to allow it when a session starts."),
        _ => None,
    };
    let connect = matches!((&r.status, &r.source), (Some(Status::NeedsAuth), Source::ClaudeAi)).then(|| {
        outline_button(("extensions-connect", n), "Open claude.ai", cx).on_click(|_, _, cx| cx.open_url(CONNECTORS_URL))
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

fn chip(text: impl Into<gpui::SharedString>, t: &Palette) -> gpui::Div {
    div()
        .flex_none()
        .px_1()
        .rounded_sm()
        .border_1()
        .border_color(t.border_variant)
        .text_color(t.text_muted)
        .child(text.into())
}
