//! Picking several past sessions in the sessions panel (Ctrl+click, Shift+click,
//! or the header's Select button) to archive or delete them together, behind
//! one confirmation. The picking rules are `kynex_core::groups::Selection`.

use std::path::{Path, PathBuf};

use crate::ui::px;
use gpui::{AnyElement, Context, Modifiers, div, prelude::*};
use kynex_core::session::{self, Kind, Session};
use kynex_core::toasts::Toast;

use super::KynexApp;
use super::storage::count;
use crate::theme::theme;
use crate::ui::{Icon, Tooltip, ghost_button, icon, icon_button, outline_button};

impl KynexApp {
    /// A click on past session `id`'s row: picks it (or a run of rows) when
    /// Ctrl or Shift is held or picking is on. Returns whether it did, so the
    /// row doesn't open too.
    pub(super) fn pick_click(&mut self, cwd: &Path, id: &str, m: &Modifiers, cx: &mut Context<Self>) -> bool {
        let o = &mut self.organize;
        if m.shift {
            o.selection.extend(&o.shown, id);
        } else if m.secondary(cx) || o.selection.on {
            o.selection.toggle(id);
        } else {
            return false;
        }
        o.selection_cwd = Some(cwd.to_path_buf());
        o.confirm_bulk_delete = false;
        cx.notify();
        true
    }

    /// The header's Select button: starts picking, or stops and drops the picks.
    pub(super) fn toggle_picking(&mut self, cwd: PathBuf, cx: &mut Context<Self>) {
        let o = &mut self.organize;
        if o.selection.on {
            o.selection.clear();
        } else {
            o.selection.on = true;
            o.selection_cwd = Some(cwd);
        }
        o.confirm_bulk_delete = false;
        cx.notify();
    }

    /// Esc: stops picking. Returns whether it was on.
    pub(super) fn stop_picking(&mut self) -> bool {
        let on = self.organize.selection.on;
        self.organize.selection.clear();
        self.organize.confirm_bulk_delete = false;
        on
    }

    /// Keeps the picks to the sessions `listed` in `cwd`; another folder drops them.
    pub(super) fn sync_picks(&mut self, cwd: &Path, listed: &[String]) {
        let o = &mut self.organize;
        if o.selection_cwd.as_deref() != Some(cwd) {
            o.selection.clear();
            o.selection_cwd = Some(cwd.to_path_buf());
        }
        o.selection.keep_only(listed);
    }

    /// Takes past session `id` out of Kynex before its transcript is deleted:
    /// shows a new session in its place if it's shown, closes it if it's open,
    /// and drops its draft and its place in the groups.
    pub(super) fn forget_past_session(&mut self, kind: Kind, cwd: &Path, id: &str) {
        if self.active_session().and_then(|s| s.session_id.as_deref()) == Some(id) {
            self.open_session(Session::new(kind, cwd.to_path_buf()));
        }
        let open = self
            .live_mut()
            .and_then(|l| l.sessions.iter().find(|s| s.session_id.as_deref() == Some(id)).map(|s| s.uid));
        if let Some(uid) = open {
            self.close_open(uid);
        }
        self.drafts.remove(id);
        if self.saved.drafts.remove(id).is_some() {
            self.save();
        }
        self.edit_groups(cwd, |g| g.forget(id));
    }

    /// Deletes every picked session but those still working, off the UI thread.
    fn delete_picked(&mut self, kind: Kind, cwd: PathBuf, busy: &[String], cx: &mut Context<Self>) {
        let picked = std::mem::take(&mut self.organize.selection.ids);
        let (ids, kept): (Vec<String>, Vec<String>) = picked.into_iter().partition(|id| !busy.contains(id));
        for id in &ids {
            self.forget_past_session(kind, &cwd, id);
        }
        self.stop_picking();
        self.sessions = None;
        cx.spawn(async move |this, cx| {
            let n = ids.len();
            let errors: Vec<String> = cx
                .background_spawn(async move {
                    ids.iter().filter_map(|id| session::delete_session(&cwd, id).err()).collect()
                })
                .await;
            this.update(cx, |this, cx| {
                let mut done = format!("Deleted {}", count(n - errors.len(), "session"));
                if !kept.is_empty() {
                    done += &format!("; kept {} still working", kept.len());
                }
                match errors.first() {
                    Some(e) => this.toast(Toast::error(format!("{done}. Couldn't delete {}: {e}", errors.len()))),
                    None => this.toast(Toast::success(done)),
                };
                this.sessions = None;
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Archives the picked sessions, or unarchives them if they all are.
    fn archive_picked(&mut self, cwd: &Path, cx: &mut Context<Self>) {
        let ids = std::mem::take(&mut self.organize.selection.ids);
        self.edit_groups(cwd, |g| {
            let all = ids.iter().all(|id| g.is_archived(id));
            for id in &ids {
                if all {
                    g.unarchive(id);
                } else {
                    g.archive(id);
                }
            }
        });
        self.stop_picking();
        cx.notify();
    }

    /// A row's box while picking, ticked when it's picked.
    pub(super) fn pick_box(&self, picked: bool, cx: &Context<Self>) -> AnyElement {
        let t = theme(cx);
        div()
            .flex_none()
            .size(px(14.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_sm()
            .border_1()
            .border_color(if picked { t.border_focused } else { t.border })
            .when(picked, |d| d.bg(t.border_focused).child(icon(Icon::Check, t.editor).size(px(10.))))
            .into_any_element()
    }

    /// The bar under the list while picking: how many, pick all, archive,
    /// delete (asking once, with the count), done. `shown` are the rows as
    /// drawn; `busy` the sessions still working, which aren't deleted.
    pub(super) fn render_pick_bar(
        &mut self,
        kind: Kind,
        cwd: PathBuf,
        shown: Vec<String>,
        busy: Vec<String>,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.organize.selection.on {
            return None;
        }
        let picked = self.organize.selection.ids.clone();
        let all_archived = !picked.is_empty() && picked.iter().all(|id| self.groups_for(&cwd).is_archived(id));
        let o = &self.organize;
        let t = theme(cx).clone();
        let n = o.selection.ids.len();
        let bar = div()
            .flex()
            .flex_none()
            .flex_wrap()
            .items_center()
            .gap_1()
            .px_2()
            .py_1()
            .border_t_1()
            .border_color(t.border)
            .text_xs();
        if o.confirm_bulk_delete && n > 0 {
            let working = o.selection.ids.iter().filter(|id| busy.contains(id)).count();
            let question = match working {
                0 => format!("Delete {}? Their transcripts can't be brought back.", count(n, "session")),
                w => format!(
                    "Delete {}? {w} still working {} kept.",
                    count(n - w, "session"),
                    if w == 1 { "is" } else { "are" }
                ),
            };
            return Some(
                bar.bg(t.error.opacity(0.08))
                    .child(div().w_full().text_color(t.text).child(question))
                    .child(div().flex_1())
                    .when(n > working, |d| {
                        d.child(outline_button("pick-yes", "Yes, delete", cx).text_color(t.error).on_click(
                            cx.listener(move |this, _, _, cx| this.delete_picked(kind, cwd.clone(), &busy, cx)),
                        ))
                    })
                    .child(outline_button("pick-no", "Cancel", cx).on_click(cx.listener(|this, _, _, cx| {
                        this.organize.confirm_bulk_delete = false;
                        cx.notify();
                    })))
                    .into_any_element(),
            );
        }
        let all_picked = !shown.is_empty() && shown.iter().all(|id| o.selection.contains(id));
        let archive_cwd = cwd.clone();
        Some(
            bar.child(div().flex_1().text_color(t.text_muted).child(if n == 0 {
                "Pick sessions".to_string()
            } else {
                format!("{n} selected")
            }))
            .child(ghost_button("pick-all", cx).child(if all_picked { "None" } else { "All" }).on_click(cx.listener(
                move |this, _, _, cx| {
                    this.organize.selection.toggle_all(&shown);
                    cx.notify();
                },
            )))
            .when(n > 0, |d| {
                d.child(
                    ghost_button("pick-archive", cx)
                        .child(if all_archived { "Unarchive" } else { "Archive" })
                        .on_click(cx.listener(move |this, _, _, cx| this.archive_picked(&archive_cwd, cx))),
                )
                .child(ghost_button("pick-delete", cx).text_color(t.error).child("Delete").on_click(
                    cx.listener(|this, _, _, cx| {
                        this.organize.confirm_bulk_delete = true;
                        cx.notify();
                    }),
                ))
            })
            .child(icon_button("pick-done", Icon::Close, cx).tooltip(Tooltip::text("Stop selecting (Esc)")).on_click(
                cx.listener(|this, _, _, cx| {
                    this.stop_picking();
                    cx.notify();
                }),
            ))
            .into_any_element(),
        )
    }
}
