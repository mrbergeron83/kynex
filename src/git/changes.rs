//! The Changes tab: the commit box, then staged and unstaged files with
//! hover actions (stage, unstage, discard).

use gpui::{AnyElement, Context, FontWeight, Hsla, SharedString, Window, div, prelude::*, px};
use kynex_core::git::{self, Change, FileStatus};

use super::diff_view::split_path;
use super::{GitView, Selection};
use crate::theme::{Palette, mono, theme};
use crate::ui::wash::row_wash;
use crate::ui::{Icon, Tooltip, icon, icon_button, scrollbar, section_label};

impl GitView {
    pub(super) fn render_changes(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = theme(cx).clone();
        let Some(status) = self.status.clone() else {
            return div().p_3().text_sm().text_color(t.text_placeholder).child("Reading the repository…");
        };
        let staged: Vec<FileStatus> = status.staged().cloned().collect();
        let unstaged: Vec<FileStatus> = status.unstaged().cloned().collect();

        let mut list: Vec<AnyElement> = Vec::new();
        if !staged.is_empty() {
            list.push(self.section_header("Staged", staged.len(), true, cx).into_any_element());
            list.extend(staged.iter().map(|f| self.file_row(f, true, cx)));
        }
        if !unstaged.is_empty() {
            list.push(self.section_header("Changes", unstaged.len(), false, cx).into_any_element());
            list.extend(unstaged.iter().map(|f| self.file_row(f, false, cx)));
        }
        if list.is_empty() {
            list.push(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_2()
                    .pt_10()
                    .text_sm()
                    .text_color(t.text_placeholder)
                    .child(icon(Icon::Check, t.success).size_5())
                    .child("Nothing to commit")
                    .child(div().text_xs().child("The working tree matches the last commit."))
                    .into_any_element(),
            );
        }

        div().flex().flex_col().size_full().child(self.commit_box(staged.len(), unstaged.len(), window, cx)).child({
            let scroll = scrollbar::handle(gpui::ElementId::Name(format!("git-files:{}", self.root.display()).into()));
            let area = div()
                .id("git-files")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .track_scroll(&scroll)
                .px_1p5()
                .pb_2()
                .children(list);
            scrollbar::vertical_over("git-files-bar", scroll, area, |d| d.flex_1().min_h_0())
        })
    }

    fn commit_box(
        &mut self,
        staged: usize,
        unstaged: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let t = theme(cx).clone();
        let focused = self.message.read(cx).is_focused(window);
        let has_message = !self.message.read(cx).text().trim().is_empty();
        let busy = self.busy.is_some();
        let (label, count) = match (self.amend, staged, unstaged) {
            (true, _, _) => ("Amend last commit", 0),
            (false, 0, 0) => ("Commit", 0),
            (false, 0, n) => ("Commit all", n),
            (false, n, _) => ("Commit", n),
        };
        let can_commit = has_message && !busy && (self.amend || staged + unstaged > 0);
        let amend = self.amend;
        div()
            .flex()
            .flex_col()
            .flex_none()
            .gap_2()
            .p_2()
            .border_b_1()
            .border_color(t.border_variant)
            .child(
                div()
                    .id("commit-message")
                    .max_h(px(140.))
                    .overflow_y_scroll()
                    .px_2()
                    .py_1p5()
                    .rounded_md()
                    .border_1()
                    .border_color(if focused { t.border_focused } else { t.border })
                    .bg(t.editor)
                    .text_sm()
                    .child(self.message.clone()),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .id("commit-amend")
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .px_1()
                            .h(px(24.))
                            .rounded_md()
                            .cursor_pointer()
                            .text_xs()
                            .text_color(if amend { t.text } else { t.text_muted })
                            .hover(|s| s.bg(t.hover))
                            .child(checkbox(amend, &t))
                            .child("Amend")
                            .tooltip(Tooltip::text("Rewrite the last commit instead of making a new one"))
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_amend(cx))),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("commit")
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .h(px(26.))
                            .px_2p5()
                            .rounded_md()
                            .text_sm()
                            .bg(if can_commit { t.accent } else { t.active })
                            .text_color(if can_commit { t.editor } else { t.text_placeholder })
                            .when(can_commit, |d| d.cursor_pointer().hover(|s| s.opacity(0.9)))
                            .child(
                                icon(Icon::GitCommit, if can_commit { t.editor } else { t.text_placeholder })
                                    .size_3p5(),
                            )
                            .child(label)
                            .when(count > 0, |d| d.child(div().opacity(0.8).child(format!("({count})"))))
                            .tooltip(Tooltip::with_meta(
                                match (has_message, staged) {
                                    (false, _) => "Write a message first",
                                    (true, 0) if !amend => "Stages every change, then commits",
                                    _ => "Commit the staged changes",
                                },
                                "Enter in the message box",
                            ))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if can_commit {
                                    this.commit(cx);
                                }
                            })),
                    ),
            )
    }

    fn section_header(
        &mut self,
        title: &'static str,
        n: usize,
        staged: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let t = theme(cx).clone();
        let (which, tip) = if staged { (Icon::Minus, "Unstage all") } else { (Icon::Plus, "Stage all") };
        div()
            .flex()
            .items_center()
            .gap_1()
            .pl_1p5()
            .pt_2()
            .pb_0p5()
            .child(section_label(title, cx))
            .child(div().px_1().rounded_sm().bg(t.active).text_color(t.text_muted).child(n.to_string()))
            .child(div().flex_1())
            .child(
                icon_button(SharedString::from(format!("all-{title}")), which, cx)
                    .tooltip(Tooltip::text(tip))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let label = if staged { "Unstaging" } else { "Staging" };
                        this.run(
                            label,
                            cx,
                            move |root| if staged { git::unstage_all(root) } else { git::stage_all(root) },
                            |_, _| {},
                        );
                    })),
            )
    }

    fn file_row(&mut self, f: &FileStatus, staged: bool, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx).clone();
        let change = if staged { f.staged } else { f.unstaged }.unwrap_or(Change::Modified);
        let (dir, name) = split_path(&f.path);
        let selected = self.selected == Some(Selection::File { path: f.path.clone(), staged });
        let confirming = !staged && self.confirm_discard.as_deref() == Some(f.path.as_str());
        let id = SharedString::from(format!("{}-{}", if staged { "s" } else { "u" }, f.path));
        let hover = t.hover;
        let path = f.path.clone();
        let tip = match &f.orig_path {
            Some(orig) => format!("{} from {orig}", change.describe()),
            None => change.describe().to_string(),
        };

        let toggle = {
            let path = path.clone();
            icon_button(SharedString::from(format!("{id}-toggle")), if staged { Icon::Minus } else { Icon::Plus }, cx)
                .tooltip(Tooltip::text(if staged { "Unstage" } else { "Stage" }))
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    let paths = vec![path.clone()];
                    let label = if staged { "Unstaging" } else { "Staging" };
                    this.run(
                        label,
                        cx,
                        move |root| if staged { git::unstage(root, &paths) } else { git::stage(root, &paths) },
                        |_, _| {},
                    );
                }))
        };
        let discard = (!staged && change != Change::Conflicted).then(|| {
            let file = f.clone();
            let el = if confirming {
                div()
                    .id(SharedString::from(format!("{id}-confirm")))
                    .flex()
                    .items_center()
                    .h(px(22.))
                    .px_1p5()
                    .rounded_sm()
                    .text_xs()
                    .text_color(t.error)
                    .cursor_pointer()
                    .hover(|s| s.bg(t.hover))
                    .child(if change == Change::Untracked { "Delete?" } else { "Discard?" })
            } else {
                icon_button(SharedString::from(format!("{id}-discard")), Icon::Undo, cx)
            };
            el.tooltip(Tooltip::text(if change == Change::Untracked {
                "Delete this new file"
            } else {
                "Discard these changes"
            }))
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                if this.confirm_discard.as_deref() != Some(file.path.as_str()) {
                    this.confirm_discard = Some(file.path.clone());
                    return cx.notify();
                }
                this.confirm_discard = None;
                let file = file.clone();
                this.run("Discarding", cx, move |root| git::discard(root, &file), |_, _| {});
            }))
        });

        div()
            .id(id)
            .group("git-file")
            .flex()
            .items_center()
            .gap_2()
            .h(px(26.))
            .pl_1p5()
            .pr_1()
            .rounded_md()
            .cursor_pointer()
            .relative()
            .child(row_wash(selected, hover, t.active))
            .child(letter(change, &t))
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap_1p5()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .flex_none()
                            .max_w_full()
                            .truncate()
                            .text_sm()
                            .text_color(if selected { t.text } else { t.text_muted })
                            .when(change == Change::Deleted, |d| d.line_through())
                            .child(name),
                    )
                    .child(div().min_w_0().truncate().text_xs().text_color(t.text_placeholder).child(dir)),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .invisible()
                    .group_hover("git-file", |s| s.visible())
                    .when(confirming, |d| d.visible())
                    .children(discard)
                    .child(toggle),
            )
            .tooltip(Tooltip::with_meta(f.path.clone(), tip))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.confirm_discard = None;
                this.select(Selection::File { path: path.clone(), staged }, cx);
            }))
            .into_any_element()
    }
}

/// The status letter, coloured by kind.
fn letter(change: Change, t: &Palette) -> impl IntoElement + use<> {
    div()
        .w(px(14.))
        .flex_none()
        .flex()
        .justify_center()
        .font_family(mono())
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(change_color(change, t))
        .child(change.letter().to_string())
}

pub(super) fn change_color(change: Change, t: &Palette) -> Hsla {
    match change {
        Change::Added | Change::Untracked => t.success,
        Change::Deleted | Change::Conflicted => t.error,
        Change::Renamed | Change::Copied => t.info,
        Change::Modified | Change::TypeChanged => t.warning,
    }
}

fn checkbox(checked: bool, t: &Palette) -> impl IntoElement + use<> {
    div()
        .size(px(13.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded_sm()
        .border_1()
        .border_color(if checked { t.accent } else { t.border })
        .when(checked, |d| d.bg(t.accent).child(icon(Icon::Check, t.on_accent).size_2p5()))
}
