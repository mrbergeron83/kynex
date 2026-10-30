//! The Branches tab's dialogs and worktrees: the name field (rename, new
//! branch, add worktree), the delete confirmation and its undo, and the
//! worktree rows with open / remove.

use crate::ui::px;
use gpui::{AnyElement, Context, Focusable, FontWeight, SharedString, Window, div, prelude::*};
use kynex_core::git::cleanup::{self, Overview};
use kynex_core::git::{self, Worktree};

use super::branches::{Naming, badge};
use super::{GitEvent, GitView};
use crate::theme::{Palette, mono, theme};
use crate::ui::wash::row_wash;
use crate::ui::{
    Icon, Tooltip, ghost_button, icon, icon_button, outline_button, popover, primary_button, section_label,
};

/// `card` centred over the Git view on a dimmed backdrop; a click beside it closes it.
fn backdrop(card: gpui::Stateful<gpui::Div>, cx: &mut Context<GitView>) -> AnyElement {
    div()
        .id("git-dialog-backdrop")
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .bg(gpui::black().opacity(0.35))
        .occlude()
        .on_mouse_down(
            gpui::MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                this.branch_tab.confirm = None;
                this.branch_tab.confirm_remote = None;
                cx.notify();
            }),
        )
        .child(card.on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation()))
        .into_any_element()
}

impl GitView {
    pub(super) fn start_naming(&mut self, naming: Naming, cx: &mut Context<Self>) {
        let text = match &naming {
            Naming::Rename(old) => old.clone(),
            Naming::NewFrom(_) | Naming::AddWorktree => String::new(),
        };
        self.name_input.update(cx, |i, cx| i.set_text(text, cx));
        self.branch_tab.naming = Some(naming);
        self.branch_tab.focus_name = true;
        cx.notify();
    }

    pub(super) fn submit_naming(&mut self, cx: &mut Context<Self>) {
        let Some(naming) = self.branch_tab.naming.clone() else { return };
        let name = self.name_input.read(cx).text().trim().to_string();
        if name.is_empty() {
            return;
        }
        self.branch_tab.naming = None;
        self.name_input.update(cx, |i, cx| i.set_text("", cx));
        match naming {
            Naming::Rename(old) => {
                self.run("Renaming branch", cx, move |root| git::rename_branch(root, &old, &name), |_, _| {})
            }
            Naming::NewFrom(start) => {
                self.run("Creating branch", cx, move |root| git::create_branch_at(root, &name, &start), |_, _| {})
            }
            Naming::AddWorktree => self.run(
                "Adding worktree",
                cx,
                move |root| git::add_worktree(root, &git::default_worktree_path(root, &name), &name),
                |_, _| {},
            ),
        }
    }

    /// The name field above the list while renaming, branching or adding a worktree.
    pub(super) fn naming_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let naming = self.branch_tab.naming.clone()?;
        let t = theme(cx).clone();
        if std::mem::take(&mut self.branch_tab.focus_name) {
            self.name_input.read(cx).focus_handle(cx).focus(window, cx);
        }
        let focused = self.name_input.read(cx).is_focused(window);
        let (label, hint) = match &naming {
            Naming::Rename(old) => (format!("Rename {old} to"), "Rename".to_string()),
            Naming::NewFrom(start) => (format!("New branch from {start}"), "Create and switch".to_string()),
            Naming::AddWorktree => (
                "New worktree on branch (existing or new)".to_string(),
                format!("Add in {}", git::default_worktree_path(&self.root, "<branch>").display()),
            ),
        };
        Some(
            div()
                .flex()
                .flex_col()
                .flex_none()
                .gap_1()
                .p_2()
                .border_b_1()
                .border_color(t.border_variant)
                .child(div().text_xs().text_color(t.text_muted).child(label))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .border_1()
                                .border_color(if focused { t.border_focused } else { t.border })
                                .bg(t.editor)
                                .text_sm()
                                .font_family(mono())
                                .child(self.name_input.clone()),
                        )
                        .child(
                            primary_button("git-name-ok", "OK", cx)
                                .tooltip(Tooltip::with_meta(hint, "Enter"))
                                .on_click(cx.listener(|this, _, _, cx| this.submit_naming(cx))),
                        )
                        .child(icon_button("git-name-cancel", Icon::Close, cx).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.branch_tab.naming = None;
                                cx.notify();
                            },
                        ))),
                )
                .into_any_element(),
        )
    }

    /// Deletes what the confirmation lists: the finished branches, and the
    /// unmerged ones too if `all`. Keeps what it deleted for Undo.
    fn delete_confirmed(&mut self, all: bool, cx: &mut Context<Self>) {
        let Some(c) = self.branch_tab.confirm.take() else { return };
        if self.busy.is_some() {
            return;
        }
        let mut items = c.finished;
        if all {
            items.extend(c.unmerged.into_iter().map(|(i, _)| i));
        }
        let with_remote = c.with_remote;
        self.branch_tab.picks.clear();
        self.busy = Some("Deleting branches".into());
        self.error = None;
        cx.notify();
        let root = self.root.clone();
        cx.spawn(async move |this, cx| {
            let (deleted, errors) =
                cx.background_spawn(async move { cleanup::delete(&root, &items, with_remote) }).await;
            this.update(cx, |this, cx| {
                this.busy = None;
                // What was deleted can come back even when some of it failed.
                if !deleted.is_empty() {
                    this.branch_tab.undo = Some(deleted);
                }
                if !errors.is_empty() {
                    this.error = Some(errors.join("\n"));
                }
                this.refresh(cx);
            })
            .ok();
        })
        .detach();
    }

    /// The delete confirmation, over the whole Git view.
    pub(super) fn render_confirm(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if let Some(remote) = self.branch_tab.confirm_remote.clone() {
            return Some(self.render_confirm_remote(remote, cx));
        }
        let c = self.branch_tab.confirm.as_ref()?;
        let t = theme(cx).clone();
        let base = self
            .branch_tab
            .overview
            .as_ref()
            .and_then(|o| o.default.clone())
            .unwrap_or_else(|| "the default branch".into());
        let (n, m) = (c.finished.len(), c.unmerged.len());
        let remotes = c.finished.iter().chain(c.unmerged.iter().map(|(i, _)| i)).filter(|i| i.remote.is_some()).count();
        let with_remote = c.with_remote;
        let worktrees = c.finished.iter().filter(|i| i.worktree.is_some()).count();
        let row = |name: String, detail: String, t: &Palette| {
            div()
                .flex()
                .items_center()
                .gap_2()
                .h(px(22.))
                .text_sm()
                .child(div().flex_1().min_w_0().truncate().font_family(mono()).child(name))
                .child(div().flex_none().text_xs().text_color(t.text_placeholder).child(detail))
        };
        let mut list: Vec<AnyElement> = c
            .finished
            .iter()
            .map(|i| {
                let mut detail = Vec::new();
                if i.worktree.is_some() {
                    detail.push("and its worktree".to_string());
                }
                if with_remote && let Some(r) = &i.remote {
                    detail.push(format!("and {r}"));
                }
                row(i.branch.clone(), detail.join(", "), &t).into_any_element()
            })
            .collect();
        if m > 0 {
            list.push(
                div()
                    .pt_2()
                    .pb_0p5()
                    .text_xs()
                    .text_color(t.warning)
                    .child(format!("Not merged into {base}"))
                    .into_any_element(),
            );
            list.extend(c.unmerged.iter().map(|(i, k)| {
                let mut detail = if *k > 0 {
                    format!("{k} commit{} not in {base}", if *k == 1 { "" } else { "s" })
                } else {
                    "not merged".into()
                };
                // Its work stays on the remote unless that goes too.
                match &i.remote {
                    Some(r) if !with_remote => detail.push_str(&format!(", kept on {r}")),
                    Some(r) => detail.push_str(&format!(", and {r}")),
                    None => {}
                }
                row(i.branch.clone(), detail, &t).text_color(t.warning).into_any_element()
            }));
        }
        let title = match (n, m) {
            (0, 1) => format!("Delete {}?", c.unmerged[0].0.branch),
            (0, m) => format!("Delete {m} unmerged branches?"),
            (n, _) => format!("Delete {n} merged branch{}?", if n == 1 { "" } else { "es" }),
        };
        let card = popover(cx)
            .id("git-confirm")
            .occlude()
            .w(px(440.))
            .p_3()
            .gap_2()
            .child(div().text_base().font_weight(FontWeight::SEMIBOLD).child(title))
            .child(div().text_xs().text_color(t.text_muted).child(if worktrees > 0 {
                format!(
                    "Clean worktrees of these branches are removed first ({worktrees}). Undo brings the branches back."
                )
            } else {
                "Undo brings them back, at the commits they point to now.".to_string()
            }))
            .child({
                let scroll = crate::ui::scrollbar::handle("git-confirm-list");
                let area = div()
                    .id("git-confirm-list")
                    .max_h(px(260.))
                    .overflow_y_scroll()
                    .track_scroll(&scroll)
                    .children(list);
                crate::ui::scrollbar::vertical_over("git-confirm-bar", scroll, area, |d| d)
            })
            .when(remotes > 0, |d| {
                d.child(
                    div()
                        .id("git-confirm-remote")
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .cursor_pointer()
                        .text_sm()
                        .child(
                            div()
                                .size(px(14.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_sm()
                                .border_1()
                                .border_color(if with_remote { t.accent } else { t.border })
                                .when(with_remote, |d| d.bg(t.accent).child(icon(Icon::Check, t.on_accent).size_3())),
                        )
                        .child(format!("Also delete their remote branches ({remotes})"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(c) = &mut this.branch_tab.confirm {
                                c.with_remote = !c.with_remote;
                            }
                            cx.notify();
                        })),
                )
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap_2()
                    .pt_1()
                    .child(outline_button("git-confirm-cancel", "Cancel", cx).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.branch_tab.confirm = None;
                            cx.notify();
                        },
                    )))
                    .when(m > 0, |d| {
                        d.child(
                            outline_button(
                                "git-confirm-all",
                                if n == 0 { "Delete anyway".to_string() } else { format!("Delete all {}", n + m) },
                                cx,
                            )
                            .text_color(t.error)
                            .on_click(cx.listener(|this, _, _, cx| this.delete_confirmed(true, cx))),
                        )
                    })
                    .when(n > 0, |d| {
                        d.child(
                            primary_button("git-confirm-delete", format!("Delete {n}"), cx)
                                .on_click(cx.listener(|this, _, _, cx| this.delete_confirmed(false, cx))),
                        )
                    }),
            );
        Some(backdrop(card, cx))
    }

    /// "Delete origin/feature on the remote?": nothing in Kynex brings it back.
    fn render_confirm_remote(&mut self, remote: String, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx).clone();
        let card = popover(cx)
            .id("git-confirm-remote-branch")
            .occlude()
            .w(px(420.))
            .p_3()
            .gap_2()
            .child(div().text_base().font_weight(FontWeight::SEMIBOLD).child(format!("Delete {remote} on the remote?")))
            .child(
                div().text_sm().text_color(t.text_muted).child(
                    "Everyone who uses the remote loses it, and Undo can't bring it back. Your local branch stays.",
                ),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .pt_1()
                    .child(outline_button("git-remote-cancel", "Cancel", cx).on_click(cx.listener(|this, _, _, cx| {
                        this.branch_tab.confirm_remote = None;
                        cx.notify();
                    })))
                    .child(primary_button("git-remote-delete", "Delete", cx).on_click(cx.listener(
                        move |this, _, _, cx| {
                            this.branch_tab.confirm_remote = None;
                            let remote = remote.clone();
                            this.run(
                                "Deleting remote branch",
                                cx,
                                move |root| git::delete_remote_branch(root, &remote),
                                |_, _| {},
                            );
                        },
                    ))),
            );
        backdrop(card, cx)
    }

    /// "Deleted N branches · Undo" after a delete, until undone or dismissed.
    pub(super) fn render_undo(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let deleted = self.branch_tab.undo.clone()?;
        let t = theme(cx).clone();
        let n = deleted.len();
        Some(
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap_2()
                .px_3()
                .py_1p5()
                .border_b_1()
                .border_color(t.border_variant)
                .bg(t.info.opacity(0.08))
                .text_sm()
                .child(icon(Icon::Trash, t.text_muted).size_3p5())
                .child(div().flex_1().min_w_0().truncate().child(if n == 1 {
                    format!("Deleted {}", deleted[0].branch)
                } else {
                    format!("Deleted {n} branches")
                }))
                .child(
                    ghost_button("git-undo-delete", cx)
                        .child(icon(Icon::Undo, t.text_muted).size_3p5())
                        .child("Undo")
                        .tooltip(Tooltip::text("Bring them back where they were, remote branches too"))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let deleted = deleted.clone();
                            this.branch_tab.undo = None;
                            this.run("Restoring branches", cx, move |root| cleanup::undo(root, &deleted), |_, _| {});
                        })),
                )
                .child(icon_button("git-undo-dismiss", Icon::Close, cx).on_click(cx.listener(|this, _, _, cx| {
                    this.branch_tab.undo = None;
                    cx.notify();
                })))
                .into_any_element(),
        )
    }

    pub(super) fn worktrees_header(&mut self, o: &Overview, t: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let prunable = o.worktrees.iter().any(|w| w.prunable);
        div()
            .flex()
            .items_center()
            .gap_1p5()
            .px_2()
            .pt_3()
            .pb_1()
            .child(section_label("Worktrees", cx))
            .child(div().text_xs().text_color(t.text_placeholder).child(o.worktrees.len().to_string()))
            .child(div().flex_1())
            .when(prunable, |d| {
                d.child(
                    ghost_button("git-wt-prune", cx)
                        .text_xs()
                        .text_color(t.text_muted)
                        .child("Prune")
                        .tooltip(Tooltip::text("Forget worktrees whose folders are gone"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.run("Pruning worktrees", cx, git::prune_worktrees, |_, _| {})
                        })),
                )
            })
            .child(
                icon_button("git-wt-add", Icon::Plus, cx)
                    .tooltip(Tooltip::text("Add a worktree"))
                    .on_click(cx.listener(|this, _, _, cx| this.start_naming(Naming::AddWorktree, cx))),
            )
            .into_any_element()
    }

    pub(super) fn worktree_row(&mut self, i: usize, w: &Worktree, t: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let hover = t.hover;
        let folder =
            w.path.file_name().map_or_else(|| w.path.display().to_string(), |n| n.to_string_lossy().into_owned());
        let asked = self.branch_tab.remove_worktree.as_ref().filter(|(p, _)| *p == w.path).map(|(_, dirty)| *dirty);
        let changes = match w.changes {
            Some(0) => "clean".to_string(),
            Some(n) => format!("{n} changed"),
            None => String::new(),
        };
        let here = w.path == self.root;
        let (open_path, remove_path) = (w.path.clone(), w.path.clone());
        let dirty = w.changes.is_some_and(|n| n > 0);
        div()
            .id(("git-wt", i))
            .group("git-wt-row")
            .relative()
            .flex()
            .flex_col()
            .justify_center()
            .gap_0p5()
            .h(px(44.))
            .px_2()
            .rounded_md()
            .child(row_wash(false, hover, t.active))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .min_w_0()
                    .child(icon(Icon::Folder, t.text_muted).size_3p5())
                    .child(div().min_w_0().truncate().text_sm().child(folder))
                    .when(w.main, |d| d.child(badge("main", t.text_muted, t)))
                    .when(w.locked, |d| d.child(badge("locked", t.warning, t)))
                    .when(w.prunable, |d| d.child(badge("folder gone", t.error, t)))
                    .child(div().flex_1())
                    .when(!here && !w.prunable, |d| {
                        d.child(
                            div().invisible().group_hover("git-wt-row", |s| s.visible()).child(
                                ghost_button(("git-wt-open", i), cx)
                                    .text_xs()
                                    .child("Open")
                                    .tooltip(Tooltip::text("Open it as a project tab"))
                                    .on_click(
                                        cx.listener(move |_, _, _, cx| {
                                            cx.emit(GitEvent::OpenFolder(open_path.clone()))
                                        }),
                                    ),
                            ),
                        )
                    })
                    .when(!w.main && !w.locked, |d| {
                        let label = match asked {
                            Some(true) => "Remove anyway",
                            Some(false) => "Remove?",
                            None => "Remove",
                        };
                        d.child(
                            div()
                                .when(asked.is_none(), |d| d.invisible().group_hover("git-wt-row", |s| s.visible()))
                                .child(
                                    ghost_button(("git-wt-remove", i), cx)
                                        .text_xs()
                                        .text_color(if asked.is_some() { t.error } else { t.text_muted })
                                        .child(label)
                                        .tooltip(Tooltip::text(if dirty {
                                            "It has uncommitted changes, which go with it"
                                        } else {
                                            "Remove this worktree's folder (its branch stays)"
                                        }))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            let path = remove_path.clone();
                                            if this.branch_tab.remove_worktree.as_ref().is_some_and(|(p, _)| *p == path)
                                            {
                                                this.branch_tab.remove_worktree = None;
                                                this.run(
                                                    "Removing worktree",
                                                    cx,
                                                    move |root| git::remove_worktree(root, &path, dirty),
                                                    |_, _| {},
                                                );
                                            } else {
                                                this.branch_tab.remove_worktree = Some((path, dirty));
                                                cx.notify();
                                            }
                                        })),
                                ),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .gap_1p5()
                    .min_w_0()
                    .text_xs()
                    .text_color(t.text_placeholder)
                    .child(div().flex_none().font_family(mono()).child(w.label().to_string()))
                    .when(!changes.is_empty(), |d| {
                        d.child("·").child(div().flex_none().when(dirty, |d| d.text_color(t.warning)).child(changes))
                    }),
            )
            .tooltip(Tooltip::text(SharedString::from(w.path.display().to_string())))
            .into_any_element()
    }
}
