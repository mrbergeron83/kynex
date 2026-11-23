//! The chat's timeline beside the transcript: one entry per message you sent,
//! with its recap and the steps worth finding again (edits, commands,
//! subagents, prompts, errors). Clicking one glides the transcript there and
//! flashes it; the part in view is marked as you scroll. Ctrl+Shift+T shows
//! it; Alt+↑ / Alt+↓ jump between turns the same way.
//! `kynex_core::claude::timeline` builds the entries.

use gpui::{Animation, AnimationExt, AnyElement, Context, FontWeight, MouseButton, div, prelude::*};
use kynex_core::claude::runs::ListItem;
use kynex_core::claude::timeline::{self, Row, StepKind, Turn};

use super::ClaudeChatView;
use super::state::LogRow;
use crate::theme::{Palette, theme};
use crate::ui::ease_height::ease_height;
use crate::ui::{Icon, Tooltip, icon, icon_button, px, scrollbar, section_label};

/// The timeline's width beside the chat.
const WIDTH: f32 = 260.;

/// A transcript row lit up after a jump, until its flash fades.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Flash {
    pub row: usize,
    /// Bumped per jump, so jumping to the same row again flashes again.
    pub generation: usize,
}

/// Row `row` as the timeline sees it.
fn describe(row: &LogRow) -> Row<'_> {
    match row {
        LogRow::UserPrompt { text, .. } => Row::UserPrompt(text),
        LogRow::EditRow { path, is_error, .. } => Row::Edit { path, is_error: *is_error },
        LogRow::ToolCall { name, summary, is_error, .. } => Row::ToolCall { name, summary, is_error: *is_error },
        LogRow::PermissionNote { title, allowed, .. } => Row::PermissionNote { title, allowed: *allowed },
        LogRow::Notice { text, is_error } => Row::Notice { text, is_error: *is_error },
        LogRow::TurnEnd { duration_ms, tools, cost_usd, is_error, files } => {
            Row::TurnEnd { duration_ms: *duration_ms, tools: *tools, files, cost_usd: *cost_usd, is_error: *is_error }
        }
        _ => Row::Other,
    }
}

impl ClaudeChatView {
    /// Ctrl+Shift+T: shows or hides the timeline.
    pub(super) fn toggle_timeline(&mut self, cx: &mut Context<Self>) {
        self.timeline_open = !self.timeline_open;
        cx.notify();
    }

    /// Brings the turns up to date with the rows, walking only the last turn again.
    fn update_turns(&mut self) {
        let rows = &self.rows;
        self.timeline.update(rows.len(), |i| describe(&rows[i]));
    }

    /// The first row drawn at the top of the transcript.
    fn row_in_view(&self) -> Option<usize> {
        let top = self.list_state.logical_scroll_top().item_ix;
        self.items.get(top.min(self.items.len().checked_sub(1)?)).map(ListItem::start)
    }

    /// Glides row `row`'s list item up to the top of the view, when the list
    /// knows where it is (laid out); false means jump instead.
    fn glide_to_row(&mut self, row: usize) -> bool {
        let Some(ix) = self.item_of_row(row) else { return false };
        let Some(bounds) = self.list_state.bounds_for_item(ix) else { return false };
        let distance = f32::from(bounds.top() - self.list_state.viewport_bounds().top());
        self.glide.stop();
        self.follow_after = false;
        self.list_state.pause_following_tail();
        self.stick_to_bottom = false;
        self.glide.push(distance);
        true
    }

    /// Scrolls the transcript to row `row`, opening the run it's folded in,
    /// puts it at the top and flashes it. Glides there when it can, as the wheel does.
    pub(super) fn jump_to_row(&mut self, row: usize, cx: &mut Context<Self>) {
        self.open_run_of(row);
        if crate::ui::motion::reduced() || cx.reduce_motion() || !self.glide_to_row(row) {
            self.reveal_row(row);
        }
        let generation = self.flash.map_or(0, |f| f.generation + 1);
        self.flash = Some(Flash { row, generation });
        cx.notify();
    }

    /// `el`, with the accent fading out of it while `row` (or one of `rows`) was just jumped to.
    pub(super) fn flash_rows(&self, rows: std::ops::Range<usize>, el: gpui::Div, t: &Palette) -> AnyElement {
        match self.flash {
            Some(f) if rows.contains(&f.row) => {
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

    /// Opens or folds turn `n`.
    fn toggle_turn(&mut self, n: usize, cx: &mut Context<Self>) {
        self.update_turns();
        let turns = self.timeline.turns();
        let current = self.row_in_view().and_then(|r| timeline::locate(turns, r)).map(|(t, _)| t);
        self.timeline_fold.toggle(turns, n, current);
        cx.notify();
    }

    /// The timeline panel, while it's open.
    pub(super) fn render_timeline(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.timeline_open {
            return None;
        }
        let t = theme(cx).clone();
        self.update_turns();
        let turns = self.timeline.turns();
        let here = self.row_in_view().and_then(|r| timeline::locate(turns, r));
        let fold = &self.timeline_fold;
        let current = here.map(|(t, _)| t);
        let at_latest = self.stick_to_bottom;
        let foldable = turns.iter().any(|t| !t.steps.is_empty());
        let any_open = fold.any_open(turns, current);

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
            .when(!at_latest && !turns.is_empty(), |d| {
                d.child(
                    icon_button("timeline-latest", Icon::ArrowDown, cx)
                        .tooltip(Tooltip::with_meta("Jump to latest", "Back to the end, following new output"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.scroll_to_bottom();
                            cx.notify();
                        })),
                )
            })
            .when(foldable, |d| {
                let (which, label) =
                    if any_open { (Icon::CollapseAll, "Collapse all") } else { (Icon::ExpandAll, "Expand all") };
                d.child(icon_button("timeline-fold-all", which, cx).tooltip(Tooltip::text(label)).on_click(
                    cx.listener(move |this, _, _, cx| {
                        this.timeline_fold.set_all(!any_open);
                        cx.notify();
                    }),
                ))
            })
            .child(
                icon_button("close-timeline", Icon::Close, cx)
                    .tooltip(Tooltip::with_meta(
                        "Hide the timeline",
                        if cfg!(target_os = "macos") { "Cmd+Shift+T" } else { "Ctrl+Shift+T" },
                    ))
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_timeline(cx))),
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
            let busy = self.turn_active;
            turns
                .iter()
                .enumerate()
                .map(|(n, turn)| {
                    let in_view = here.filter(|(t, _)| *t == n).map(|(_, step)| step);
                    let open = fold.is_open(turns, n, current);
                    self.render_turn(n, turn, in_view, open, busy && n == last, &t, cx)
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
                .border_l_1()
                .border_color(t.border)
                .bg(t.panel)
                .child(header)
                .child({
                    let scroll = scrollbar::handle(("claude-timeline-scroll", cx.entity_id().as_u64() as usize));
                    let area = div()
                        .id("claude-timeline-scroll")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .track_scroll(&scroll)
                        .py_1();
                    scrollbar::vertical_over("claude-timeline-bar", scroll, area.children(body), |d| {
                        d.flex_1().min_h_0()
                    })
                })
                .into_any_element(),
        )
    }

    /// One turn: the message, its recap, and its steps while `open`. `current`
    /// is set when the transcript is in this turn, with the step in view if any.
    #[allow(clippy::too_many_arguments)]
    fn render_turn(
        &self,
        n: usize,
        turn: &Turn,
        current: Option<Option<usize>>,
        open: bool,
        working: bool,
        t: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (hover, row) = (t.hover, turn.row);
        // The chevron (or room for one, so the dots line up) opens the steps.
        let slot = div().flex_none().size(px(14.)).mt(px(2.));
        let chevron = if turn.steps.is_empty() {
            slot.into_any_element()
        } else {
            slot.id(("timeline-fold", n))
                .flex()
                .items_center()
                .justify_center()
                .rounded_sm()
                .cursor_pointer()
                .hover(move |s| s.bg(hover))
                .child(icon(if open { Icon::ChevronDown } else { Icon::ChevronRight }, t.text_placeholder).size_3())
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.toggle_turn(n, cx);
                }))
                .into_any_element()
        };
        let in_turn = current.is_some();
        let on_message = current == Some(None);
        let head = div()
            .id(("timeline-turn", n))
            .flex()
            .items_start()
            .gap_1p5()
            .mx_1()
            .pl_1()
            .pr_2()
            .py_1()
            .rounded_md()
            .cursor_pointer()
            .hover(move |s| s.bg(hover))
            .when(on_message, |d| d.bg(t.active))
            .child(chevron)
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
            .on_click(cx.listener(move |this, _, _, cx| this.jump_to_row(row, cx)));

        let steps: Vec<AnyElement> = turn
            .steps
            .iter()
            .enumerate()
            .map(|(i, step)| {
                let (row, selected) = (step.row, current == Some(Some(i)));
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
                    // Icons under the message text, past the chevron and dot.
                    .ml(px(33.))
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
                    .on_click(cx.listener(move |this, _, _, cx| this.jump_to_row(row, cx)))
                    .into_any_element()
            })
            .collect();

        // Folding open or shut eases, rather than shoving the turns below.
        let turn = div().flex().flex_col().pb_1().child(head).when(open, |d| d.children(steps));
        ease_height(("timeline-turn-ease", n), turn).into_any_element()
    }
}
