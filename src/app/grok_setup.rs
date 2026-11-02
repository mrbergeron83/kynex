//! Settings → Providers → Grok: opting in to Grok Build as a provider, and
//! setting it up without a terminal: installing it, signing in in the browser,
//! signing out. Off until turned on; turned off, Grok leaves the provider picker.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gpui::{Context, div, prelude::*};
use kynex_core::cli::Login;
use kynex_core::grok::setup::{self, Status};

use super::KynexApp;
use super::settings::heading;
use crate::theme::Palette;
use crate::ui::{Tooltip, outline_button, primary_button};

/// The Grok part of the Providers section.
#[derive(Clone, Default)]
pub struct GrokView {
    /// Where Grok stands, once looked up.
    status: Option<Status>,
    /// A `grok login` underway.
    login: Option<Rc<RefCell<Login>>>,
    /// What's underway: "Installing Grok Build…".
    busy: Option<&'static str>,
    error: Option<String>,
}

impl KynexApp {
    fn grok_view(&mut self) -> Option<&mut GrokView> {
        self.app_settings.as_mut().map(|s| &mut s.providers.grok)
    }

    /// Looks up where Grok stands, off the UI thread.
    pub(super) fn load_grok_status(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let status = cx.background_spawn(async { setup::status() }).await;
            this.update(cx, |this, cx| {
                if let Some(g) = this.grok_view() {
                    g.status = Some(status);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Offers Grok for new sessions, or stops offering it.
    fn set_grok_enabled(&mut self, on: bool, cx: &mut Context<Self>) {
        self.saved.grok = on;
        self.save();
        self.refresh_new_sessions();
        if on {
            self.load_grok_status(cx);
        }
        cx.notify();
    }

    /// Runs `job` off the UI thread with `label` showing, then looks Grok up again.
    fn grok_job(
        &mut self,
        label: &'static str,
        job: impl FnOnce() -> Result<(), String> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        let Some(g) = self.grok_view().filter(|g| g.busy.is_none()) else { return };
        g.busy = Some(label);
        g.error = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { job() }).await;
            this.update(cx, |this, cx| {
                if let Some(g) = this.grok_view() {
                    g.busy = None;
                    g.error = result.err();
                }
                this.load_grok_status(cx);
                // Installed now: the picker can offer it.
                this.refresh_new_sessions();
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Starts `grok login` (it opens the browser) and watches it until it ends.
    fn grok_sign_in(&mut self, cx: &mut Context<Self>) {
        let login = match setup::login() {
            Ok(login) => Rc::new(RefCell::new(login)),
            Err(e) => {
                if let Some(g) = self.grok_view() {
                    g.error = Some(e);
                }
                return cx.notify();
            }
        };
        if let Some(g) = self.grok_view() {
            g.login = Some(login.clone());
            g.error = None;
        }
        cx.notify();
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(300)).await;
                let done = this.update(cx, |this, cx| {
                    // Settings closed, or cancelled: the login goes with it.
                    let Some(g) = this.grok_view().filter(|g| g.login.is_some()) else { return true };
                    cx.notify();
                    let Some(result) = login.borrow_mut().finished() else { return false };
                    g.login = None;
                    g.error = result.err();
                    this.load_grok_status(cx);
                    true
                });
                if done.unwrap_or(true) {
                    break;
                }
            }
        })
        .detach();
    }

    pub(super) fn render_grok(&self, t: &Palette, cx: &mut Context<Self>) -> gpui::Div {
        let Some(g) = self.app_settings.as_ref().map(|s| &s.providers.grok) else { return div() };
        let section = div().flex().flex_col().gap_2().child(heading(
            "Grok",
            "Opt in to run sessions on xAI's Grok Build, with its own sign-in, models and tools. It's a separate \
             program; Kynex can install it for you.",
            t,
        ));
        if !self.saved.grok {
            return section.child(
                div().flex().child(
                    primary_button("grok-enable", "Use Grok", cx)
                        .on_click(cx.listener(|this, _, _, cx| this.set_grok_enabled(true, cx))),
                ),
            );
        }
        let (line, color) = match (&g.status, &g.login) {
            (_, Some(login)) => (
                login.borrow().url().map_or("Opening the browser to sign in…".into(), |url| {
                    format!("Finish signing in in your browser. If it didn't open: {url}")
                }),
                t.text_muted,
            ),
            (None, _) => ("Checking…".to_string(), t.text_placeholder),
            (Some(s @ Status::Ready(_)), _) => (s.describe(), t.text_muted),
            (Some(s), _) => (s.describe(), t.warning),
        };
        let idle = g.busy.is_none() && g.login.is_none();
        let status = g.status.clone();
        let buttons = div()
            .flex()
            .flex_wrap()
            .gap_2()
            .when(idle && status == Some(Status::Missing), |d| {
                d.child(primary_button("grok-install", "Install Grok Build", cx).on_click(cx.listener(
                    |this, _, _, cx| this.grok_job("Installing Grok Build… (a minute or so)", setup::install, cx),
                )))
            })
            .when(idle && matches!(status, Some(Status::SignedOut(_))), |d| {
                d.child(
                    primary_button("grok-sign-in", "Sign in", cx)
                        .on_click(cx.listener(|this, _, _, cx| this.grok_sign_in(cx))),
                )
            })
            .when(idle && matches!(status, Some(Status::Ready(_))), |d| {
                d.child(
                    outline_button("grok-sign-out", "Sign out", cx)
                        .tooltip(Tooltip::text("Grok sessions stop working until you sign in again"))
                        .on_click(cx.listener(|this, _, _, cx| this.grok_job("Signing out…", setup::logout, cx))),
                )
            })
            .when(g.login.is_some(), |d| {
                d.child(outline_button("grok-sign-in-cancel", "Cancel", cx).on_click(cx.listener(|this, _, _, cx| {
                    if let Some(g) = this.grok_view() {
                        g.login = None;
                    }
                    cx.notify();
                })))
            })
            .when(idle, |d| {
                d.child(
                    outline_button("grok-disable", "Stop offering Grok", cx)
                        .tooltip(Tooltip::text("New sessions can't pick Grok; Grok Build stays installed"))
                        .on_click(cx.listener(|this, _, _, cx| this.set_grok_enabled(false, cx))),
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
            .child(buttons)
            .children(g.busy.map(|b| div().text_xs().text_color(t.text_placeholder).child(b)))
            .children(g.error.clone().map(|e| div().text_xs().text_color(t.error).child(e)))
    }
}
