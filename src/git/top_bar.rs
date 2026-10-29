//! The Git view's top bar: the branch (a picker to switch or create one), how
//! it stands against its upstream, and fetch / pull / push.

use gpui::{AnyElement, Context, Focusable, FontWeight, SharedString, Window, anchored, deferred, div, prelude::*, px};
use kynex_core::fmt::ago_unix;
use kynex_core::git::{self, Branch, Head, Sync};

use super::GitView;
use crate::theme::{Palette, mono, theme};
use crate::ui::{Icon, Tooltip, ghost_button, icon, icon_button, outline_button, popover, scrollbar, spinner};

impl GitView {
    pub(super) fn render_top_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = theme(cx).clone();
        let status = self.status.clone().unwrap_or_default();
        let head_label = match &status.head {
            Head::Branch(b) => b.clone(),
            Head::Unborn(b) => format!("{b} (no commits yet)"),
            Head::Detached(sha) if sha.is_empty() => "…".into(),
            Head::Detached(sha) => format!("detached at {sha}"),
        };
        let has_branch = matches!(status.head, Head::Branch(_));
        let busy = self.busy.is_some();

        let branch_button = ghost_button("git-branch", cx)
            .font_weight(FontWeight::MEDIUM)
            .child(icon(Icon::GitBranch, t.text_muted).size_3p5())
            .child(div().font_family(mono()).child(head_label))
            .child(icon(Icon::ChevronDown, t.text_placeholder).size_3())
            .tooltip(Tooltip::text("Switch or create a branch"))
            .on_click(cx.listener(|this, _, window, cx| {
                this.branch_menu = !this.branch_menu;
                if this.branch_menu {
                    this.new_branch.read(cx).focus_handle(cx).focus(window, cx);
                }
                cx.notify();
            }));
        let menu = self.branch_menu.then(|| self.branch_menu(window, cx));

        let upstream = status.upstream.clone().map(|u| {
            let in_sync = status.ahead == 0 && status.behind == 0;
            div()
                .flex()
                .items_center()
                .gap_1p5()
                .text_xs()
                .text_color(t.text_placeholder)
                .child(format!("→ {u}"))
                .when(in_sync, |d| d.child("up to date"))
                .when(status.ahead > 0, |d| {
                    d.child(div().text_color(t.info).child(format!("{} to push", status.ahead)))
                })
                .when(status.behind > 0, |d| {
                    d.child(div().text_color(t.warning).child(format!("{} to pull", status.behind)))
                })
        });
        let no_upstream = has_branch && status.upstream.is_none();

        let sync_button = |id: &'static str,
                           which: Icon,
                           label: String,
                           tip: &'static str,
                           op: Sync,
                           highlight: bool,
                           cx: &mut Context<Self>| {
            ghost_button(id, cx)
                .text_color(if highlight { t.text } else { t.text_muted })
                .when(busy, |d| d.opacity(0.5))
                .child(icon(which, if highlight { t.accent } else { t.text_muted }).size_3p5())
                .child(label)
                .tooltip(Tooltip::text(tip))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.run(op.label(), cx, move |root| git::sync(root, op), |_, _| {});
                }))
        };

        div()
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .h(px(40.))
            .px_2()
            .border_b_1()
            .border_color(t.border_variant)
            .child(div().relative().child(branch_button).children(menu))
            .children(upstream)
            .when(no_upstream, |d| d.child(div().text_xs().text_color(t.text_placeholder).child("not published")))
            .child(div().flex_1())
            .when_some(self.busy.clone(), |d, label| {
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .text_xs()
                        .text_color(t.text_muted)
                        .child(spinner("git-busy", t.accent, px(12.)))
                        .child(format!("{label}…")),
                )
            })
            .child(sync_button(
                "git-fetch",
                Icon::Refresh,
                "Fetch".into(),
                "Get the latest from the remote without changing your files",
                Sync::Fetch,
                false,
                cx,
            ))
            .when(!no_upstream, |d| {
                d.child(sync_button(
                    "git-pull",
                    Icon::ArrowDown,
                    if status.behind > 0 { format!("Pull {}", status.behind) } else { "Pull".into() },
                    "Bring in the remote's new commits",
                    Sync::Pull,
                    status.behind > 0,
                    cx,
                ))
            })
            .when(has_branch, |d| {
                d.child(sync_button(
                    "git-push",
                    Icon::ArrowUp,
                    match (no_upstream, status.ahead) {
                        (true, _) => "Publish".into(),
                        (false, 0) => "Push".into(),
                        (false, n) => format!("Push {n}"),
                    },
                    if no_upstream {
                        "Push this branch to the remote and track it"
                    } else {
                        "Send your commits to the remote"
                    },
                    Sync::Push,
                    no_upstream || status.ahead > 0,
                    cx,
                ))
            })
            .child(
                icon_button("git-refresh", Icon::Refresh, cx)
                    .tooltip(Tooltip::text("Re-read the repository"))
                    .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
            )
    }

    fn branch_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx).clone();
        let focused = self.new_branch.read(cx).is_focused(window);
        let (local, remote): (Vec<Branch>, Vec<Branch>) = self.branches.iter().cloned().partition(|b| !b.remote);
        let mut rows: Vec<AnyElement> = local.iter().enumerate().map(|(i, b)| self.branch_row(i, b, &t, cx)).collect();
        if !remote.is_empty() {
            rows.push(
                div()
                    .px_2()
                    .pt_2()
                    .pb_0p5()
                    .text_xs()
                    .text_color(t.text_placeholder)
                    .child("Remote branches")
                    .into_any_element(),
            );
            rows.extend(remote.iter().enumerate().map(|(i, b)| self.branch_row(1000 + i, b, &t, cx)));
        }
        div()
            .absolute()
            .top(px(28.))
            .left_0()
            .child(deferred(
                anchored().snap_to_window_with_margin(px(8.)).child(
                    popover(cx)
                        .w(px(340.))
                        .occlude()
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                            this.branch_menu = false;
                            cx.notify();
                        }))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_1p5()
                                .p_1()
                                .child(
                                    div()
                                        .flex_1()
                                        .px_2()
                                        .py_1()
                                        .rounded_md()
                                        .border_1()
                                        .border_color(if focused { t.border_focused } else { t.border })
                                        .bg(t.editor)
                                        .text_sm()
                                        .child(self.new_branch.clone()),
                                )
                                .child(
                                    outline_button("git-create-branch", "Create", cx)
                                        .tooltip(Tooltip::with_meta("Create a branch here and switch to it", "Enter"))
                                        .on_click(cx.listener(|this, _, _, cx| this.create_branch(cx))),
                                ),
                        )
                        .child(div().my_1().h(px(1.)).bg(t.border_variant))
                        .child({
                            let scroll = scrollbar::handle("git-branch-list");
                            let area = div()
                                .id("git-branch-list")
                                .max_h(px(320.))
                                .overflow_y_scroll()
                                .track_scroll(&scroll)
                                .children(rows);
                            scrollbar::vertical_over("git-branch-bar", scroll, area, |d| d)
                        }),
                ),
            ))
            .into_any_element()
    }

    fn branch_row(&mut self, i: usize, b: &Branch, t: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let hover = t.hover;
        let branch = b.clone();
        let mut meta = Vec::new();
        if b.gone {
            meta.push("upstream gone".to_string());
        } else {
            if b.ahead > 0 {
                meta.push(format!("↑{}", b.ahead));
            }
            if b.behind > 0 {
                meta.push(format!("↓{}", b.behind));
            }
        }
        meta.push(ago_unix(b.time));
        div()
            .id(("git-branch-row", i))
            .flex()
            .items_center()
            .gap_2()
            .px_2()
            .h(px(28.))
            .rounded_md()
            .text_sm()
            .when(b.current, |d| d.bg(t.active))
            .when(!b.current, |d| d.cursor_pointer().hover(move |s| s.bg(hover)))
            .child(
                icon(
                    if b.current { Icon::Check } else { Icon::GitBranch },
                    if b.current { t.accent } else { t.text_placeholder },
                )
                .size_3p5(),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_family(mono())
                    .text_color(if b.remote { t.text_muted } else { t.text })
                    .child(b.name.clone()),
            )
            .child(div().flex_none().text_xs().text_color(t.text_placeholder).child(meta.join(" · ")))
            .when(!b.current, |d| {
                d.tooltip(Tooltip::text(SharedString::from(if b.remote {
                    format!("Check out {} as a local branch", b.name)
                } else {
                    format!("Switch to {}", b.name)
                })))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.branch_menu = false;
                    let branch = branch.clone();
                    this.run("Switching branch", cx, move |root| git::switch_branch(root, &branch), |_, _| {});
                }))
            })
            .into_any_element()
    }
}
