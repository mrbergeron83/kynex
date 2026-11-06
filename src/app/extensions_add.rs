//! The Extensions modal's Add server panel: a server of your own, added
//! through `claude mcp add`. The browser is a setting (Settings → General).

use gpui::{AnyElement, Context, Entity, FontWeight, div, prelude::*};
use kynex_core::mcp_config::{self, NewServer, Scope};

use super::KynexApp;
use super::extensions::AddForm;
use super::settings::radio;
use crate::theme::Palette;
use crate::ui::input::TextInput;
use crate::ui::{outline_button, primary_button};

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

fn field(label: &'static str, input: Entity<TextInput>, t: &Palette) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(div().text_xs().text_color(t.text_muted).child(label))
        .child(div().px_2().py_1().rounded_md().border_1().border_color(t.border).bg(t.editor).text_sm().child(input))
}
