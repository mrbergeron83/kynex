//! Getting the `claude` CLI ready without a terminal: the install / sign-in
//! screen that takes the workspace's place, and the update strip above the composer.

use std::time::Duration;

use gpui::{AnyElement, Context, FontWeight, Hsla, div, prelude::*, px};
use kynex_core::cli::{self, UpdateState};

use super::{KynexApp, models};
use crate::theme::{Palette, theme};
use crate::ui::{Icon, Tooltip, ghost_button, icon, icon_button, outline_button, primary_button, spinner};

pub(super) const CLI_CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);

/// The last Claude Code version check: running now, when, and why it failed.
#[derive(Default)]
pub(super) struct CliCheck {
    pub checking: bool,
    pub at: Option<std::time::SystemTime>,
    pub error: Option<String>,
}

/// Getting the `claude` CLI ready to run sessions, without leaving Kynex.
pub enum Setup {
    Checking,
    Ready,
    Missing { error: Option<String> },
    Installing,
    LoggedOut { error: Option<String> },
    SigningIn(cli::Login),
}

impl KynexApp {
    /// Look for a newer Claude Code now and every few hours after.
    pub(super) fn watch_cli_updates(cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                let found = cx.background_spawn(async { cli::check() }).await;
                if this.update(cx, |this, cx| this.cli_checked(found, false, cx)).is_err() {
                    break;
                }
                cx.background_executor().timer(CLI_CHECK_EVERY).await;
            }
        })
        .detach();
    }

    /// Checks for a newer Claude Code right now, from the About modal or the Kynex
    /// menu. Offers it even if the user skipped that version before.
    pub(super) fn check_cli_now(&mut self, cx: &mut Context<Self>) {
        if self.cli_check.checking || matches!(self.cli_update, UpdateState::Updating { .. }) {
            return;
        }
        self.cli_check.checking = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let found = cx.background_spawn(async { cli::check() }).await;
            this.update(cx, |this, cx| this.cli_checked(found, true, cx)).ok();
        })
        .detach();
    }

    /// Takes in a version check. Never interrupts an update in progress; a
    /// background check stays quiet about a release the user skipped.
    fn cli_checked(&mut self, found: Result<(String, String), String>, manual: bool, cx: &mut Context<Self>) {
        if manual {
            self.cli_check.checking = false;
        }
        self.cli_check.at = Some(std::time::SystemTime::now());
        self.cli_check.error = found.as_ref().err().cloned();
        if let Ok((installed, _)) = &found {
            self.claude_version = Some(installed.clone());
        }
        if matches!(self.cli_update, UpdateState::Updating { .. }) {
            return cx.notify();
        }
        match found {
            Ok((installed, latest)) if cli::is_newer(&latest, &installed) => {
                if manual || self.saved.skipped_cli_version.as_deref() != Some(latest.as_str()) {
                    self.cli_update = UpdateState::Available { installed, latest };
                }
            }
            // Updated elsewhere (or by the CLI itself) since the last check.
            Ok(_) if matches!(self.cli_update, UpdateState::Available { .. }) => self.cli_update = UpdateState::Idle,
            Ok(_) => {}
            Err(e) => eprintln!("kynex: couldn't check for Claude Code updates: {e}"),
        }
        cx.notify();
    }

    /// Installs the newest Claude Code with `claude update`. Idle sessions stop
    /// first so their binary isn't in use, and resume on the new one with the next
    /// message; busy ones restart once they finish.
    pub(super) fn update_cli(&mut self, cx: &mut Context<Self>) {
        let UpdateState::Available { latest, .. } = &self.cli_update else { return };
        self.cli_update = UpdateState::Updating { latest: latest.clone() };
        for s in self.all_sessions_mut().filter(|s| !s.busy) {
            s.kill();
        }
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async { cli::update() }).await;
            this.update(cx, |this, cx| {
                this.cli_update = match result {
                    Ok(version) => {
                        this.claude_version = Some(version.clone());
                        let busy = this.code.iter().filter(|(_, l)| l.busy()).map(|(p, _)| p.clone());
                        this.restart_when_idle.extend(busy.collect::<Vec<_>>());
                        UpdateState::Updated { version }
                    }
                    Err(e) => UpdateState::Failed(e),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn skip_cli_version(&mut self, cx: &mut Context<Self>) {
        if let UpdateState::Available { latest, .. } = &self.cli_update {
            self.saved.skipped_cli_version = Some(latest.clone());
            self.save();
        }
        self.cli_update = UpdateState::Idle;
        cx.notify();
    }

    /// Asks the CLI whether it's installed and signed in, and shows the setup
    /// screen if it isn't.
    pub(super) fn check_setup(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let status = cx.background_spawn(async { cli::status() }).await;
            this.update(cx, |this, cx| {
                this.setup = match status {
                    cli::Status::Ready => {
                        models::retry_fetch();
                        Setup::Ready
                    }
                    cli::Status::Missing => Setup::Missing { error: None },
                    cli::Status::LoggedOut => Setup::LoggedOut { error: None },
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn install_cli(&mut self, cx: &mut Context<Self>) {
        self.setup = Setup::Installing;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async { cli::install() }).await;
            this.update(cx, |this, cx| match result {
                Ok(()) => this.check_setup(cx),
                Err(e) => {
                    this.setup = Setup::Missing { error: Some(e) };
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Starts `claude auth login` (which opens the browser) and watches it until
    /// it finishes or the user cancels.
    fn sign_in(&mut self, console: bool, cx: &mut Context<Self>) {
        match cli::Login::start(console) {
            Ok(login) => self.setup = Setup::SigningIn(login),
            Err(e) => self.setup = Setup::LoggedOut { error: Some(e) },
        }
        self.login_code.update(cx, |input, cx| input.set_text("", cx));
        cx.notify();
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(300)).await;
                let done = this.update(cx, |this, cx| {
                    let Setup::SigningIn(login) = &mut this.setup else { return true };
                    // Repaint while waiting, so the sign-in URL shows once printed.
                    cx.notify();
                    match login.finished() {
                        None => false,
                        Some(Ok(())) => {
                            this.check_setup(cx);
                            true
                        }
                        Some(Err(e)) => {
                            this.setup = Setup::LoggedOut { error: Some(e) };
                            true
                        }
                    }
                });
                if done.unwrap_or(true) {
                    break;
                }
            }
        })
        .detach();
    }

    pub(super) fn submit_login_code(&mut self, cx: &mut Context<Self>) {
        let code = self.login_code.read(cx).text().trim().to_string();
        let Setup::SigningIn(login) = &mut self.setup else { return };
        if code.is_empty() {
            return;
        }
        if let Err(e) = login.send_code(&code) {
            self.setup = Setup::LoggedOut { error: Some(e) };
        }
        self.login_code.update(cx, |input, cx| input.set_text("", cx));
        cx.notify();
    }

    /// Installing Claude Code or signing in, in the workspace's place, until the CLI is ready.
    pub(super) fn render_setup(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let t = theme(cx).clone();
        let busy = |label: &'static str| {
            div()
                .flex()
                .items_center()
                .gap_2()
                .text_sm()
                .text_color(t.text_muted)
                .child(spinner("setup-spin", t.accent, px(14.)))
                .child(label)
        };
        let error = |e: &Option<String>| e.clone().map(|e| div().text_xs().text_color(t.error).child(e));
        let (title, body, content): (&str, &str, AnyElement) = match &self.setup {
            Setup::Checking | Setup::Ready => return None,
            Setup::Missing { error: e } => (
                "Set up Claude Code",
                "Kynex runs on Claude Code, which isn't installed on this computer yet. Kynex can install it for you; it takes about a minute.",
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .children(error(e))
                    .child(
                        div().flex().child(
                            primary_button("setup-install", "Install Claude Code", cx)
                                .on_click(cx.listener(|this, _, _, cx| this.install_cli(cx))),
                        ),
                    )
                    .into_any_element(),
            ),
            Setup::Installing => (
                "Set up Claude Code",
                "Downloading and installing the latest Claude Code.",
                busy("Installing Claude Code…").into_any_element(),
            ),
            Setup::LoggedOut { error: e } => (
                "Sign in to Claude",
                "Sign in with your Claude account (Pro, Max, Team or Enterprise). Your browser opens to finish.",
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .children(error(e))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                primary_button("setup-sign-in", "Sign in", cx)
                                    .on_click(cx.listener(|this, _, _, cx| this.sign_in(false, cx))),
                            )
                            .child(
                                ghost_button("setup-console", cx)
                                    .text_color(t.text_muted)
                                    .child("Use an Anthropic Console account")
                                    .tooltip(Tooltip::text("Pay per use with API billing instead of a subscription"))
                                    .on_click(cx.listener(|this, _, _, cx| this.sign_in(true, cx))),
                            ),
                    )
                    .into_any_element(),
            ),
            Setup::SigningIn(login) => {
                let url = login.url();
                (
                    "Sign in to Claude",
                    "Finish signing in in your browser. Kynex continues on its own when you're done.",
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(busy("Waiting for the browser…"))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1p5()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(t.text_muted)
                                        .child("If the page shows a code, paste it here:"),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .child(
                                            div()
                                                .flex_1()
                                                .px_2()
                                                .py_1()
                                                .rounded_md()
                                                .border_1()
                                                .border_color(t.border)
                                                .bg(t.editor)
                                                .text_sm()
                                                .child(self.login_code.clone()),
                                        )
                                        .child(
                                            outline_button("setup-code", "Continue", cx)
                                                .on_click(cx.listener(|this, _, _, cx| this.submit_login_code(cx))),
                                        ),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .when_some(url, |d, url| {
                                    d.child(
                                        ghost_button("setup-open", cx)
                                            .text_color(t.text_muted)
                                            .child("Open the sign-in page again")
                                            .on_click(move |_, _, cx| cx.open_url(&url)),
                                    )
                                })
                                .child(
                                    ghost_button("setup-cancel", cx).text_color(t.text_muted).child("Cancel").on_click(
                                        cx.listener(|this, _, _, cx| {
                                            this.setup = Setup::LoggedOut { error: None };
                                            cx.notify();
                                        }),
                                    ),
                                ),
                        )
                        .into_any_element(),
                )
            }
        };
        Some(
            div()
                .flex()
                .flex_1()
                .min_h_0()
                .items_center()
                .justify_center()
                .bg(t.editor)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_4()
                        .w(px(460.))
                        .p_6()
                        .rounded_lg()
                        .border_1()
                        .border_color(t.border)
                        .bg(t.panel)
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1p5()
                                .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child(title))
                                .child(div().text_sm().text_color(t.text_muted).child(body)),
                        )
                        .child(content),
                )
                .into_any_element(),
        )
    }

    /// The Claude Code update strip above the composer: an offer, progress, or the outcome.
    pub(super) fn render_cli_update(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let t = theme(cx).clone();
        let (lead, text, color): (AnyElement, String, _) = match &self.cli_update {
            UpdateState::Idle => return None,
            UpdateState::Available { installed, latest } => (
                icon(Icon::ArrowUp, t.info).size_3p5().into_any_element(),
                format!("Claude Code {latest} is available (you have {installed})"),
                t.text,
            ),
            UpdateState::Updating { latest } => (
                spinner("cli-update-spin", t.accent, px(14.)).into_any_element(),
                format!("Updating Claude Code to {latest}…"),
                t.text_muted,
            ),
            UpdateState::Updated { version } => (
                icon(Icon::Check, t.success).size_3p5().into_any_element(),
                format!("Claude Code updated to {version}. Sessions pick it up with their next message."),
                t.text,
            ),
            UpdateState::Failed(e) => (icon(Icon::Dot, t.error).size_3p5().into_any_element(), e.clone(), t.text),
        };
        let dismiss = || {
            icon_button("cli-update-dismiss", Icon::Close, cx).on_click(cx.listener(|this, _, _, cx| {
                this.cli_update = UpdateState::Idle;
                cx.notify();
            }))
        };
        let actions = match &self.cli_update {
            UpdateState::Available { .. } => div()
                .flex()
                .items_center()
                .gap_1()
                .child(
                    ghost_button("cli-skip", cx)
                        .text_xs()
                        .text_color(t.text_muted)
                        .child("Skip this version")
                        .on_click(cx.listener(|this, _, _, cx| this.skip_cli_version(cx))),
                )
                .child(
                    outline_button("cli-update", "Update", cx)
                        .tooltip(Tooltip::text("Runs `claude update`; idle sessions restart on the new version"))
                        .on_click(cx.listener(|this, _, _, cx| this.update_cli(cx))),
                )
                .into_any_element(),
            UpdateState::Updating { .. } => div().into_any_element(),
            _ => dismiss().into_any_element(),
        };
        Some(update_strip(lead, text, color, actions, &t))
    }
}

/// A one-line strip above the composer: an update offer, its progress or its outcome.
pub(super) fn update_strip(
    lead: AnyElement,
    text: String,
    color: Hsla,
    actions: AnyElement,
    t: &Palette,
) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap_2()
        .pl_2()
        .pr_1()
        .py_1()
        .rounded_md()
        .border_1()
        .border_color(t.border_variant)
        .bg(t.panel)
        .child(lead)
        .child(div().flex_1().min_w_0().text_xs().text_color(color).child(text))
        .child(actions)
        .into_any_element()
}
