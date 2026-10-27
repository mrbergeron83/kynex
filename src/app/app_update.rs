//! Kynex updating itself: a check against the GitHub releases at startup and every
//! few hours, an offer in a strip above the composer, then download, swap and
//! restart (`kynex_core::update`). Tabs and sessions come back after the restart.

use gpui::{AnyElement, Context, div, prelude::*, px};
use kynex_core::cli::UpdateState;
use kynex_core::update;

use super::KynexApp;
use super::setup::{CLI_CHECK_EVERY, update_strip};
use crate::theme::theme;
use crate::ui::{Icon, Tooltip, ghost_button, icon, icon_button, outline_button, primary_button, spinner};

const VERSION: &str = env!("CARGO_PKG_VERSION");

impl KynexApp {
    /// Look for a newer Kynex now and every few hours after. Development builds
    /// don't replace themselves, so they don't look.
    pub(super) fn watch_app_updates(cx: &mut Context<Self>) {
        update::clean_up();
        if update::installed_exe().is_err() {
            return;
        }
        cx.spawn(async move |this, cx| {
            loop {
                let found = cx.background_spawn(async { update::check(VERSION) }).await;
                if this.update(cx, |this, cx| this.app_checked(found, cx)).is_err() {
                    break;
                }
                cx.background_executor().timer(CLI_CHECK_EVERY).await;
            }
        })
        .detach();
    }

    fn app_checked(&mut self, found: Result<Option<String>, String>, cx: &mut Context<Self>) {
        if !matches!(self.app_update, UpdateState::Idle | UpdateState::Available { .. }) {
            return;
        }
        match found {
            Ok(Some(latest)) if self.saved.skipped_app_version.as_deref() != Some(latest.as_str()) => {
                self.app_update = UpdateState::Available { installed: VERSION.to_string(), latest };
            }
            Ok(_) => {}
            Err(e) => eprintln!("kynex: couldn't check for Kynex updates: {e}"),
        }
        cx.notify();
    }

    /// Downloads and swaps in the new version; Kynex keeps running the old one
    /// until you restart.
    fn update_app(&mut self, cx: &mut Context<Self>) {
        let UpdateState::Available { latest, .. } = &self.app_update else { return };
        let latest = latest.clone();
        self.app_update = UpdateState::Updating { latest: latest.clone() };
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn({
                let latest = latest.clone();
                async move { update::install(&latest) }
            });
            let result = result.await;
            this.update(cx, |this, cx| {
                this.app_update = match result {
                    Ok(()) => UpdateState::Updated { version: latest },
                    Err(e) => UpdateState::Failed(format!("Couldn't update Kynex: {e}")),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn restart_app(&mut self, cx: &mut Context<Self>) {
        self.save();
        match update::relaunch() {
            Ok(()) => cx.quit(),
            Err(e) => {
                self.app_update = UpdateState::Failed(e);
                cx.notify();
            }
        }
    }

    fn skip_app_version(&mut self, cx: &mut Context<Self>) {
        if let UpdateState::Available { latest, .. } = &self.app_update {
            self.saved.skipped_app_version = Some(latest.clone());
            self.save();
        }
        self.app_update = UpdateState::Idle;
        cx.notify();
    }

    /// The Kynex update strip above the composer.
    pub(super) fn render_app_update(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let t = theme(cx).clone();
        let (lead, text, color): (AnyElement, String, _) = match &self.app_update {
            UpdateState::Idle => return None,
            UpdateState::Available { installed, latest } => (
                icon(Icon::ArrowUp, t.info).size_3p5().into_any_element(),
                format!("Kynex {latest} is available (you have {installed})"),
                t.text,
            ),
            UpdateState::Updating { latest } => (
                spinner("app-update-spin", t.accent, px(14.)).into_any_element(),
                format!("Downloading Kynex {latest}…"),
                t.text_muted,
            ),
            UpdateState::Updated { version } => (
                icon(Icon::Check, t.success).size_3p5().into_any_element(),
                format!("Kynex {version} is installed. Restart to use it; your tabs and sessions come back."),
                t.text,
            ),
            UpdateState::Failed(e) => (icon(Icon::Dot, t.error).size_3p5().into_any_element(), e.clone(), t.text),
        };
        let actions = match &self.app_update {
            UpdateState::Available { .. } => div()
                .flex()
                .items_center()
                .gap_1()
                .child(
                    ghost_button("app-skip", cx)
                        .text_xs()
                        .text_color(t.text_muted)
                        .child("Skip this version")
                        .on_click(cx.listener(|this, _, _, cx| this.skip_app_version(cx))),
                )
                .child(
                    outline_button("app-update", "Update", cx)
                        .tooltip(Tooltip::text("Downloads the new version; Kynex restarts when you say so"))
                        .on_click(cx.listener(|this, _, _, cx| this.update_app(cx))),
                )
                .into_any_element(),
            UpdateState::Updating { .. } => div().into_any_element(),
            UpdateState::Updated { .. } => div()
                .flex()
                .items_center()
                .gap_1()
                .child(ghost_button("app-later", cx).text_xs().text_color(t.text_muted).child("Later").on_click(
                    cx.listener(|this, _, _, cx| {
                        this.app_update = UpdateState::Idle;
                        cx.notify();
                    }),
                ))
                .child(
                    primary_button("app-restart", "Restart", cx)
                        .tooltip(Tooltip::text("Running turns stop; sessions resume after the restart"))
                        .on_click(cx.listener(|this, _, _, cx| this.restart_app(cx))),
                )
                .into_any_element(),
            UpdateState::Idle | UpdateState::Failed(_) => icon_button("app-update-dismiss", Icon::Close, cx)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.app_update = UpdateState::Idle;
                    cx.notify();
                }))
                .into_any_element(),
        };
        Some(update_strip(lead, text, color, actions, &t))
    }
}
