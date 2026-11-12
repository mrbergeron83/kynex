//! The Git pane's worktree list: one row per worktree with its branch and changes; a click
//! shows that worktree below. A project with a single checkout still shows it.

use crate::ui::px;
use gpui::{AnyElement, Context, div, prelude::*};
use kynex_core::workspace::same_folder;

use super::GitPane;
use crate::git::diff_view::counts;
use crate::theme::theme;
use crate::ui::wash::row_wash;
use crate::ui::{Icon, Tooltip, icon};

impl GitPane {
    pub(super) fn render_worktrees(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx).clone();
        let list = self.worktrees.read(cx).list.clone();
        let mut rows = Vec::new();
        for (i, w) in list.iter().enumerate() {
            let selected = same_folder(&w.path, &self.selected);
            let path = w.path.clone();
            rows.push(
                div()
                    .id(("git-pane-wt", i))
                    .relative()
                    .flex()
                    .items_center()
                    .gap_2()
                    .h(px(26.))
                    .px_2()
                    .rounded_md()
                    .cursor_pointer()
                    .child(row_wash(selected, t.hover, t.active))
                    .child(icon(Icon::GitBranch, t.text_muted).size_3())
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .text_color(if selected { t.text } else { t.text_muted })
                            .child(w.label().to_string()),
                    )
                    .when(!w.main, |d| d.child(div().flex_none().text_xs().text_color(t.info).child("worktree")))
                    .when_some(w.changes.filter(|&n| n > 0), |d, n| {
                        d.child(div().flex_none().text_xs().text_color(t.warning).child(n.to_string()))
                            .when_some(w.lines, |d, (a, r)| d.child(counts(a, r, &t)))
                    })
                    .tooltip(Tooltip::text(w.path.display().to_string()))
                    .on_click(cx.listener(move |this, _, _, cx| this.select(path.clone(), cx)))
                    .into_any_element(),
            );
        }
        if rows.is_empty() {
            let name = self.root.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
            rows.push(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .h(px(26.))
                    .px_2()
                    .rounded_md()
                    .bg(t.active)
                    .child(icon(Icon::GitBranch, t.text_muted).size_3())
                    .child(div().text_sm().child(name))
                    .into_any_element(),
            );
        }
        div()
            .id("git-pane-worktrees")
            .flex_none()
            .max_h(px(150.))
            .overflow_y_scroll()
            .p_1()
            .border_b_1()
            .border_color(t.border_variant)
            .children(rows)
            .into_any_element()
    }
}
