//! The right-hand side: a diff as fixed-height rows (file headers, hunk
//! headers, numbered lines) in a virtualized list, under a header for the
//! selected file or commit.

use gpui::{
    AnyElement, Context, FontWeight, Hsla, ListHorizontalSizingBehavior, SharedString, div, prelude::*, px,
    uniform_list,
};
use kynex_core::fmt::ago_unix;
use kynex_core::git::{self, FileDiff, LineKind};

use super::{GitView, Selection};
use crate::theme::{Palette, mono, theme};
use crate::ui::{Icon, Tooltip, icon, outline_button, spinner};

const ROW_HEIGHT: f32 = 20.;

#[derive(Clone)]
enum Row {
    File { path: SharedString, old_path: Option<SharedString>, added: usize, removed: usize },
    Hunk(SharedString),
    Line { kind: LineKind, old: Option<u32>, new: Option<u32>, text: SharedString },
    Note(SharedString),
}

/// A loaded diff, flattened into rows.
#[derive(Default)]
pub struct DiffState {
    rows: Vec<Row>,
    /// The row with the most characters, which sizes the list's width.
    widest: usize,
    added: usize,
    removed: usize,
    loading: bool,
    error: Option<String>,
}

impl DiffState {
    pub fn loading() -> Self {
        Self { loading: true, ..Default::default() }
    }

    pub fn failed(error: String) -> Self {
        Self { error: Some(error), ..Default::default() }
    }

    /// `headers` adds a row naming each file, for diffs with more than one.
    pub fn new(files: &[FileDiff], headers: bool) -> Self {
        let mut rows = Vec::new();
        for f in files {
            if headers {
                rows.push(Row::File {
                    path: f.path.clone().into(),
                    old_path: f.old_path.clone().map(Into::into),
                    added: f.added,
                    removed: f.removed,
                });
            }
            if f.binary {
                rows.push(Row::Note("Binary file".into()));
            } else if f.hunks.is_empty() {
                rows.push(Row::Note(
                    if f.old_path.is_some() { "Renamed, no content changes" } else { "No content changes" }.into(),
                ));
            }
            for h in &f.hunks {
                rows.push(Row::Hunk(h.header.clone().into()));
                rows.extend(h.lines.iter().map(|l| Row::Line {
                    kind: l.kind,
                    old: l.old,
                    new: l.new,
                    // Tabs would render as nothing.
                    text: l.text.replace('\t', "    ").into(),
                }));
            }
        }
        let width = |r: &Row| match r {
            Row::Line { text, .. } | Row::Hunk(text) | Row::Note(text) => text.len(),
            Row::File { path, .. } => path.len(),
        };
        let widest = rows.iter().enumerate().max_by_key(|(_, r)| width(r)).map_or(0, |(i, _)| i);
        Self {
            widest,
            added: files.iter().map(|f| f.added).sum(),
            removed: files.iter().map(|f| f.removed).sum(),
            rows,
            loading: false,
            error: None,
        }
    }
}

impl GitView {
    pub(super) fn render_diff(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx).clone();
        let Some(selection) = self.selected.clone() else {
            let hint = match self.tab {
                super::Tab::Changes => "Select a file to see its changes",
                super::Tab::History => "Select a commit to see what it changed",
            };
            return centered(hint, &t).into_any_element();
        };
        let header = match &selection {
            Selection::File { path, staged } => self.file_header(path, *staged, cx).into_any_element(),
            Selection::Commit(_) => self.commit_header(cx),
        };
        let body = if let Some(e) = &self.diff.error {
            centered(e.clone(), &t).text_color(t.error).into_any_element()
        } else if self.diff.loading {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .child(spinner("diff-loading", t.text_placeholder, px(16.)))
                .into_any_element()
        } else if self.diff.rows.is_empty() {
            centered("No changes", &t).into_any_element()
        } else {
            let rows = self.diff.rows.clone();
            let t2 = t.clone();
            uniform_list("git-diff", rows.len(), move |range, _, _| range.map(|i| render_row(&rows[i], &t2)).collect())
                .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
                .with_width_from_item(Some(self.diff.widest))
                .track_scroll(&self.diff_scroll)
                .flex_1()
                .font_family(mono())
                .text_size(px(12.5))
                .into_any_element()
        };
        div().flex().flex_col().size_full().child(header).child(body).into_any_element()
    }

    fn file_header(&mut self, path: &str, staged: bool, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = theme(cx).clone();
        let (dir, name) = split_path(path);
        let (added, removed) = (self.diff.added, self.diff.removed);
        let file = path.to_string();
        let toggle = outline_button("diff-stage-toggle", if staged { "Unstage" } else { "Stage" }, cx)
            .tooltip(Tooltip::text(if staged {
                "Take this file out of the next commit"
            } else {
                "Add this file to the next commit"
            }))
            .on_click(cx.listener(move |this, _, _, cx| {
                let paths = vec![file.clone()];
                let label = if staged { "Unstaging" } else { "Staging" };
                this.run(
                    label,
                    cx,
                    move |root| if staged { git::unstage(root, &paths) } else { git::stage(root, &paths) },
                    |_, _| {},
                );
            }));
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .h(px(36.))
            .px_3()
            .border_b_1()
            .border_color(t.border_variant)
            .child(div().flex_none().text_sm().font_weight(FontWeight::MEDIUM).child(name))
            .child(div().min_w_0().truncate().text_xs().text_color(t.text_placeholder).child(dir))
            .child(chip(if staged { "Staged" } else { "Unstaged" }, if staged { t.success } else { t.text_muted }, &t))
            .child(div().flex_1())
            .child(counts(added, removed, &t))
            .child(toggle)
    }

    fn commit_header(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx).clone();
        let Some(d) = &self.detail else { return div().into_any_element() };
        let c = &d.commit;
        let sha = c.sha.clone();
        let parents = c.parents.iter().map(|p| p.chars().take(7).collect::<String>()).collect::<Vec<_>>();
        div()
            .flex()
            .flex_col()
            .flex_none()
            .gap_1p5()
            .px_4()
            .py_3()
            .border_b_1()
            .border_color(t.border_variant)
            .child(div().text_base().font_weight(FontWeight::SEMIBOLD).child(c.subject.clone()))
            .when(!d.body.is_empty(), |el| {
                el.child(
                    div()
                        .id("commit-body")
                        .max_h(px(160.))
                        .overflow_y_scroll()
                        .text_sm()
                        .text_color(t.text_muted)
                        .child(d.body.clone()),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_x_2()
                    .gap_y_1()
                    .pt_0p5()
                    .text_xs()
                    .text_color(t.text_muted)
                    .child(div().text_color(t.text).child(c.author.clone()))
                    .child(div().text_color(t.text_placeholder).child(d.email.clone()))
                    .child("·")
                    .child(ago_unix(c.time))
                    .child("·")
                    .child(
                        div()
                            .id("commit-sha")
                            .flex()
                            .items_center()
                            .gap_1()
                            .px_1()
                            .rounded_sm()
                            .cursor_pointer()
                            .hover(|s| s.bg(t.hover))
                            .font_family(mono())
                            .child(c.short.clone())
                            .child(icon(Icon::Copy, t.text_placeholder).size_3())
                            .tooltip(Tooltip::text("Copy the full hash"))
                            .on_click(move |_, _, cx| {
                                cx.write_to_clipboard(gpui::ClipboardItem::new_string(sha.clone()))
                            }),
                    )
                    .when(parents.len() > 1, |el| el.child(format!("merge of {}", parents.join(" + "))))
                    .child(div().flex_1())
                    .child(files_label(&self.diff))
                    .child(counts(self.diff.added, self.diff.removed, &t)),
            )
            .into_any_element()
    }
}

fn files_label(diff: &DiffState) -> String {
    let n = diff.rows.iter().filter(|r| matches!(r, Row::File { .. })).count();
    format!("{n} file{}", if n == 1 { "" } else { "s" })
}

fn render_row(row: &Row, t: &Palette) -> AnyElement {
    let base = div().flex().items_center().h(px(ROW_HEIGHT)).min_w_full().whitespace_nowrap();
    match row {
        Row::File { path, old_path, added, removed } => base
            .mt_px()
            .px_3()
            .gap_2()
            .bg(t.panel)
            .border_t_1()
            .border_color(t.border_variant)
            .font_weight(FontWeight::MEDIUM)
            .text_color(t.text)
            .child(icon(Icon::Code, t.text_muted).size_3())
            .child(match old_path {
                Some(old) => format!("{old} → {path}"),
                None => path.to_string(),
            })
            .child(counts(*added, *removed, t))
            .into_any_element(),
        Row::Hunk(header) => base
            .pl(px(104.))
            .bg(t.info.opacity(0.07))
            .text_color(t.info.opacity(0.85))
            .child(header.clone())
            .into_any_element(),
        Row::Note(text) => {
            base.pl(px(104.)).text_color(t.text_placeholder).italic().child(text.clone()).into_any_element()
        }
        Row::Line { kind, old, new, text } => {
            let (bg, gutter, sign, fg): (Option<Hsla>, Option<Hsla>, &str, Hsla) = match kind {
                LineKind::Added => (Some(t.diff_add), Some(t.success.opacity(0.12)), "+", t.text),
                LineKind::Removed => (Some(t.diff_del), Some(t.error.opacity(0.12)), "-", t.text),
                LineKind::Context => (None, None, " ", t.text_muted),
                LineKind::NoNewline => (None, None, " ", t.text_placeholder),
            };
            let number = |n: Option<u32>| {
                div()
                    .w(px(44.))
                    .h_full()
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_end()
                    .pr_2()
                    .text_color(t.text_placeholder)
                    .when_some(gutter, |d, g| d.bg(g))
                    .child(n.map(|n| n.to_string()).unwrap_or_default())
            };
            base.when_some(bg, |d, bg| d.bg(bg))
                .child(number(*old))
                .child(number(*new))
                .child(
                    div()
                        .w(px(16.))
                        .flex_none()
                        .flex()
                        .justify_center()
                        .text_color(match kind {
                            LineKind::Added => t.success,
                            LineKind::Removed => t.error,
                            _ => t.text_placeholder,
                        })
                        .child(sign),
                )
                .child(
                    div().pr_4().text_color(fg).when(*kind == LineKind::NoNewline, |d| d.italic()).child(text.clone()),
                )
                .into_any_element()
        }
    }
}

/// `+12 −3` in green and red.
pub(super) fn counts(added: usize, removed: usize, t: &Palette) -> impl IntoElement + use<> {
    div()
        .flex()
        .flex_none()
        .gap_1p5()
        .font_family(mono())
        .text_xs()
        .when(added > 0, |d| d.child(div().text_color(t.success).child(format!("+{added}"))))
        .when(removed > 0, |d| d.child(div().text_color(t.error).child(format!("−{removed}"))))
}

pub(super) fn chip(label: &'static str, color: Hsla, t: &Palette) -> impl IntoElement + use<> {
    div()
        .flex_none()
        .px_1p5()
        .rounded_sm()
        .border_1()
        .border_color(t.border_variant)
        .text_xs()
        .text_color(color)
        .child(label)
}

fn centered(text: impl Into<SharedString>, t: &Palette) -> gpui::Div {
    div()
        .flex_1()
        .flex()
        .items_center()
        .justify_center()
        .p_4()
        .text_sm()
        .text_color(t.text_placeholder)
        .child(text.into())
}

/// `src/app/mod.rs` → (`src/app`, `mod.rs`).
pub(super) fn split_path(path: &str) -> (String, String) {
    match path.rsplit_once('/') {
        Some((dir, name)) => (dir.to_string(), name.to_string()),
        None => (String::new(), path.to_string()),
    }
}
