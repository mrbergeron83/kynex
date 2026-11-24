//! Claude's todo list over the composer, as the CLI shows it above its input:
//! every item while Claude works, the one in hand marked; once the turn is over
//! it folds to a line (gone when everything is done), which opens it again.
//! The list is read in `kynex_core::claude::todos`; this file only draws it.

use gpui::{AnyElement, Context, div, prelude::*};

use kynex_core::claude::todos::{self, Status};

use super::{ClaudePane, Key};
use crate::theme::theme;
use crate::ui::{Icon, Tooltip, icon, px, spinner};

/// Rows shown at most; the rest are counted.
const ROWS: usize = 8;

impl ClaudePane {
    pub(super) fn render_todos(&mut self, key: Key, cx: &mut Context<Self>) -> Option<AnyElement> {
        let session = self.session_by_key(key)?;
        let list = session.todos();
        if list.is_empty() {
            return None;
        }
        let busy = session.busy;
        let (done, all) = todos::progress(&list);
        if !busy && done == all {
            return None;
        }
        let open = busy || self.todos_open;
        let t = theme(cx).clone();
        let header = div()
            .id("todos-header")
            .flex()
            .items_center()
            .gap_1()
            .px_1()
            .text_xs()
            .text_color(t.text_placeholder)
            .when(!busy, |d| {
                d.cursor_pointer()
                    .hover(|s| s.text_color(t.text_muted))
                    .tooltip(Tooltip::text(if open { "Fold the todo list" } else { "Show the todo list" }))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.todos_open = !this.todos_open;
                        cx.notify();
                    }))
                    .child(icon(if open { Icon::ChevronDown } else { Icon::ChevronRight }, t.text_placeholder).size_3())
            })
            .child(format!("Todos · {done} / {all} done"));
        if !open {
            return Some(header.into_any_element());
        }
        let rows = list.iter().take(ROWS).enumerate().map(|(i, todo)| {
            let (mark, colour) = match todo.status {
                Status::InProgress if busy => (spinner(("todo-spin", i), t.accent, px(12.)).into_any_element(), t.text),
                Status::InProgress => (icon(Icon::Dot, t.accent).size_3().into_any_element(), t.text),
                Status::Done => (icon(Icon::Check, t.text_placeholder).size_3().into_any_element(), t.text_placeholder),
                Status::Pending => (div().size_3().flex_none().into_any_element(), t.text_muted),
            };
            div()
                .flex()
                .items_center()
                .gap_2()
                .px_1()
                .text_sm()
                .text_color(colour)
                .child(div().w(px(12.)).flex().flex_none().justify_center().child(mark))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .when(todo.status == Status::Done, |d| d.line_through())
                        .child(todo.label().to_string()),
                )
        });
        let hidden = list.len().saturating_sub(ROWS);
        let more = (hidden > 0)
            .then(|| div().px_1().pl(px(24.)).text_xs().text_color(t.text_placeholder).child(format!("{hidden} more")));
        Some(
            div()
                .flex()
                .flex_col()
                .gap_0p5()
                .px_2()
                .py_1()
                .rounded_md()
                .border_1()
                .border_color(t.border_variant)
                .bg(t.panel)
                .child(header)
                .children(rows)
                .children(more)
                .into_any_element(),
        )
    }
}
