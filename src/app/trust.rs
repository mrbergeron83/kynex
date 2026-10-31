//! "Trust this folder?": Claude Code ignores a folder's own settings and
//! `.mcp.json` until you accept its trust dialog, which Kynex users never see.
//! When the project's folder has such files and isn't trusted, a strip above
//! the composer asks, and Trust records it as the CLI's dialog would
//! (`kynex_core::trust`).

use std::path::PathBuf;

use gpui::{AnyElement, Context, div, prelude::*};
use kynex_core::toasts::Toast;

use super::KynexApp;
use super::setup::update_strip;
use crate::theme::theme;
use crate::ui::{Icon, Tooltip, ghost_button, icon, outline_button};

/// What Kynex knows about a folder's trust, for this run.
#[derive(Default)]
pub(super) struct Trust {
    /// Checked folders, with the files the CLI would ignore (empty: nothing to ask).
    checked: std::collections::HashMap<PathBuf, Vec<&'static str>>,
    /// Folders you said "Not now" to.
    dismissed: std::collections::HashSet<PathBuf>,
}

impl KynexApp {
    /// The strip asking to trust the project's folder, once it's known to need it.
    pub(super) fn render_trust(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dir = self.active_project()?.path.clone();
        let Some(files) = self.trust.checked.get(&dir) else {
            self.check_trust(dir, cx);
            return None;
        };
        if files.is_empty() || self.trust.dismissed.contains(&dir) {
            return None;
        }
        let t = theme(cx).clone();
        let text = format!("Claude Code ignores this folder's {} until you trust it", files.join(", "));
        let (trust_dir, later_dir) = (dir.clone(), dir);
        let actions = div()
            .flex()
            .items_center()
            .gap_1()
            .child(ghost_button("trust-later", cx).text_xs().text_color(t.text_muted).child("Not now").on_click(
                cx.listener(move |this, _, _, cx| {
                    this.trust.dismissed.insert(later_dir.clone());
                    cx.notify();
                }),
            ))
            .child(
                outline_button("trust-folder", "Trust folder", cx)
                    .tooltip(Tooltip::text(
                        "Only if you trust its files: its settings and MCP servers can run commands",
                    ))
                    .on_click(cx.listener(move |this, _, _, cx| this.trust_folder(trust_dir.clone(), cx))),
            )
            .into_any_element();
        Some(update_strip(icon(Icon::Shield, t.warning).size_3p5().into_any_element(), text, t.text, actions, &t))
    }

    /// Asks, off the UI thread, whether the CLI would ignore anything in `dir`.
    fn check_trust(&mut self, dir: PathBuf, cx: &mut Context<Self>) {
        // Marked checked now so a render doesn't start it again.
        self.trust.checked.insert(dir.clone(), Vec::new());
        cx.spawn(async move |this, cx| {
            let probe = dir.clone();
            let files = cx.background_spawn(async move { kynex_core::trust::needs_trust(&probe) }).await;
            this.update(cx, |this, cx| {
                this.trust.checked.insert(dir, files);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Trusts `dir` as the CLI's dialog would; idle sessions there restart to pick it up.
    fn trust_folder(&mut self, dir: PathBuf, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let target = dir.clone();
            let done = cx.background_spawn(async move { kynex_core::trust::trust(&target) }).await;
            this.update(cx, |this, cx| {
                match done {
                    Ok(()) => {
                        this.trust.checked.insert(dir.clone(), Vec::new());
                        for s in this.all_sessions_mut().filter(|s| s.cwd == dir) {
                            s.stale = true;
                        }
                        this.restart_idle();
                        this.toast(Toast::success(
                            "Folder trusted: its settings apply from each session's next message",
                        ));
                    }
                    Err(e) => {
                        this.toast(Toast::error(e));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}
