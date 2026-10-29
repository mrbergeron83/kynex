//! Letting Claude's work carry on while you're away: closing the window hides
//! Kynex instead of quitting (`kynex_core::tray`: the notification area on
//! Windows, the Dock on macOS), the computer kept awake while sessions work
//! (`kynex_core::awake`), and the shown session's `claude` started before you
//! send, so sending doesn't wait on it.

use gpui::{App, Context, Window};
use kynex_core::awake::{Awake, KeepAwake};
use kynex_core::toasts::Toast;
use kynex_core::tray::{self, Haven, OnClose, Tray, TrayEvent};

use super::KynexApp;
use super::setup::Setup;

impl KynexApp {
    /// Closing the window hides Kynex when the setting and the OS allow it; the
    /// tray icon (Windows) brings it back or quits.
    pub(super) fn watch_close(window: &Window, cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        window.on_window_should_close(cx, move |_, cx| {
            let hide = this.update(cx, |this, cx| this.hide_to_background(cx)).unwrap_or(false);
            !hide
        });
    }

    /// Hides the window if closing it should keep Kynex running; says whether it did.
    fn hide_to_background(&mut self, cx: &mut Context<Self>) -> bool {
        let haven = Haven::of_os();
        if !self.saved.on_close.hides(haven) {
            return false;
        }
        let Some(haven) = haven else { return false };
        if haven == Haven::Tray && self.tray.is_none() {
            // No icon to come back from: close as usual.
            self.sync_tray(cx);
            if self.tray.is_none() {
                return false;
            }
        }
        self.stash_draft(cx);
        self.save();
        match haven {
            Haven::Tray => tray::hide_window(self.native_window),
            Haven::Dock => cx.hide(),
        }
        true
    }

    /// Adds the tray icon when closing hides to it, and removes it otherwise.
    pub(super) fn sync_tray(&mut self, cx: &mut Context<Self>) {
        let wanted = self.saved.on_close.hides(Haven::of_os()) && Haven::of_os() == Some(Haven::Tray);
        if wanted == self.tray.is_some() {
            return;
        }
        if !wanted {
            self.tray = None;
            return;
        }
        // Clicks arrive inside the OS's message loop; handle them on a task of their own.
        let (tx, rx) = async_channel::unbounded::<TrayEvent>();
        match Tray::new(
            "Kynex",
            Box::new(move |ev| {
                let _ = tx.try_send(ev);
            }),
        ) {
            Ok(t) => self.tray = Some(t),
            Err(e) => {
                self.toast(Toast::error(e));
                return;
            }
        }
        cx.spawn(async move |this, cx| {
            while let Ok(ev) = rx.recv().await {
                let handled = this.update(cx, |this, cx| match ev {
                    TrayEvent::Show => this.show_from_background(cx),
                    TrayEvent::Quit => cx.quit(),
                });
                if handled.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// Brings the hidden (or merely buried) window back to the front.
    pub(super) fn show_from_background(&mut self, cx: &mut Context<Self>) {
        tray::show_window(self.native_window);
        if let Some(handle) = cx.windows().first().copied() {
            cx.defer(move |cx: &mut App| {
                handle.update(cx, |_, window, _| window.activate_window()).ok();
            });
        }
        cx.activate(true);
    }

    pub(super) fn set_on_close(&mut self, on: OnClose, cx: &mut Context<Self>) {
        self.saved.on_close = on;
        self.save();
        self.sync_tray(cx);
        cx.notify();
    }

    /// Takes or lets go of the keep-awake hold when whether it's wanted changes.
    /// Only calls the OS on a change.
    pub(super) fn sync_awake(&mut self) {
        let working = self.all_sessions().any(|s| s.busy || s.has_tasks());
        let wanted = self.saved.keep_awake.wanted(working);
        // A failure is said once; it'd fail the same way on every frame.
        if wanted == self.awake.is_some() || (wanted && self.awake_failed) {
            return;
        }
        if !wanted {
            self.awake = None;
            return;
        }
        match Awake::hold("Claude is working in Kynex") {
            Ok(hold) => self.awake = Some(hold),
            Err(e) => {
                self.awake_failed = true;
                self.toast(Toast::error(e));
            }
        }
    }

    pub(super) fn set_keep_awake(&mut self, k: KeepAwake, cx: &mut Context<Self>) {
        self.saved.keep_awake = k;
        self.save();
        self.awake_failed = false;
        self.sync_awake();
        cx.notify();
    }

    /// Starts the shown session's process if it isn't running, as a message is
    /// likely soon: the composer focused or typed in, a session opened.
    pub(super) fn warm_shown(&mut self) {
        if !matches!(self.setup, Setup::Ready) {
            return;
        }
        let model = self.active_model();
        if let Some(s) = self.session_mut().filter(|s| !s.is_running()) {
            s.model = model;
            s.warm();
        }
    }
}
