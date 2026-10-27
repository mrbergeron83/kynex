//! The session timeline beside the transcript: one entry per message you
//! sent, with its recap and the steps worth finding again (edits, commands,
//! subagents, prompts, errors). Clicking one scrolls the transcript there and
//! flashes it; the part in view is marked as you scroll. Alt+↑ / Alt+↓ jump
//! between turns, and each turn can be rewound to. `kynex_core::timeline`
//! builds the entries.

use gpui::{
    Animation, AnimationExt, AnyElement, Context, FontWeight, ListOffset, MouseButton, Window, div, prelude::*, px,
};
use kynex_core::live::Key;
use kynex_core::timeline::{self, StepKind, Turn};
use kynex_core::transcript::row_of;

use super::KynexApp;
use super::keys::{NextTurn, PrevTurn, ToggleTimeline};
use crate::theme::{Palette, theme};
use crate::ui::{Icon, Tooltip, icon, icon_button, section_label};

const WIDTH: f32 = 260.;

/// A transcript row lit up after a jump, until its flash fades.
pub(super) struct Flash {
    pub key: Key,
    pub row: usize,
    /// Bumped per jump, so jumping to the same row again flashes again.
    pub generation: usize,
}

impl KynexApp {
    pub(super) fn toggle_timeline(&mut self, _: &ToggleTimeline, _: &mut Window, cx: &mut Context<Self>) {
        self.saved.timeline_open = !self.saved.timeline_open;
        self.save();
        cx.notify();
    }

    pub(super) fn prev_turn(&mut self, _: &PrevTurn, _: &mut Window, cx: &mut Context<Self>) {
        self.step_turn(false, cx);
    }

    pub(super) fn next_turn(&mut self, _: &NextTurn, _: &mut Window, cx: &mut Context<Self>) {
        self.step_turn(true, cx);
    }

    /// Jumps to the turn after (or before) the one in view.
    fn step_turn(&mut self, forward: bool, cx: &mut Context<Self>) {
        let Some(s) = self.active_session() else { return };
        let turns = timeline::turns(&s.items);
        if turns.is_empty() {
            return;
        }
        let current = self.item_in_view().and_then(|i| timeline::locate(&turns, i)).map(|(t, _)| t);
        let target = match (current, forward) {
            (None, _) => 0,
            (Some(t), true) => (t + 1).min(turns.len() - 1),
            (Some(t), false) => t.saturating_sub(1),
        };
        self.jump_to_item(turns[target].item, cx);
    }

    /// The first item drawn at the top of the shown transcript.
    fn item_in_view(&self) -> Option<usize> {
        let th = self.threads.get(&self.active_key()?)?;
        let row = th.list.logical_scroll_top().item_ix;
        th.rows.get(row.min(th.rows.len().checked_sub(1)?)).map(|r| r.first_item())
    }

    /// Scrolls the transcript to the row drawing `item`, puts it at the top
    /// and flashes it. Glides there when it can, as the wheel does.
    fn jump_to_item(&mut self, item: usize, cx: &mut Context<Self>) {
        let reduce = cx.reduce_motion();
        let Some(key) = self.active_key() else { return };
        let Some(th) = self.threads.get_mut(&key) else { return };
        let Some(row) = row_of(&th.rows, item) else { return };
        if reduce || !th.glide_to_row(row) {
            th.list.scroll_to(ListOffset { item_ix: row, offset_in_item: px(0.) });
        }
        let generation = self.flash.as_ref().map_or(0, |f| f.generation + 1);
        self.flash = Some(Flash { key, row, generation });
        cx.notify();
    }

    /// A row's flash after a jump: the accent, fading out.
    pub(super) fn flash_row(&self, key: Key, row: usize, el: gpui::Stateful<gpui::Div>, t: &Palette) -> AnyElement {
        match &self.flash {
            Some(f) if f.key == key && f.row == row => {
                let accent = t.accent;
                el.with_animation(
                    ("row-flash", f.generation),
                    Animation::new(std::time::Duration::from_millis(1200)).with_easing(gpui::ease_in_out),
                    move |d, a| d.bg(accent.opacity(0.16 * (1. - a))),
                )
                .into_any_element()
            }
            _ => el.into_any_element(),
        }
    }

    /// The panel, when it's open and there's a conversation to outline.
    pub(super) fn render_timeline(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.saved.timeline_open {
            return None;
        }
        let t = theme(cx).clone();
        let s = self.active_session()?;
        let turns = timeline::turns(&s.items);
        let busy = s.busy;
        let here = self.item_in_view().and_then(|i| timeline::locate(&turns, i));

        let header = div()
            .flex()
            .flex_none()
            .items_center()
            .h(px(32.))
            .pl_3()
            .pr_1()
            .border_b_1()
            .border_color(t.border_variant)
            .child(section_label("Timeline", cx).flex_1())
            .child(
                icon_button("close-timeline", Icon::Close, cx)
                    .tooltip(Tooltip::with_meta("Hide the timeline", "Ctrl+Shift+T"))
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_timeline(&ToggleTimeline, window, cx))),
            );

        let body: Vec<AnyElement> = if turns.is_empty() {
            vec![
                div()
                    .px_3()
                    .py_2()
                    .text_sm()
                    .text_color(t.text_placeholder)
                    .child("Your messages and what Claude did for each show up here.")
                    .into_any_element(),
            ]
        } else {
            let last = turns.len() - 1;
            turns
                .iter()
                .enumerate()
                .map(|(n, turn)| {
                    let current = here.filter(|(t, _)| *t == n).map(|(_, step)| step);
                    self.render_turn(n, turn, current, busy && n == last, &t, cx)
                })
                .collect()
        };

        Some(
            div()
                .flex()
                .flex_col()
                .flex_none()
                .w(px(WIDTH))
                .h_full()
                .bg(t.panel)
                .border_l_1()
                .border_color(t.border)
                .child(header)
                .child(div().id("timeline-scroll").flex_1().min_h_0().overflow_y_scroll().py_1().children(body))
                .into_any_element(),
        )
    }

    /// One turn: the message, its recap, and its steps. `current` is set when
    /// the transcript is in this turn, with the step in view if any.
    fn render_turn(
        &self,
        n: usize,
        turn: &Turn,
        current: Option<Option<usize>>,
        working: bool,
        t: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (hover, item) = (t.hover, turn.item);
        let in_turn = current.is_some();
        let on_message = current == Some(None);
        let text = turn.text.clone();
        let head = div()
            .id(("timeline-turn", n))
            .group("timeline-turn")
            .flex()
            .items_start()
            .gap_1p5()
            .mx_1()
            .px_2()
            .py_1()
            .rounded_md()
            .cursor_pointer()
            .hover(move |s| s.bg(hover))
            .when(on_message, |d| d.bg(t.active))
            .child(div().mt(px(5.)).size(px(7.)).flex_none().rounded_full().map(|d| match (in_turn, turn.failed) {
                (_, true) => d.bg(t.error),
                (true, _) => d.bg(t.accent),
                _ => d.border_1().border_color(t.text_placeholder),
            }))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(if in_turn { t.text } else { t.text_muted })
                            .line_clamp(2)
                            .text_ellipsis()
                            .child(turn.prompt.clone()),
                    )
                    .children(
                        turn.recap
                            .clone()
                            .or(working.then(|| "Working…".to_string()))
                            .map(|r| div().text_xs().text_color(t.text_placeholder).truncate().child(r)),
                    ),
            )
            .child(
                icon_button(("timeline-rewind", n), Icon::Undo, cx)
                    .invisible()
                    .group_hover("timeline-turn", |s| s.visible())
                    .tooltip(Tooltip::with_meta("Rewind to here", "Go back to before this message"))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.open_rewind_at(Some(text.clone()), cx);
                    })),
            )
            .on_click(cx.listener(move |this, _, _, cx| this.jump_to_item(item, cx)));

        let steps = turn.steps.iter().enumerate().map(|(i, step)| {
            let (item, selected) = (step.item, current == Some(Some(i)));
            let (which, colour) = match step.kind {
                StepKind::Edit => (Icon::Code, t.text_muted),
                StepKind::Command => (Icon::Terminal, t.text_muted),
                StepKind::Agent => (Icon::Play, t.text_muted),
                StepKind::Plan => (Icon::Check, t.info),
                StepKind::Prompt => (Icon::Shield, t.warning),
                StepKind::Error => (Icon::Dot, t.error),
            };
            div()
                .id(("timeline-step", n * 10_000 + i))
                .flex()
                .items_center()
                .gap_1p5()
                .ml(px(20.))
                .mr_1()
                .px_2()
                .h(px(22.))
                .rounded_md()
                .cursor_pointer()
                .text_xs()
                .text_color(if selected { t.text } else { t.text_muted })
                .hover(move |s| s.bg(hover))
                .when(selected, |d| d.bg(t.active))
                .child(icon(which, colour).size_3().flex_none())
                .child(div().flex_1().min_w_0().truncate().child(step.label.clone()))
                .on_click(cx.listener(move |this, _, _, cx| this.jump_to_item(item, cx)))
        });

        div().flex().flex_col().pb_1().child(head).children(steps).into_any_element()
    }
}
