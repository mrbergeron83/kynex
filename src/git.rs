//! The Git view: branch, changed files, history and a diff, for one project.

use std::path::PathBuf;

use gpui::{Context, FontWeight, SharedString, Window, div, prelude::*, px, uniform_list};

use kynex_core::git::{self, Status};

use crate::theme::{mono, theme};
use crate::ui::{Icon, Tooltip, icon, icon_button};

/// Git runs on the background executor so the UI never waits on it.
pub struct GitView {
    root: PathBuf,
    status: Status,
    selected: Option<String>,
    diff: Vec<SharedString>,
    loading: bool,
}

fn lines(diff: Vec<String>) -> Vec<SharedString> {
    diff.into_iter().map(SharedString::from).collect()
}

impl GitView {
    pub fn new(root: PathBuf, cx: &mut Context<Self>) -> Self {
        let mut view = Self { root, status: Status::default(), selected: None, diff: Vec::new(), loading: false };
        view.refresh(cx);
        view
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.loading = true;
        let root = self.root.clone();
        let selected = self.selected.clone();
        cx.spawn(async move |this, cx| {
            let (status, diff) = cx
                .background_spawn(async move {
                    let status = git::read_status(&root);
                    let diff = selected.map(|f| lines(git::read_diff(&root, &f)));
                    (status, diff)
                })
                .await;
            this.update(cx, |this, cx| {
                this.status = status;
                if let Some(d) = diff {
                    this.diff = d;
                }
                this.loading = false;
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn select(&mut self, file: String, cx: &mut Context<Self>) {
        self.selected = Some(file.clone());
        let root = self.root.clone();
        cx.spawn(async move |this, cx| {
            let diff = cx.background_spawn(async move { lines(git::read_diff(&root, &file)) }).await;
            this.update(cx, |this, cx| {
                this.diff = diff;
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }
}

impl Render for GitView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx).clone();
        let header = div()
            .flex()
            .items_center()
            .gap_2()
            .h(px(36.))
            .px_3()
            .border_b_1()
            .border_color(t.border_variant)
            .child(icon(Icon::GitBranch, t.text_muted))
            .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(self.status.branch.clone()))
            .child(div().flex_1())
            .child(
                icon_button("git-refresh", Icon::Refresh, cx)
                    .tooltip(Tooltip::text("Refresh"))
                    .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
            );

        if let Some(e) = &self.status.error {
            return div()
                .flex()
                .flex_col()
                .size_full()
                .child(header)
                .child(div().p_3().text_sm().text_color(t.error).child(e.clone()));
        }

        let files = self.status.files.clone();
        let selected = self.selected.clone();
        let file_list = uniform_list("git-files", files.len(), {
            let t = t.clone();
            cx.processor(move |_this, range: std::ops::Range<usize>, _window, cx| {
                range
                    .map(|i| {
                        let (status, file) = files[i].clone();
                        let color = match status.trim() {
                            "M" | "MM" => t.warning,
                            "A" | "??" => t.success,
                            "D" => t.error,
                            _ => t.text_muted,
                        };
                        let is_selected = selected.as_deref() == Some(file.as_str());
                        let hover = t.hover;
                        div()
                            .id(("git-file", i))
                            .flex()
                            .items_center()
                            .gap_2()
                            .h(px(24.))
                            .px_3()
                            .text_sm()
                            .cursor_pointer()
                            .hover(move |s| s.bg(hover))
                            .when(is_selected, |d| d.bg(t.active))
                            .child(
                                div()
                                    .w(px(18.))
                                    .flex_none()
                                    .font_family(mono())
                                    .text_xs()
                                    .text_color(color)
                                    .child(status),
                            )
                            .child(div().flex_1().min_w_0().truncate().child(file.clone()))
                            .on_click(cx.listener(move |this, _, _, cx| this.select(file.clone(), cx)))
                    })
                    .collect()
            })
        })
        .flex_1();

        let log = self.status.log.clone();
        let history = uniform_list("git-log", log.len(), {
            let t = t.clone();
            move |range: std::ops::Range<usize>, _window: &mut Window, _cx: &mut gpui::App| {
                range
                    .map(|i| {
                        let (hash, msg) = log[i].split_once(' ').unwrap_or((&log[i], ""));
                        div()
                            .flex()
                            .gap_2()
                            .h(px(22.))
                            .px_3()
                            .items_center()
                            .text_xs()
                            .child(
                                div()
                                    .flex_none()
                                    .font_family(mono())
                                    .text_color(t.text_placeholder)
                                    .child(hash.to_string()),
                            )
                            .child(div().flex_1().min_w_0().truncate().text_color(t.text_muted).child(msg.to_string()))
                    })
                    .collect()
            }
        })
        .flex_1();

        let diff = self.diff.clone();
        let diff_view = if self.selected.is_none() {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(t.text_placeholder)
                .child("Select a file to see its diff")
                .into_any_element()
        } else {
            uniform_list("git-diff", diff.len(), {
                let t = t.clone();
                move |range: std::ops::Range<usize>, _window: &mut Window, _cx: &mut gpui::App| {
                    range
                        .map(|i| {
                            let line = diff[i].clone();
                            let row = div().h(px(20.)).px_3().whitespace_nowrap().child(line.clone());
                            if line.starts_with('+') && !line.starts_with("+++") {
                                row.bg(t.success.opacity(0.1)).text_color(t.success)
                            } else if line.starts_with('-') && !line.starts_with("---") {
                                row.bg(t.error.opacity(0.1)).text_color(t.error)
                            } else if line.starts_with("@@") {
                                row.text_color(t.info)
                            } else {
                                row.text_color(t.text_muted)
                            }
                        })
                        .collect()
                }
            })
            .flex_1()
            .font_family(mono())
            .text_size(px(13.))
            .into_any_element()
        };

        div().flex().flex_col().size_full().child(header).child(
            div()
                .flex()
                .flex_1()
                .min_h_0()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .w(px(320.))
                        .flex_none()
                        .border_r_1()
                        .border_color(t.border_variant)
                        .child(
                            div()
                                .px_3()
                                .py_1p5()
                                .text_xs()
                                .text_color(t.text_muted)
                                .child(format!("Changes ({})", self.status.files.len())),
                        )
                        .child(div().flex().flex_col().h(gpui::relative(0.55)).child(file_list))
                        .child(
                            div()
                                .px_3()
                                .py_1p5()
                                .border_t_1()
                                .border_color(t.border_variant)
                                .text_xs()
                                .text_color(t.text_muted)
                                .child("History"),
                        )
                        .child(div().flex().flex_col().flex_1().min_h_0().child(history)),
                )
                .child(div().flex().flex_col().flex_1().min_w_0().bg(t.editor).child(diff_view)),
        )
    }
}
