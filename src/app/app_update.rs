//! Kynex updating itself: a check against the GitHub releases at startup and every
//! few hours (or on demand from the Kynex menu), a small toast when a new version
//! turns up, then download, swap and restart from the About modal
//! (`kynex_core::update`). Tabs and sessions come back after the restart.

use std::time::{Duration, SystemTime};

use gpui::{Context, prelude::*};
use kynex_core::cli::UpdateState;
use kynex_core::toasts::{Action, Toast};
use kynex_core::update;

use super::KynexApp;
use super::setup::CLI_CHECK_EVERY;

pub(super) const VERSION: &str = env!("CARGO_PKG_VERSION");

impl KynexApp {
    /// Look for a newer Kynex now and every few hours after. Development builds
    /// don't replace themselves, so they only check when asked.
    pub(super) fn watch_app_updates(cx: &mut Context<Self>) {
        update::clean_up();
        if update::installed_exe().is_err() {
            return;
        }
        cx.spawn(async move |this, cx| {
            loop {
                if this.update(cx, |this, cx| this.check_for_update(cx)).is_err() {
                    break;
                }
                cx.background_executor().timer(CLI_CHECK_EVERY).await;
            }
        })
        .detach();
    }

    /// Asks GitHub for the newest release now.
    pub(super) fn check_for_update(&mut self, cx: &mut Context<Self>) {
        if self.update_checking || matches!(self.app_update, UpdateState::Updating { .. } | UpdateState::Updated { .. })
        {
            return;
        }
        self.update_checking = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let found = cx.background_spawn(async { update::check(VERSION) }).await;
            this.update(cx, |this, cx| this.app_checked(found, cx)).ok();
        })
        .detach();
    }

    fn app_checked(&mut self, found: Result<Option<String>, String>, cx: &mut Context<Self>) {
        self.update_checking = false;
        self.update_checked = Some(SystemTime::now());
        if matches!(self.app_update, UpdateState::Updating { .. } | UpdateState::Updated { .. }) {
            return;
        }
        match found {
            Ok(Some(latest)) => {
                // Toast once per version found, not on every periodic check; the
                // title bar's Update pill carries it after that.
                let new = !matches!(&self.app_update, UpdateState::Available { latest: l, .. } if *l == latest);
                if new && !self.about_open {
                    let toast = Toast::info(format!("Kynex {latest} is available"));
                    self.toast(toast.action(Action::ViewUpdate).lasting(Duration::from_secs(8)));
                }
                self.app_update = UpdateState::Available { installed: VERSION.to_string(), latest };
            }
            Ok(None) => self.app_update = UpdateState::Idle,
            Err(e) => self.app_update = UpdateState::Failed(e),
        }
        cx.notify();
    }

    /// A newer version is waiting to be installed or restarted into.
    pub(super) fn update_waiting(&self) -> bool {
        matches!(self.app_update, UpdateState::Available { .. } | UpdateState::Updated { .. })
    }

    /// Downloads and swaps in the new version; Kynex keeps running the old one
    /// until you restart.
    pub(super) fn update_app(&mut self, cx: &mut Context<Self>) {
        let UpdateState::Available { latest, .. } = &self.app_update else { return };
        let latest = latest.clone();
        self.app_update = UpdateState::Updating { latest: latest.clone() };
        self.close_update_toast();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let version = latest.clone();
            let result = cx.background_spawn(async move { update::install(&version) }).await;
            this.update(cx, |this, cx| {
                this.app_update = match result {
                    Ok(()) => UpdateState::Updated { version: latest },
                    Err(e) => UpdateState::Failed(format!("Couldn't update: {e}")),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn restart_app(&mut self, cx: &mut Context<Self>) {
        self.save();
        match update::relaunch() {
            Ok(()) => cx.quit(),
            Err(e) => {
                self.app_update = UpdateState::Failed(e);
                cx.notify();
            }
        }
    }

    /// The "new version" toast has said its piece: About is open, or it's installing.
    pub(super) fn close_update_toast(&mut self) {
        self.toasts.dismiss_where(|t| t.action == Some(Action::ViewUpdate));
    }
}
