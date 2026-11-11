//! Undo / redo of changes to a workspace's panes, reopening a closed pane, and bringing
//! back an older layout. The history itself is `kynex_core::snapshots`, kept with the
//! workspace; a layout is applied by dropping the live tiling and letting `sync_tiles`
//! restore the saved one, as it does after a restart (sessions reopen, terminals start anew).

use gpui::{Context, Window};
use kynex_core::snapshots::Snapshot;
use kynex_core::tiling::SavedLayout;
use kynex_core::toasts::Toast;

use super::KynexApp;

/// Seconds since the Unix epoch.
pub(super) fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

impl KynexApp {
    /// The older layouts of the shown workspace, newest first.
    pub(super) fn layout_entries(&mut self) -> Vec<Snapshot> {
        let Some((project, cwd, _)) = self.tiling_scope() else { return Vec::new() };
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

    /// Brings back `snapshot`, one of [`Self::layout_entries`].
    pub(super) fn restore_layout(&mut self, snapshot: &Snapshot, window: &mut Window, cx: &mut Context<Self>) {
        self.change_layout("That layout is gone", window, cx, |h| h.restore(snapshot));
    }

    /// Asks the history for a layout and applies it; says `none` if there isn't one.
    fn change_layout(
        &mut self,
        none: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
        pick: impl FnOnce(&mut kynex_core::snapshots::History) -> Option<SavedLayout>,
    ) {
        let Some((project, cwd, _)) = self.tiling_scope() else { return };
        let Some(layout) = pick(self.saved_workspaces(&project).history(&cwd)) else {
            self.toast(Toast::info(none));
            return cx.notify();
        };
        // Dropped, the panes come back from what is saved, on the next frame.
        self.tiles.remove(&cwd);
        self.saved_workspaces(&project).set_layout(&cwd, Some(layout));
        self.focus_input(window, cx);
        cx.notify();
    }
}
