//! Settings → Providers → Antigravity: native `agy` CLI status and install.
//! Available both natively (with `agy`'s own sign-in) and via OpenRouter override.

use gpui::{Context, div, prelude::*};
use kynex_core::antigravity::setup::{self, Status};

use super::KynexApp;
use super::app_settings::heading;
use crate::theme::Palette;
use crate::ui::primary_button;

#[derive(Clone, Default)]
pub struct AntigravityView {
    status: Option<Status>,
    busy: Option<&'static str>,
    error: Option<String>,
}

impl KynexApp {
    fn antigravity_view(&mut self) -> Option<&mut AntigravityView> {
        self.app_settings.as_mut().map(|s| &mut s.providers.antigravity)
    }

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
                this.refresh_provider_catalog(cx);
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
            "Google's native `agy` CLI. Runs natively or through your OpenRouter key via Antigravity (OpenRouter).",
            t,
        ));
        let (line, color) = match &a.status {
            None => ("Checking…".to_string(), t.text_placeholder),
            Some(s @ Status::Ready(_)) => (s.describe(), t.text_muted),
            Some(s) => (s.describe(), t.warning),
        };
        let idle = a.busy.is_none();
        let missing = a.status == Some(Status::Missing);
        let update = self.cli_update_control(super::setup::ManagedCli::Antigravity, t, cx);
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
            .when((idle && missing) || update.is_some(), |d| {
                d.child(div().flex().flex_wrap().items_center().gap_2().children(update).when(idle && missing, |d| {
                    d.child(
                        primary_button("antigravity-install", "Install Antigravity", cx)
                            .on_click(cx.listener(|this, _, _, cx| this.install_antigravity(cx))),
                    )
                }))
            })
            .children(a.busy.map(|b| div().text_xs().text_color(t.text_placeholder).child(b)))
            .children(a.error.clone().map(|e| div().text_xs().text_color(t.error).child(e)))
    }
}
