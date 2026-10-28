//! The Extensions modal's Add server panel: the browser preset (Playwright on
//! your own Edge or Chrome, or a fresh one) and a server of your own, both
//! added through `claude mcp add`.

use gpui::{AnyElement, Context, Entity, FontWeight, div, prelude::*};
use kynex_core::mcp_config::{self, BROWSER_EXTENSION_URL, Browser, NewServer, Scope};

use super::KynexApp;
use super::extensions::{AddForm, chip};
use super::settings::radio;
use crate::theme::{Palette, mono};
use crate::ui::input::TextInput;
use crate::ui::{Tooltip, outline_button, primary_button};

const NODE_URL: &str = "https://nodejs.org/";

impl KynexApp {
    fn edit_add_form(&mut self, cx: &mut Context<Self>, edit: impl FnOnce(&mut AddForm)) {
        if let Some(form) = self.extensions.as_mut().and_then(|v| v.adding.as_mut()) {
            edit(form);
            cx.notify();
        }
    }

    fn add_server(&mut self, server: NewServer, cx: &mut Context<Self>) {
        let busy = format!("Adding {}…", server.name);
        self.change_servers(busy, move |cwd| mcp_config::add(cwd, &server), cx);
    }

    fn add_browser(&mut self, cx: &mut Context<Self>) {
        let Some(form) = self.extensions.as_ref().and_then(|v| v.adding.as_ref()) else { return };
        let server = form.browser.preset(form.own_profile);
        self.add_server(server, cx);
    }

    /// Adds what's typed in the panel's own-server fields.
    pub(super) fn add_custom_server(&mut self, cx: &mut Context<Self>) {
        let Some(form) = self.extensions.as_ref().and_then(|v| v.adding.as_ref()) else { return };
        let (name, what) = (form.name.read(cx).text().to_string(), form.what.read(cx).text().to_string());
        match NewServer::parse(&name, form.scope, &what) {
            Ok(server) => self.add_server(server, cx),
            Err(e) => {
                if let Some(view) = &mut self.extensions {
                    view.error = Some(e);
                }
                cx.notify();
            }
        }
    }

    pub(super) fn render_add_server(&self, form: &AddForm, t: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let busy = self.extensions.as_ref().is_some_and(|v| v.busy.is_some());
        let browser = form.browser;
        let preset = browser.preset(form.own_profile);
        let browsers = div().flex().gap_1().children(Browser::ALL.into_iter().enumerate().map(|(n, b)| {
            let installed = form.installed.contains(&b);
            let label = if installed { b.label().to_string() } else { format!("{} (not found)", b.label()) };
            choice(("ext-browser", n), b == browser, label, None, t)
                .on_click(cx.listener(move |this, _, _, cx| this.edit_add_form(cx, |f| f.browser = b)))
        }));
        let profiles = div()
            .flex()
            .flex_col()
            .child(
                choice(
                    "ext-own-profile",
                    form.own_profile,
                    "Your own browser".into(),
                    Some(format!(
                        "Keeps your logins. Needs the Playwright extension in {}, which asks before each connection.",
                        browser.label()
                    )),
                    t,
                )
                .on_click(cx.listener(|this, _, _, cx| this.edit_add_form(cx, |f| f.own_profile = true))),
            )
            .child(
                choice(
                    "ext-fresh-profile",
                    !form.own_profile,
                    "A fresh browser".into(),
                    Some("Opens a separate window with no saved logins. Nothing to install.".into()),
                    t,
                )
                .on_click(cx.listener(|this, _, _, cx| this.edit_add_form(cx, |f| f.own_profile = false))),
            );
        let extension_step = form.own_profile.then(|| {
            let other_store = (browser == Browser::Edge).then_some(" Edge asks to allow extensions from other stores.");
            step(
                format!("Install the Playwright extension in {}.{}", browser.label(), other_store.unwrap_or("")),
                outline_button("ext-store", "Open store page", cx)
                    .on_click(|_, _, cx| cx.open_url(BROWSER_EXTENSION_URL)),
                t,
            )
        });
        let node_step = (!form.npx).then(|| {
            step(
                "Node.js isn't on your PATH; the browser server runs with npx. Install it, then restart Kynex.".into(),
                outline_button("ext-node", "Open nodejs.org", cx).on_click(|_, _, cx| cx.open_url(NODE_URL)),
                t,
            )
            .text_color(t.warning)
        });
        let configured = self.extensions.as_ref().and_then(|v| v.config.as_ref());
        let exists = configured.is_some_and(|c| c.servers.iter().any(|s| s.name == preset.name));
        let add_browser = if exists {
            div()
                .text_xs()
                .text_color(t.text_muted)
                .child(format!("{} is already set up; remove it below to set it up again.", preset.name))
                .into_any_element()
        } else {
            primary_button("ext-add-browser", format!("Add {}", browser.label()), cx)
                .when(busy, |b| b.opacity(0.5))
                .tooltip(Tooltip::with_meta(
                    format!("Adds {} for every folder", preset.name),
                    preset.transport.detail(),
                ))
                .on_click(cx.listener(|this, _, _, cx| this.add_browser(cx)))
                .into_any_element()
        };

        let scopes = div().flex().flex_col().children(Scope::ALL.into_iter().enumerate().map(|(n, s)| {
            choice(("ext-scope", n), s == form.scope, s.label().into(), None, t)
                .on_click(cx.listener(move |this, _, _, cx| this.edit_add_form(cx, |f| f.scope = s)))
        }));
        let add_custom = primary_button("ext-add-custom", "Add", cx)
            .when(busy, |b| b.opacity(0.5))
            .on_click(cx.listener(|this, _, _, cx| this.add_custom_server(cx)));

        div()
            .flex()
            .flex_col()
            .gap_4()
            .p_3()
            .rounded_md()
            .border_1()
            .border_color(t.border)
            .bg(t.panel)
            .child(
                div()
                    .flex()
                    .items_center()
                    .child(div().flex_1().text_sm().font_weight(FontWeight::SEMIBOLD).child("Add a server"))
                    .child(outline_button("ext-add-cancel", "Cancel", cx).on_click(cx.listener(|this, _, _, cx| {
                        if let Some(v) = &mut this.extensions {
                            v.adding = None;
                            v.error = None;
                        }
                        cx.notify();
                    }))),
            )
            .child(
                section("Browser", "Lets Claude open pages, click and fill in forms, for every folder.", t)
                    .child(browsers)
                    .child(profiles)
                    .children(extension_step)
                    .children(node_step)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(add_browser)
                            .when(!exists, |d| d.child(chip(preset.name.clone(), t).text_xs().font_family(mono()))),
                    ),
            )
            .child(
                section("Your own", "A command Claude Code runs, or the URL of an HTTP server.", t)
                    .child(field("Name", form.name.clone(), t))
                    .child(field("Command or URL", form.what.clone(), t))
                    .child(scopes)
                    .child(div().flex().child(add_custom)),
            )
            .into_any_element()
    }
}

fn section(title: &'static str, help: &'static str, t: &Palette) -> gpui::Div {
    div().flex().flex_col().gap_2().child(
        div()
            .flex()
            .flex_col()
            .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(title))
            .child(div().text_xs().text_color(t.text_muted).child(help)),
    )
}

/// A radio choice with an optional line under it.
fn choice(
    id: impl Into<gpui::ElementId>,
    selected: bool,
    label: String,
    help: Option<String>,
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
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(div().text_sm().child(label))
                .children(help.map(|h| div().text_xs().text_color(t.text_muted).child(h))),
        )
}

/// Something to do first, with the button that does it.
fn step(text: String, button: impl IntoElement, t: &Palette) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .gap_3()
        .px_2()
        .text_xs()
        .text_color(t.text_muted)
        .child(div().flex_1().min_w_0().child(text))
        .child(button)
}

fn field(label: &'static str, input: Entity<TextInput>, t: &Palette) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(div().text_xs().text_color(t.text_muted).child(label))
        .child(div().px_2().py_1().rounded_md().border_1().border_color(t.border).bg(t.editor).text_sm().child(input))
}
