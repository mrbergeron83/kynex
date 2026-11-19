//! Settings → Providers → Antigravity: opting in to Google's `agy` agent as a provider,
//! and installing it without a terminal. It runs on the saved OpenRouter key, so there's
//! no sign-in. Off until turned on; turned off, Antigravity leaves the provider picker.

use gpui::{Context, div, prelude::*};
use kynex_core::antigravity::setup::{self, Status};

use super::KynexApp;
use super::settings::heading;
use crate::theme::Palette;
use crate::ui::{Tooltip, outline_button, primary_button};

/// The Antigravity part of the Providers section.
#[derive(Clone, Default)]
pub struct AntigravityView {
    /// Where Antigravity stands, once looked up.
    status: Option<Status>,
    /// What's underway: "Installing Antigravity…".
    busy: Option<&'static str>,
    error: Option<String>,
}

impl KynexApp {
    fn antigravity_view(&mut self) -> Option<&mut AntigravityView> {
        self.app_settings.as_mut().map(|s| &mut s.providers.antigravity)
    }

    /// Looks up where Antigravity stands, off the UI thread.
    pub(super) fn load_antigravity_status(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let status = cx.background_spawn(async { setup::status() }).await;
            this.update(cx, |this, cx| {
                if let Some(a) = this.antigravity_view() {
                    a.status = Some(status);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Offers Antigravity for new sessions, or stops offering it.
    fn set_antigravity_enabled(&mut self, on: bool, cx: &mut Context<Self>) {
        self.saved.antigravity = on;
        self.save();
        self.refresh_new_sessions();
        if on {
            self.load_antigravity_status(cx);
        }
        cx.notify();
    }

    /// Installs `agy` off the UI thread with a label showing, then looks it up again.
    fn install_antigravity(&mut self, cx: &mut Context<Self>) {
        let Some(a) = self.antigravity_view().filter(|a| a.busy.is_none()) else { return };
        a.busy = Some("Installing Antigravity… (a minute or so)");
        a.error = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async {
                    let result = setup::install();
                    kynex_core::antigravity::recheck();
                    result
                })
                .await;
            this.update(cx, |this, cx| {
                if let Some(a) = this.antigravity_view() {
                    a.busy = None;
                    a.error = result.err();
                }
                this.load_antigravity_status(cx);
                // Installed now: the picker can offer it.
                this.refresh_new_sessions();
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn render_antigravity(&self, t: &Palette, cx: &mut Context<Self>) -> gpui::Div {
        let Some(a) = self.app_settings.as_ref().map(|s| &s.providers.antigravity) else { return div() };
        let section = div().flex().flex_col().gap_2().child(heading(
            "Antigravity",
            "Opt in to run sessions on Google's Antigravity agent, with its tools, on your OpenRouter key: Gemini \
             works without a Google subscription. It's a separate program; Kynex can install it for you.",
            t,
        ));
        if !self.saved.antigravity {
            return section.child(
                div().flex().child(
                    primary_button("antigravity-enable", "Use Antigravity", cx)
                        .on_click(cx.listener(|this, _, _, cx| this.set_antigravity_enabled(true, cx))),
                ),
            );
        }
        let (line, color) = match &a.status {
            None => ("Checking…".to_string(), t.text_placeholder),
            Some(s @ Status::Ready(_)) => (s.describe(), t.text_muted),
            Some(s) => (s.describe(), t.warning),
        };
        let idle = a.busy.is_none();
        let missing = a.status == Some(Status::Missing);
        let no_key = self.openrouter_key.is_none();
        let buttons = div()
            .flex()
            .flex_wrap()
            .gap_2()
            .when(idle && missing, |d| {
                d.child(
                    primary_button("antigravity-install", "Install Antigravity", cx)
                        .on_click(cx.listener(|this, _, _, cx| this.install_antigravity(cx))),
                )
            })
            .when(idle, |d| {
                d.child(
                    outline_button("antigravity-disable", "Stop offering Antigravity", cx)
                        .tooltip(Tooltip::text("New sessions can't pick Antigravity; it stays installed"))
                        .on_click(cx.listener(|this, _, _, cx| this.set_antigravity_enabled(false, cx))),
                )
            });
        section
            .child(
                div()
                    .px_2()
                    .py_1p5()
                    .rounded_md()
                    .border_1()
                    .border_color(t.border_variant)
                    .bg(t.editor)
                    .text_sm()
                    .text_color(color)
                    .child(line),
            )
            .when(no_key, |d| {
                d.child(
                    div()
                        .text_xs()
                        .text_color(t.warning)
                        .child("Antigravity runs on your OpenRouter key: save one above first."),
                )
            })
            .child(buttons)
            .children(a.busy.map(|b| div().text_xs().text_color(t.text_placeholder).child(b)))
            .children(a.error.clone().map(|e| div().text_xs().text_color(t.error).child(e)))
    }
}
