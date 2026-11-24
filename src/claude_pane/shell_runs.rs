//! A `!` command the user ran, and its output, as a transcript row
//! (`kynex_core::claude::input::shell`). Running one from the composer comes with it.

use crate::ui::px;
use gpui::{AnyElement, Context, div, prelude::*};
use kynex_core::claude::input::shell::Run;

use super::ClaudePane;
use super::Key;
use crate::theme::{mono, theme};
use crate::ui::selectable::{Spot, selectable};
use crate::ui::spinner;

impl ClaudePane {
    /// A `!` command in the transcript: the command, then what it printed.
    pub(super) fn render_shell_run(&self, key: Key, ix: usize, run: &Run, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx).clone();
        let running = run.output.is_none() && !run.sent;
        let failed = run.output.as_ref().is_some_and(|o| o.failed());
        let head = div()
            .flex()
            .items_start()
            .gap_2()
            .font_family(mono())
            .text_sm()
            .child(div().flex_none().text_color(t.warning).child("!"))
            .child(div().flex_1().min_w_0().child(selectable(
                key,
                Spot::new(ix, 0),
                run.command.clone(),
                t.text_selection,
            )))
            .when(running, |d| d.child(spinner(("shell-run", ix), t.warning, px(12.))))
            .when_some(run.output.as_ref().and_then(|o| o.code).filter(|_| failed), |d, code| {
                d.child(div().flex_none().text_xs().text_color(t.error).child(format!("exit {code}")))
            });
        let output = run.output.as_ref().filter(|o| !o.stdout.is_empty() || !o.stderr.is_empty()).map(|o| {
            div()
                .id(("shell-output", ix))
                .max_h(px(320.))
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap_1()
                .font_family(mono())
                .text_xs()
                .when(!o.stdout.is_empty(), |d| {
                    d.child(div().text_color(t.text_muted).child(selectable(
                        key,
                        Spot::new(ix, 1),
                        o.stdout.clone(),
                        t.text_selection,
                    )))
                })
                .when(!o.stderr.is_empty(), |d| {
                    d.child(div().text_color(if failed { t.error } else { t.text_muted }).child(selectable(
                        key,
                        Spot::new(ix, 2),
                        o.stderr.clone(),
                        t.text_selection,
                    )))
                })
        });
        div()
            .flex()
            .flex_col()
            .gap_2()
            .px_3()
            .py_2()
            .rounded_lg()
            .border_1()
            .border_color(t.border_variant)
            .bg(t.element)
            .child(head)
            .children(output)
            .into_any_element()
    }
}
