//! "Removed workspaces": a last, folded section of the sessions tree. Removing a workspace
//! keeps its sessions (`kynex_core::removed`), so each former workspace is a row here, folded,
//! with its sessions under it once opened. A session opens to read: the log is found without
//! its folder, which is gone, so nothing can be sent. "Continue in <main workspace>" copies it
//! into the project's own transcripts and resumes it there. "Forget…" deletes them all.
//!
//! Right-clicking a row opens a small strip under it (not a popup menu) with what it can do.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use gpui::{AnyElement, Context, MouseButton, MouseDownEvent, div, prelude::*};
use kynex_core::removed::{self, Removed};
use kynex_core::session::{self, Kind, SessionInfo};
use kynex_core::toasts::Toast;
use kynex_core::workspace;

use super::KynexApp;
use crate::theme::{mono, theme};
use crate::ui::wash::row_wash;
use crate::ui::{Icon, Tooltip, ghost_button, icon, outline_button, px, section_label};

/// What the section shows and asks. Folding is for this run only.
#[derive(Default)]
pub(super) struct RemovedUi {
    /// The section itself is open.
    open: bool,
    /// The former workspaces that are unfolded.
    unfolded: HashSet<PathBuf>,
    /// Their sessions, read when first unfolded.
    sessions: HashMap<PathBuf, Vec<SessionInfo>>,
    /// The strip under a row: what the right-click offers.
    acting: Option<Acting>,
}

#[derive(Clone, PartialEq)]
enum Acting {
    /// A former workspace's strip: Forget…
    Workspace(PathBuf),
    /// The same, after Forget…: asks once.
    Forgetting(PathBuf),
    /// A session's strip: Continue in the main workspace.
    Session(PathBuf, String),
}

impl KynexApp {
    /// Reads the sessions of the removed workspace at `path` off the UI thread.
    fn read_removed_sessions(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let there = path.clone();
        cx.spawn(async move |this, cx| {
            let list = cx.background_spawn(async move { session::list_sessions(&there) }).await;
            this.update(cx, |this, cx| {
                this.removed_ui.sessions.insert(path, list);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Folds or unfolds a former workspace, reading its sessions the first time.
    fn toggle_removed(&mut self, path: &Path, cx: &mut Context<Self>) {
        let ui = &mut self.removed_ui;
        if !ui.unfolded.remove(path) {
            ui.unfolded.insert(path.to_path_buf());
            self.read_removed_sessions(path.to_path_buf(), cx);
        }
        cx.notify();
    }

    /// "Forget" on a removed workspace: deletes its sessions (transcripts, and their places in
    /// its groups) and drops it from the list. Off the UI thread.
    pub(super) fn forget_removed_workspace(&mut self, project: PathBuf, path: PathBuf, cx: &mut Context<Self>) {
        self.removed_ui.acting = None;
        let there = path.clone();
        cx.spawn(async move |this, cx| {
            let purged = cx.background_spawn(async move { removed::delete_sessions(&there) }).await;
            this.update(cx, |this, cx| {
                for id in &purged.deleted {
                    this.drafts.remove(id);
                    if this.saved.drafts.remove(id).is_some() {
                        this.save();
                    }
                }
                this.edit_groups(&path, |g| purged.deleted.iter().for_each(|id| g.forget(id)));
                this.removed_ui.sessions.remove(&path);
                this.removed_ui.unfolded.remove(&path);
                if let Some(live) = this.code.get_mut(&project) {
                    let closed = live.close_where(|s| {
                        workspace::same_folder(&s.cwd, &path)
                            && s.session_id.as_ref().is_some_and(|id| purged.deleted.contains(id))
                    });
                    this.forget(&closed);
                }
                match purged.errors.first() {
                    // Some stay: so does the row, for another try.
                    Some(e) => {
                        this.toast(Toast::error(format!("Couldn't delete {} session(s): {e}", purged.errors.len())));
                    }
                    None => {
                        this.saved_workspaces(&project).forget_removed(&path);
                        this.save_workspaces(&project, cx);
                        this.toast(Toast::success("Forgot the workspace and its sessions"));
                    }
                }
                this.reread_sessions();
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// "Continue in <main workspace>": copies the log there, then shows the main workspace
    /// with the session resumed in it.
    fn continue_removed_session(&mut self, from: PathBuf, id: String, cx: &mut Context<Self>) {
        self.removed_ui.acting = None;
        let Some(project) = self.workspace_project() else { return };
        let (to, sid) = (project.clone(), id.clone());
        cx.spawn(async move |this, cx| {
            let copied = cx.background_spawn(async move { session::continue_in(&from, &sid, &to) }).await;
            this.update(cx, |this, cx| {
                match copied {
                    Ok(()) => {
                        this.select_workspace(&project, cx);
                        this.switch_session(Kind::Code, project, id);
                        this.reread_sessions();
                    }
                    Err(e) => _ = this.toast(Toast::error(e)),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The section: nothing when no workspace was removed. A header that folds, a row per
    /// former workspace (name, branch) that folds too, and under it its sessions.
    pub(super) fn render_removed_workspaces(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let project = self.workspace_project()?;
        let current: Vec<_> = self.workspaces_of(&project, cx).into_iter().map(|(c, _)| c).collect();
        let shown: Vec<Removed> =
            self.saved_workspaces(&project).removed_to_show(&current).into_iter().cloned().collect();
        if shown.is_empty() {
            return None;
        }
        let t = theme(cx).clone();
        let open = self.removed_ui.open;
        let acting = self.removed_ui.acting.clone();
        let main = project.file_name().map_or_else(|| "the project".into(), |n| n.to_string_lossy().into_owned());
        let mut rows: Vec<AnyElement> = Vec::new();
        for (n, r) in shown.iter().enumerate().filter(|_| open) {
            let unfolded = self.removed_ui.unfolded.contains(&r.path);
            let (path, menu_path) = (r.path.clone(), r.path.clone());
            rows.push(
                div()
                    .id(("removed-workspace", n))
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .min_h(px(28.))
                    .pl_2()
                    .pr_1()
                    .rounded_md()
                    .cursor_pointer()
                    .text_sm()
                    .relative()
                    .child(row_wash(false, t.hover, t.selection))
                    .child(
                        icon(if unfolded { Icon::ChevronDown } else { Icon::ChevronRight }, t.text_placeholder)
                            .size(px(10.)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(t.text_muted)
                            .child(r.label.clone()),
                    )
                    .children(r.branch.clone().filter(|b| *b != r.label).map(|b| {
                        div()
                            .flex_none()
                            .max_w(px(100.))
                            .truncate()
                            .font_family(mono())
                            .text_xs()
                            .text_color(t.text_placeholder)
                            .child(b)
                    }))
                    .tooltip(Tooltip::text(format!("{}\nRemoved: its sessions stay", r.path.display())))
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_removed(&path, cx)))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.removed_ui.acting = Some(Acting::Workspace(menu_path.clone()));
                            cx.notify();
                        }),
                    )
                    .into_any_element(),
            );
            match &acting {
                Some(Acting::Workspace(p)) if workspace::same_folder(p, &r.path) => {
                    let path = r.path.clone();
                    rows.push(strip(
                        &t,
                        "Remove its sessions and this row",
                        cx,
                        [
                            ghost_button(("removed-forget", n), cx).text_color(t.error).child("Forget…").on_click(
                                cx.listener(move |this, _, _, cx| {
                                    this.removed_ui.acting = Some(Acting::Forgetting(path.clone()));
                                    cx.notify();
                                }),
                            ),
                            ghost_button(("removed-close", n), cx).child("Cancel").on_click(cx.listener(
                                |this, _, _, cx| {
                                    this.removed_ui.acting = None;
                                    cx.notify();
                                },
                            )),
                        ],
                    ));
                }
                Some(Acting::Forgetting(p)) if workspace::same_folder(p, &r.path) => {
                    let (project, path) = (project.clone(), r.path.clone());
                    let sessions = self.removed_ui.sessions.get(&r.path).map(Vec::len);
                    let what = match sessions {
                        Some(1) => "its 1 session".to_string(),
                        Some(k) => format!("its {k} sessions"),
                        None => "its sessions".to_string(),
                    };
                    rows.push(strip(
                        &t,
                        &format!("Delete {what}? They can't be brought back."),
                        cx,
                        [
                            outline_button(("removed-forget-yes", n), "Forget", cx).text_color(t.error).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    this.forget_removed_workspace(project.clone(), path.clone(), cx)
                                }),
                            ),
                            ghost_button(("removed-forget-no", n), cx).child("Keep").on_click(cx.listener(
                                |this, _, _, cx| {
                                    this.removed_ui.acting = None;
                                    cx.notify();
                                },
                            )),
                        ],
                    ));
                }
                _ => {}
            }
            if !unfolded {
                continue;
            }
            let Some(list) = self.removed_ui.sessions.get(&r.path) else {
                rows.push(
                    div()
                        .pl_6()
                        .h(px(22.))
                        .text_xs()
                        .text_color(t.text_placeholder)
                        .child("Loading…")
                        .into_any_element(),
                );
                continue;
            };
            for (i, s) in list.iter().enumerate() {
                let id = i + n * 1000;
                let (from, sid, menu_sid) = (r.path.clone(), s.id.clone(), s.id.clone());
                let menu_from = r.path.clone();
                rows.push(
                    div()
                        .id(("removed-session", id))
                        .flex()
                        .items_center()
                        .h(px(24.))
                        .pl_6()
                        .pr_1()
                        .rounded_md()
                        .cursor_pointer()
                        .text_sm()
                        .relative()
                        .child(row_wash(false, t.hover, t.selection))
                        .child(div().flex_1().min_w_0().truncate().text_color(t.text_muted).child(s.title.clone()))
                        .tooltip(Tooltip::text(format!(
                            "{}\nRead only: its folder is gone",
                            kynex_core::fmt::ago(s.modified)
                        )))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.switch_session(Kind::Code, from.clone(), sid.clone());
                            cx.notify();
                        }))
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                                cx.stop_propagation();
                                this.removed_ui.acting = Some(Acting::Session(menu_from.clone(), menu_sid.clone()));
                                cx.notify();
                            }),
                        )
                        .into_any_element(),
                );
                if let Some(Acting::Session(p, sid)) = &acting
                    && sid == &s.id
                    && workspace::same_folder(p, &r.path)
                {
                    let (from, sid) = (r.path.clone(), s.id.clone());
                    rows.push(strip(
                        &t,
                        "Its folder is gone: copy it to the project's folder to go on",
                        cx,
                        [
                            outline_button(("removed-continue", id), format!("Continue in {main}"), cx).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    this.continue_removed_session(from.clone(), sid.clone(), cx)
                                }),
                            ),
                            ghost_button(("removed-continue-no", id), cx).child("Cancel").on_click(cx.listener(
                                |this, _, _, cx| {
                                    this.removed_ui.acting = None;
                                    cx.notify();
                                },
                            )),
                        ],
                    ));
                }
            }
            if list.is_empty() {
                rows.push(
                    div()
                        .pl_6()
                        .h(px(22.))
                        .text_xs()
                        .text_color(t.text_placeholder)
                        .child("No sessions left")
                        .into_any_element(),
                );
            }
        }
        let header = div()
            .id("removed-workspaces-header")
            .flex()
            .items_center()
            .gap_1p5()
            .px_2()
            .pt_1p5()
            .pb_1()
            .cursor_pointer()
            .child(icon(if open { Icon::ChevronDown } else { Icon::ChevronRight }, t.text_placeholder).size(px(10.)))
            .child(section_label(&format!("Removed workspaces · {}", shown.len()), cx).flex_1())
            .tooltip(Tooltip::text("Workspaces removed here: their sessions stay to read"))
            .on_click(cx.listener(|this, _, _, cx| {
                this.removed_ui.open = !this.removed_ui.open;
                cx.notify();
            }));
        Some(
            div()
                .flex()
                .flex_col()
                .flex_none()
                .px_1p5()
                .pb_1p5()
                .border_b_1()
                .border_color(t.border_variant)
                .child(header)
                .children(rows)
                .into_any_element(),
        )
    }
}

/// The strip under a row after a right-click: a line of words and its buttons.
fn strip<const N: usize>(
    t: &crate::theme::Palette,
    text: &str,
    _cx: &Context<KynexApp>,
    buttons: [gpui::Stateful<gpui::Div>; N],
) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .pl_6()
        .pr_2()
        .py_1()
        .child(div().text_xs().text_color(t.text_muted).child(text.to_string()))
        .child(div().flex().flex_wrap().gap_1().children(buttons))
        .into_any_element()
}
