//! Acting on many sessions at once in the sessions panel: the right-click menu
//! for picked sessions (archive, move to a group, delete), tidying old ones
//! away ("Archive sessions older than…"), and the list's keys (↑ / ↓, Enter,
//! F2, Delete, Shift+Delete) while it has the focus.

use std::time::UNIX_EPOCH;

use gpui::{AnyElement, Context, Window, div, prelude::*};
use kynex_core::session::Kind;
use kynex_core::toasts::Toast;

use super::KynexApp;
use super::keys::{ListArchive, ListDelete, ListDown, ListOpen, ListRename, ListUp};
use crate::theme::theme;
use crate::ui::{divider, menu_item};

/// What a menu entry does when clicked.
type Action = Box<dyn Fn(&mut KynexApp, &mut Window, &mut Context<KynexApp>)>;

/// The ages "Archive sessions older than…" offers, in days.
pub(super) const TIDY_AGES: [(u32, &str); 3] = [(30, "30 days"), (90, "90 days"), (365, "a year")];

impl KynexApp {
    /// The folder and kind of the sessions the panel lists.
    fn panel_folder(&self) -> Option<(Kind, std::path::PathBuf)> {
        self.active_session().map(|s| (s.kind, s.cwd.clone()))
    }

    /// The right-click menu for several picked sessions.
    pub(super) fn sessions_menu_items(&mut self, ids: &[String], cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some((_, cwd)) = self.panel_folder() else { return Vec::new() };
        let groups = self.groups_for(&cwd).clone();
        let n = ids.len();
        let t = theme(cx).clone();
        let mut items = Vec::new();
        let item = |label: String, key: usize, cx: &mut Context<Self>, f: Action| {
            menu_item(("sessions-menu", key), label, false, cx)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.menu = None;
                    f(this, window, cx);
                    cx.notify();
                }))
                .into_any_element()
        };
        let all_archived = ids.iter().all(|id| groups.is_archived(id));
        let (archive_ids, archive_cwd) = (ids.to_vec(), cwd.clone());
        items.push(item(
            if all_archived { format!("Unarchive {n}") } else { format!("Archive {n}") },
            0,
            cx,
            Box::new(move |this, _, _| {
                this.edit_groups(&archive_cwd, |g| {
                    for id in &archive_ids {
                        if all_archived {
                            g.unarchive(id);
                        } else {
                            g.archive(id);
                        }
                    }
                });
                this.organize.selection.clear();
            }),
        ));
        items.push(div().my_1().child(divider(cx)).into_any_element());
        for (i, g) in groups.groups.iter().enumerate() {
            let (ids, cwd) = (ids.to_vec(), cwd.clone());
            items.push(item(
                format!("Move {n} to {}", g.name),
                10 + i,
                cx,
                Box::new(move |this, _, _| {
                    this.edit_groups(&cwd, |g| ids.iter().for_each(|id| g.move_to(id, Some(i))));
                    this.organize.selection.clear();
                }),
            ));
        }
        let (new_ids, new_cwd) = (ids.to_vec(), cwd.clone());
        items.push(item(
            "New group with these…".into(),
            1,
            cx,
            Box::new(move |this, window, cx| {
                let mut at = 0;
                this.edit_groups(&new_cwd, |g| {
                    at = g.add_group("");
                    new_ids.iter().for_each(|id| g.move_to(id, Some(at)));
                });
                this.organize.selection.clear();
                this.start_group_rename(new_cwd.clone(), at, window, cx);
            }),
        ));
        if ids.iter().any(|id| groups.group_of(id).is_some()) {
            let (ids, cwd) = (ids.to_vec(), cwd.clone());
            items.push(item(
                "Remove from their groups".into(),
                2,
                cx,
                Box::new(move |this, _, _| {
                    this.edit_groups(&cwd, |g| ids.iter().for_each(|id| g.move_to(id, None)));
                }),
            ));
        }
        items.push(div().my_1().child(divider(cx)).into_any_element());
        let delete_ids = ids.to_vec();
        items.push(
            menu_item(("sessions-menu", 3usize), format!("Delete {n}…"), false, cx)
                .text_color(t.error)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.menu = None;
                    this.ask_delete_sessions(&delete_ids);
                    cx.notify();
                }))
                .into_any_element(),
        );
        items
    }

    /// The panel header's menu: archive sessions not used for a while.
    pub(super) fn sessions_panel_menu_items(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let by_provider = self.saved.sessions_by_provider;
        let mut items = vec![
            menu_item("sessions-by-provider", "Group by provider", by_provider, cx)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.menu = None;
                    this.saved.sessions_by_provider = !by_provider;
                    this.save();
                    cx.notify();
                }))
                .into_any_element(),
            div().my_1().child(divider(cx)).into_any_element(),
        ];
        items.extend(TIDY_AGES.iter().enumerate().map(|(i, (days, label))| {
            let days = *days;
            menu_item(("tidy", i), format!("Archive sessions older than {label}"), false, cx)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.menu = None;
                    this.archive_older_than(days, cx);
                }))
                .into_any_element()
        }));
        items
    }

    /// Archives the listed sessions of the shown folder not used in `days`;
    /// pinned and grouped ones stay. Says how many went.
    pub(super) fn archive_older_than(&mut self, days: u32, cx: &mut Context<Self>) {
        let Some((_, cwd)) = self.panel_folder() else { return };
        let secs = |t: std::time::SystemTime| t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
        let listed: Vec<(String, i64)> = self
            .sessions
            .as_ref()
            .filter(|l| l.read.cwd == cwd)
            .map(|l| l.items.iter().map(|i| (i.id.clone(), secs(i.modified))).collect())
            .unwrap_or_default();
        let cutoff = secs(std::time::SystemTime::now()) - i64::from(days) * 86_400;
        let mut archived = 0;
        self.edit_groups(&cwd, |g| archived = g.archive_older_than(&listed, cutoff));
        self.toast(Toast::info(match archived {
            0 => "No sessions to archive: pinned and grouped ones stay".to_string(),
            1 => "Archived 1 session".to_string(),
            n => format!("Archived {n} sessions"),
        }));
        cx.notify();
    }

    /// Picks `ids` and asks to delete them, with the panel's one confirmation.
    pub(super) fn ask_delete_sessions(&mut self, ids: &[String]) {
        self.organize.selection.clear();
        self.organize.selection.on = true;
        self.organize.selection.ids = ids.to_vec();
        self.organize.selection_cwd = self.panel_folder().map(|(_, cwd)| cwd);
        self.organize.confirm_bulk_delete = true;
    }

    /// The sessions the list's keys act on: the picked ones, else the one under the cursor.
    fn list_targets(&self) -> Vec<String> {
        if self.organize.selection.ids.is_empty() {
            self.organize.cursor.clone().into_iter().collect()
        } else {
            self.organize.selection.ids.clone()
        }
    }

    fn list_step(&mut self, by: isize, cx: &mut Context<Self>) {
        let shown = &self.organize.shown;
        if shown.is_empty() {
            return;
        }
        let at = self.organize.cursor.as_ref().and_then(|c| shown.iter().position(|s| s == c));
        let next = match at {
            Some(i) => (i as isize + by).clamp(0, shown.len() as isize - 1) as usize,
            None if by > 0 => 0,
            None => shown.len() - 1,
        };
        self.organize.cursor = Some(shown[next].clone());
        cx.notify();
    }

    pub(super) fn list_up(&mut self, _: &ListUp, _: &mut Window, cx: &mut Context<Self>) {
        self.list_step(-1, cx);
    }

    pub(super) fn list_down(&mut self, _: &ListDown, _: &mut Window, cx: &mut Context<Self>) {
        self.list_step(1, cx);
    }

    /// Enter: opens the session under the cursor.
    pub(super) fn list_open(&mut self, _: &ListOpen, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(id), Some((kind, cwd))) = (self.organize.cursor.clone(), self.panel_folder()) else { return };
        self.show_code_view();
        if self.active_session().and_then(|s| s.session_id.as_deref()) != Some(id.as_str()) {
            self.switch_session(kind, cwd, id);
        }
        self.focus_input(window, cx);
        cx.notify();
    }

    /// F2: renames the session under the cursor.
    pub(super) fn list_rename(&mut self, _: &ListRename, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(id), Some((_, cwd))) = (self.organize.cursor.clone(), self.panel_folder()) else { return };
        self.start_session_rename(cwd, id, false, window, cx);
    }

    /// Delete: archives the picked sessions (or the one under the cursor);
    /// archived ones come back.
    pub(super) fn list_archive(&mut self, _: &ListArchive, _: &mut Window, cx: &mut Context<Self>) {
        let (ids, Some((_, cwd))) = (self.list_targets(), self.panel_folder()) else { return };
        if ids.is_empty() {
            return;
        }
        self.edit_groups(&cwd, |g| {
            let all_archived = ids.iter().all(|id| g.is_archived(id));
            for id in &ids {
                if all_archived {
                    g.unarchive(id);
                } else {
                    g.archive(id);
                }
            }
        });
        self.organize.selection.clear();
        cx.notify();
    }

    /// Shift+Delete: deletes the picked sessions (or the one under the cursor), after asking.
    pub(super) fn list_delete(&mut self, _: &ListDelete, _: &mut Window, cx: &mut Context<Self>) {
        let ids = self.list_targets();
        if !ids.is_empty() {
            self.ask_delete_sessions(&ids);
            cx.notify();
        }
    }
}
