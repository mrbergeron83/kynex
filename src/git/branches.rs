//! The Branches tab: local branches, then remote-only ones, then worktrees,
//! each with what's known about it (upstream, pull request, merged, protected),
//! a menu of actions, and picking several with Ctrl / Shift+click to delete
//! them together. The rules are `kynex_core::git::cleanup`; the confirmation,
//! undo, naming and worktree parts are in `branch_ops.rs`.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::ui::px;
use gpui::{
    AnyElement, ClickEvent, Context, FontWeight, Hsla, SharedString, Window, anchored, deferred, div, prelude::*,
};
use kynex_core::fmt::ago_unix;
use kynex_core::git;
use kynex_core::git::cleanup::{self, BranchRow, Cleanup, Deleted, GitHubInfo, Merged, Overview};
use kynex_core::git::github::{Checks, PrState};
use kynex_core::groups::Selection as Picks;

use super::{GitView, Selection};
use crate::theme::{Palette, mono, theme};
use crate::ui::wash::row_wash;
use crate::ui::{Icon, Tooltip, ghost_button, icon, icon_button, menu_item, popover, scrollbar, section_label};

/// How long GitHub's answers (pull requests, protection) are reused.
const GITHUB_FOR: Duration = Duration::from_secs(300);

/// What a branch menu entry does.
type MenuAction = Box<dyn Fn(&mut GitView, &mut Context<GitView>)>;

/// What a name typed in the tab's name field is for.
#[derive(Clone, PartialEq)]
pub(super) enum Naming {
    Rename(String),
    NewFrom(String),
    AddWorktree,
}

/// Branches about to be deleted, waiting on a yes.
pub(super) struct Confirm {
    /// Finished: merged or squash-merged.
    pub(super) finished: Vec<Cleanup>,
    /// With work nowhere else, and how many commits of it.
    pub(super) unmerged: Vec<(Cleanup, usize)>,
    pub(super) with_remote: bool,
}

#[derive(Default)]
pub(super) struct BranchTab {
    pub(super) overview: Option<Overview>,
    pub(super) loading: bool,
    /// GitHub's last answer, `None` inside when `gh` couldn't give one.
    github: Option<(Instant, Option<GitHubInfo>)>,
    pub(super) picks: Picks,
    /// The branch whose action menu is open.
    pub(super) menu_for: Option<String>,
    pub(super) naming: Option<Naming>,
    pub(super) confirm: Option<Confirm>,
    /// Deleted by the last delete, until undone or dismissed.
    pub(super) undo: Option<Vec<Deleted>>,
    /// A worktree whose Remove was clicked once, and whether it has changes.
    pub(super) remove_worktree: Option<(PathBuf, bool)>,
    /// The name field just opened and wants the keyboard.
    pub(super) focus_name: bool,
    /// A remote branch (`origin/feature`) to delete, waiting on a yes.
    pub(super) confirm_remote: Option<String>,
}

impl BranchTab {
    /// The next read asks GitHub again (the refresh button, a fetch).
    pub(super) fn forget_github(&mut self) {
        self.github = None;
    }
}

impl GitView {
    /// Re-reads the Branches tab, asking GitHub again only every few minutes
    /// (or when `fresh`).
    pub(super) fn load_branches(&mut self, fresh: bool, cx: &mut Context<Self>) {
        if self.branch_tab.loading {
            return;
        }
        self.branch_tab.loading = true;
        let cached = self.branch_tab.github.clone().filter(|(at, _)| !fresh && at.elapsed() < GITHUB_FOR);
        let root = self.root.clone();
        cx.spawn(async move |this, cx| {
            let ask = cached.is_none();
            let (github, overview) = cx
                .background_spawn(async move {
                    let github = match cached {
                        Some((_, g)) => g,
                        None => cleanup::github_info(&root),
                    };
                    let overview = cleanup::overview(&root, github.as_ref());
                    (github, overview)
                })
                .await;
            this.update(cx, |this, cx| {
                let tab = &mut this.branch_tab;
                tab.loading = false;
                if ask {
                    tab.github = Some((Instant::now(), github));
                }
                match overview {
                    Ok(o) => {
                        let listed: Vec<String> = o.branches.iter().map(|r| r.branch.name.clone()).collect();
                        tab.picks.keep_only(&listed);
                        tab.overview = Some(o);
                    }
                    Err(e) => this.error = Some(e),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn render_branches(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx).clone();
        let Some(overview) = self.branch_tab.overview.clone() else {
            return div().p_3().text_sm().text_color(t.text_placeholder).child("Reading branches…").into_any_element();
        };
        let shown: Vec<String> = overview.branches.iter().map(|r| r.branch.name.clone()).collect();
        let (local, remote): (Vec<&BranchRow>, Vec<&BranchRow>) =
            overview.branches.iter().partition(|r| !r.branch.remote);
        let mut list: Vec<AnyElement> = Vec::new();
        list.push(section("Local branches", local.len(), &t, cx));
        list.extend(local.iter().map(|r| self.branch_tab_row(r, &overview, &shown, &t, cx)));
        if !remote.is_empty() {
            list.push(section("Remote only", remote.len(), &t, cx));
            list.extend(remote.iter().map(|r| self.branch_tab_row(r, &overview, &shown, &t, cx)));
        }
        list.push(self.worktrees_header(&overview, &t, cx));
        list.extend(overview.worktrees.iter().enumerate().map(|(i, w)| self.worktree_row(i, w, &t, cx)));

        let scroll = scrollbar::handle(gpui::ElementId::Name(format!("git-branches:{}", self.root.display()).into()));
        let area = div()
            .id("git-branches")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&scroll)
            .px_1p5()
            .pb_2()
            .children(list);
        div()
            .flex()
            .flex_col()
            .size_full()
            .child(self.branches_toolbar(&overview, cx))
            .children(self.naming_bar(window, cx))
            .child(scrollbar::vertical_over("git-branches-bar", scroll, area, |d| d.flex_1().min_h_0()))
            .into_any_element()
    }

    /// Clean up merged…, and what to do with the picked branches.
    fn branches_toolbar(&mut self, overview: &Overview, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = theme(cx).clone();
        let finished = cleanup::cleanup(overview);
        let picked = self.branch_tab.picks.ids.len();
        div()
            .flex()
            .flex_none()
            .flex_wrap()
            .items_center()
            .gap_1()
            .p_1p5()
            .border_b_1()
            .border_color(t.border_variant)
            .when(picked == 0, |d| {
                let n = finished.len();
                d.child(
                    ghost_button("git-cleanup", cx)
                        .when(n == 0, |b| b.opacity(0.5))
                        .child(icon(Icon::Trash, if n > 0 { t.text_muted } else { t.text_placeholder }).size_3p5())
                        .child(if n > 0 { format!("Clean up merged ({n})…") } else { "Nothing to clean up".into() })
                        .tooltip(Tooltip::with_meta(
                            "Delete local branches whose work is merged",
                            "Merged or squash-merged, with the remote branch gone or the pull request merged; \
                             never protected or checked-out ones",
                        ))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if !finished.is_empty() {
                                this.ask_delete(finished.clone(), Vec::new(), cx);
                            }
                        })),
                )
            })
            .when(picked > 0, |d| {
                d.child(
                    ghost_button("git-delete-picked", cx)
                        .text_color(t.error)
                        .child(icon(Icon::Trash, t.error).size_3p5())
                        .child(format!("Delete {picked}…"))
                        .on_click(cx.listener(|this, _, _, cx| this.delete_picked(cx))),
                )
                .child(
                    ghost_button("git-clear-picks", cx).text_color(t.text_muted).child("Clear").on_click(cx.listener(
                        |this, _, _, cx| {
                            this.branch_tab.picks.clear();
                            cx.notify();
                        },
                    )),
                )
            })
            .child(div().flex_1())
            .when(self.branch_tab.loading, |d| {
                d.child(crate::ui::spinner("git-branches-loading", t.text_placeholder, px(12.)))
            })
            .when(!overview.github, |d| {
                d.child(div().id("git-no-github").text_xs().text_color(t.text_placeholder).child("no GitHub").tooltip(
                    Tooltip::with_meta(
                        "GitHub isn't answering",
                        "Is gh installed and signed in? Without it there are no pull requests, \
                             and branches count as protected by name (main, master, develop, release/*…)",
                    ),
                ))
            })
    }

    fn branch_tab_row(
        &mut self,
        r: &BranchRow,
        overview: &Overview,
        shown: &[String],
        t: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let b = &r.branch;
        let name = b.name.clone();
        let picked = self.branch_tab.picks.contains(&name);
        let selected = matches!(&self.selected, Some(Selection::Compare(n)) if *n == name);
        let menu_open = self.branch_tab.menu_for.as_deref() == Some(name.as_str());
        let hover = t.hover;
        let id = SharedString::from(format!("git-br:{name}"));

        let mut badges: Vec<AnyElement> = Vec::new();
        if r.default {
            badges.push(badge("default", t.accent, t).into_any_element());
        }
        if r.protected && !r.default {
            badges.push(
                div()
                    .id(SharedString::from(format!("git-br-shield:{name}")))
                    .flex_none()
                    .child(icon(Icon::Shield, t.warning).size_3())
                    .tooltip(Tooltip::text("Protected: Kynex won't delete it"))
                    .into_any_element(),
            );
        }
        if let Some(pr) = &r.pr {
            let (color, word) = match pr.state {
                PrState::Open => (t.success, "open"),
                PrState::Draft => (t.text_muted, "draft"),
                PrState::Merged => (t.accent, "merged"),
                PrState::Closed => (t.error, "closed"),
            };
            let checks = match pr.checks {
                Some(Checks::Passing) => " ✓",
                Some(Checks::Failing) => " ✗",
                Some(Checks::Pending) => " …",
                None => "",
            };
            badges.push(badge(&format!("#{} {word}{checks}", pr.number), color, t).into_any_element());
        }
        match r.merged {
            Merged::Yes if !r.default => badges.push(badge("merged", t.success, t).into_any_element()),
            Merged::Squashed => badges.push(badge("squash merged", t.success, t).into_any_element()),
            _ => {}
        }

        let mut meta: Vec<String> = Vec::new();
        if !b.remote {
            if b.gone {
                meta.push("remote gone".into());
            } else if b.upstream.is_none() {
                meta.push("not published".into());
            }
            if b.ahead > 0 {
                meta.push(format!("↑{}", b.ahead));
            }
            if b.behind > 0 {
                meta.push(format!("↓{}", b.behind));
            }
        }
        if let Merged::No(n) = r.merged {
            meta.push(format!("{n} unmerged"));
        }
        if let Some(w) = &b.worktree {
            meta.push(format!(
                "in {}",
                w.file_name().map_or_else(|| w.display().to_string(), |n| n.to_string_lossy().into())
            ));
        }
        meta.push(ago_unix(b.time));

        let shown = shown.to_vec();
        let click_name = name.clone();
        let menu_name = name.clone();
        let default = overview.default.clone();
        div()
            .id(id)
            .group("git-br-row")
            .relative()
            .flex()
            .flex_col()
            .justify_center()
            .gap_0p5()
            .h(px(46.))
            .px_2()
            .rounded_md()
            .cursor_pointer()
            .child(row_wash(picked || selected, hover, t.active))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .min_w_0()
                    .when(picked, |d| d.child(icon(Icon::Check, t.accent).size_3()))
                    .when(b.current, |d| d.child(icon(Icon::Dot, t.accent).size_3()))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .font_family(mono())
                            .text_sm()
                            .text_color(if b.remote { t.text_muted } else { t.text })
                            .when(b.current, |d| d.font_weight(FontWeight::MEDIUM))
                            .child(name.clone()),
                    )
                    .children(badges)
                    .child(div().flex_1())
                    .child(
                        div()
                            .flex_none()
                            .invisible()
                            .group_hover("git-br-row", |s| s.visible())
                            .when(menu_open, |d| d.visible())
                            .child(
                                icon_button(SharedString::from(format!("git-br-more:{name}")), Icon::ChevronDown, cx)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        let open = this.branch_tab.menu_for.as_deref() == Some(menu_name.as_str());
                                        this.branch_tab.menu_for = (!open).then(|| menu_name.clone());
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap_1p5()
                    .min_w_0()
                    .text_xs()
                    .text_color(t.text_placeholder)
                    .child(div().flex_none().child(meta.join(" · ")))
                    .child(div().min_w_0().truncate().child(b.subject.clone())),
            )
            .tooltip(Tooltip::with_meta(name.clone(), b.subject.clone()))
            .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                let m = ev.modifiers();
                let picks = &mut this.branch_tab.picks;
                if m.shift {
                    picks.extend(&shown, &click_name);
                } else if m.secondary(cx) || picks.on {
                    picks.toggle(&click_name);
                } else if default.as_deref().is_some_and(|d| d != click_name) {
                    this.select(Selection::Compare(click_name.clone()), cx);
                    return;
                }
                cx.notify();
            }))
            .when(menu_open, |d| d.child(self.branch_menu_popover(r, overview, cx)))
            .into_any_element()
    }

    fn branch_menu_popover(&mut self, r: &BranchRow, overview: &Overview, cx: &mut Context<Self>) -> AnyElement {
        let b = r.branch.clone();
        let name = b.name.clone();
        let mut items: Vec<AnyElement> = Vec::new();
        let mut item = |id: &str, label: String, f: MenuAction, cx: &mut Context<Self>| {
            items.push(
                menu_item(SharedString::from(format!("git-br-{id}:{name}")), label, false, cx)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.branch_tab.menu_for = None;
                        f(this, cx);
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        };
        if !b.current && b.worktree.is_none() {
            let branch = b.clone();
            item(
                "switch",
                if b.remote { "Check out".into() } else { "Switch to".into() },
                Box::new(move |this, cx| {
                    let branch = branch.clone();
                    this.run("Switching branch", cx, move |root| git::switch_branch(root, &branch), |_, _| {});
                }),
                cx,
            );
        }
        let start = b.name.clone();
        item(
            "new",
            "New branch from here…".into(),
            Box::new(move |this, cx| this.start_naming(Naming::NewFrom(start.clone()), cx)),
            cx,
        );
        if !b.remote {
            if r.locked().is_none() || b.current {
                let old = b.name.clone();
                item(
                    "rename",
                    "Rename…".into(),
                    Box::new(move |this, cx| this.start_naming(Naming::Rename(old.clone()), cx)),
                    cx,
                );
            }
            if b.upstream.is_none() || b.gone {
                let n = b.name.clone();
                item(
                    "publish",
                    "Publish".into(),
                    Box::new(move |this, cx| {
                        let n = n.clone();
                        this.run("Publishing", cx, move |root| git::publish_branch(root, &n), |_, _| {});
                    }),
                    cx,
                );
            }
        }
        if r.pr.is_some() {
            let n = b.short_name().to_string();
            item(
                "pr",
                "Open pull request".into(),
                Box::new(move |this, cx| {
                    let n = n.clone();
                    this.run("Opening pull request", cx, move |root| git::github::open_pr(root, &n), |_, _| {});
                }),
                cx,
            );
        }
        if let Some(d) = overview.default.clone().filter(|d| *d != b.short_name()) {
            let n = b.name.clone();
            item(
                "compare",
                format!("Compare with {d}"),
                Box::new(move |this, cx| this.select(Selection::Compare(n.clone()), cx)),
                cx,
            );
        }
        if !b.remote && r.locked().is_none() && b.worktree.is_none() {
            let row = r.clone();
            item(
                "delete",
                "Delete…".into(),
                Box::new(move |this, cx| this.ask_delete_rows(std::slice::from_ref(&row), cx)),
                cx,
            );
        }
        let remote = if b.remote { Some(b.name.clone()) } else { b.upstream.clone().filter(|_| !b.gone) };
        if let Some(remote) = remote.filter(|_| !r.protected && !r.default) {
            item(
                "delete-remote",
                format!("Delete {remote} on the remote"),
                Box::new(move |this, _| this.branch_tab.confirm_remote = Some(remote.clone())),
                cx,
            );
        }
        let name = b.name.clone();
        div()
            .absolute()
            .top(px(40.))
            .right_0()
            .child(deferred(
                anchored().snap_to_window_with_margin(gpui::px(8.)).child(
                    popover(cx)
                        .id(SharedString::from(format!("git-br-menu:{name}")))
                        .w(px(240.))
                        .occlude()
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                            this.branch_tab.menu_for = None;
                            cx.notify();
                        }))
                        .children(items),
                ),
            ))
            .into_any_element()
    }

    /// Delete… on the picked branches: the protected and checked-out ones stay out.
    fn delete_picked(&mut self, cx: &mut Context<Self>) {
        let Some(o) = &self.branch_tab.overview else { return };
        let rows: Vec<BranchRow> =
            o.branches.iter().filter(|r| self.branch_tab.picks.contains(&r.branch.name)).cloned().collect();
        self.ask_delete_rows(&rows, cx);
    }

    /// Asks before deleting `rows`, split into finished and unmerged ones.
    fn ask_delete_rows(&mut self, rows: &[BranchRow], cx: &mut Context<Self>) {
        let mut finished = Vec::new();
        let mut unmerged = Vec::new();
        for r in rows.iter().filter(|r| !r.branch.remote && r.locked().is_none() && r.branch.worktree.is_none()) {
            let item = Cleanup {
                branch: r.branch.name.clone(),
                sha: r.branch.sha.clone(),
                remote: r.branch.upstream.clone().filter(|_| !r.branch.gone),
                upstream: r.branch.upstream.clone(),
                worktree: None,
            };
            match r.merged {
                Merged::Yes | Merged::Squashed => finished.push(item),
                Merged::No(n) => unmerged.push((item, n)),
                Merged::Unknown => unmerged.push((item, 0)),
            }
        }
        if finished.is_empty() && unmerged.is_empty() {
            self.error = Some("None of these can be deleted: they're protected or checked out".into());
            cx.notify();
            return;
        }
        self.ask_delete(finished, unmerged, cx);
    }

    pub(super) fn ask_delete(
        &mut self,
        finished: Vec<Cleanup>,
        unmerged: Vec<(Cleanup, usize)>,
        cx: &mut Context<Self>,
    ) {
        self.branch_tab.confirm = Some(Confirm { finished, unmerged, with_remote: false });
        cx.notify();
    }
}

fn section(label: &str, count: usize, t: &Palette, cx: &mut Context<GitView>) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap_1p5()
        .px_2()
        .pt_3()
        .pb_1()
        .child(section_label(label, cx))
        .child(div().text_xs().text_color(t.text_placeholder).child(count.to_string()))
        .into_any_element()
}

pub(super) fn badge(label: &str, color: Hsla, t: &Palette) -> impl IntoElement + use<> {
    div()
        .flex_none()
        .px_1()
        .rounded_sm()
        .border_1()
        .border_color(color.opacity(0.45))
        .text_xs()
        .text_color(color)
        .when(color == t.text_muted, |d| d.border_color(t.border_variant))
        .child(label.to_string())
}
