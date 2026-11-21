//! Undo / redo of changes to a workspace's panes, reopening a closed pane, and restoring snapshots.

use gpui::{Context, Window};
use kynex_core::snapshots::Snapshot;
use kynex_core::tiling::SavedLayout;
use kynex_core::toasts::Toast;

use super::KynexApp;

pub(super) fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

impl KynexApp {
    pub(super) fn layout_entries(&mut self) -> Vec<Snapshot> {
        let Some((project, cwd)) = self.tiling_scope() else { return Vec::new() };
        self.saved_workspaces(&project).history(&cwd).entries()
    }

    pub(super) fn undo_layout(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.change_layout("Nothing to undo", window, cx, |h| h.undo());
    }

    pub(super) fn redo_layout(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.change_layout("Nothing to redo", window, cx, |h| h.redo());
    }

    pub(super) fn reopen_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.change_layout("No closed pane to reopen", window, cx, |h| h.reopen_closed());
    }

    pub(super) fn restore_layout(&mut self, snapshot: &Snapshot, window: &mut Window, cx: &mut Context<Self>) {
        self.change_layout("That layout is gone", window, cx, |h| h.restore(snapshot));
    }

    fn change_layout(
        &mut self,
        none: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
        pick: impl FnOnce(&mut kynex_core::snapshots::History) -> Option<SavedLayout>,
    ) {
        let Some((project, cwd)) = self.tiling_scope() else { return };
        let Some(layout) = pick(self.saved_workspaces(&project).history(&cwd)) else {
            self.toast(Toast::info(none));
            return cx.notify();
        };
        self.tiles.remove(&cwd);
        self.saved_workspaces(&project).set_layout(&cwd, Some(layout));
        self.focus_input(window, cx);
        cx.notify();
    }
}
