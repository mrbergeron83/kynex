//! Themes from JSON files in the data dir's `themes/` folder (`kynex_core::themes`): read
//! when Kynex starts and again whenever a file changes. A file that can't be used is reported
//! once and left out; if it was the theme in use, the built-in one is drawn until it is fixed.

use gpui::{AppContext, Context};
use kynex_core::themes::{self, Loaded};
use kynex_core::toasts::Toast;

use super::KynexApp;
use crate::theme;

impl KynexApp {
    /// Reads the themes now, and again after every change to the folder. The watch lives as long
    /// as the returned handle; `None` if the folder can't be watched (the themes still load once).
    pub(super) fn watch_themes(cx: &mut Context<Self>) -> Option<kynex_core::watch::Watch> {
        let (tx, rx) = async_channel::bounded::<()>(1);
        let watch = match themes::watch(move || {
            let _ = tx.try_send(());
        }) {
            Ok(w) => Some(w),
            Err(e) => {
                eprintln!("kynex: {e}");
                None
            }
        };
        cx.spawn(async move |this, cx| {
            // Once at the start, then on each change.
            loop {
                let dir = themes::dir();
                let loaded = cx.background_spawn(async move { themes::load_all(&dir) }).await;
                if this.update(cx, |this, cx| this.themes_loaded(loaded, cx)).is_err() {
                    break;
                }
                if rx.recv().await.is_err() {
                    break;
                }
            }
        })
        .detach();
        watch
    }

    /// New files are known: report the ones that don't work (once each), and draw the theme in use.
    fn themes_loaded(&mut self, loaded: Loaded, cx: &mut Context<Self>) {
        let new: Vec<&String> = loaded.errors.iter().filter(|e| !self.theme_errors.contains(e)).collect();
        for e in new {
            self.toast(Toast::error(format!("Theme not used: {e}")));
        }
        self.custom_themes = loaded.themes;
        self.theme_errors = loaded.errors;
        if self.saved.custom_theme.is_some() {
            self.apply_theme(cx);
        }
        cx.notify();
    }

    /// Whether a theme file is what is drawn (the saved one reads), so the OS's light / dark doesn't apply.
    pub(super) fn custom_theme_applied(&self) -> bool {
        self.saved.custom_theme.as_ref().is_some_and(|id| self.custom_themes.iter().any(|c| &c.id == id))
    }

    /// Draws the saved theme: the file's if it reads, else the built-in one.
    fn apply_theme(&mut self, cx: &mut Context<Self>) {
        let custom = self.saved.custom_theme.as_ref().and_then(|id| self.custom_themes.iter().find(|c| &c.id == id));
        match custom {
            Some(c) => theme::set_custom(c, cx),
            None => theme::set(self.saved.theme, cx),
        }
        cx.refresh_windows();
    }

    pub(super) fn set_custom_theme(&mut self, id: String, cx: &mut Context<Self>) {
        self.saved.custom_theme = Some(id);
        self.save();
        self.apply_theme(cx);
        cx.notify();
    }
}
