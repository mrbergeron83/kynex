//! The conversation: a virtualized `gpui::list` per session that follows the
//! tail while a reply streams, and how each [`Item`] draws. The list's rows
//! come from `kynex_core::transcript::rows`; tool calls draw in `tool_rows.rs`.

use std::rc::Rc;
use std::time::Instant;

use gpui::{
    AnyElement, App, Context, FollowMode, FontWeight, ListAlignment, ListOffset, ListState, Pixels, ScrollEvent,
    Window, div, list, prelude::*, px,
};
use kynex_core::live::Key;
use kynex_core::motion::Glide;
use kynex_core::paths::project_name;
use kynex_core::session::{Item, Kind, PermState};
use kynex_core::transcript::{self, Row};
use kynex_core::{commands, fmt};

use super::KynexApp;
use super::state::{Active, View};
use crate::markdown;
use crate::prompt;
use crate::theme::{Palette, label_font, mono, theme};
use crate::ui::ease_height::ease_height;
use crate::ui::motion::{self, fade_in, pop_in};
use crate::ui::selectable::{Spot, selectable};
use crate::ui::wash::wash;
use crate::ui::{Icon, Tooltip, ghost_button, icon};

/// The transcript and composer's column width.
pub const THREAD_WIDTH: Pixels = px(780.);

/// The transcript list for one conversation, its rows, and how many items
/// they were laid out from.
pub struct Thread {
    pub list: ListState,
    pub(super) rows: Vec<Row>,
    items: usize,
    /// Smooth scrolling in progress, and when its last frame ran.
    glide: Glide,
    last_frame: Option<Instant>,
    /// Gliding to the end: follow new output once there.
    follow_after: bool,
    /// A turn just finished here while shown: go to its end.
    turn_ended: bool,
}

/// How far ↑ / ↓ scroll the transcript, as a browser's arrow keys do.
const ARROW_STEP: f32 = 40.;

/// A keyboard scroll of the transcript; signs are -1 up, 1 down.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Scroll {
    Line(f32),
    Page(f32),
    Top,
    Bottom,
}

impl Thread {
    /// Following new output at the bottom, or gliding there to.
    pub(super) fn at_latest(&self) -> bool {
        self.list.is_following_tail() || self.follow_after
    }

    /// How far down the list is scrolled, in pixels.
    fn offset(&self) -> f32 {
        -f32::from(self.list.scroll_px_offset_for_scrollbar().y)
    }

    /// Glides row `row` up to the top of the view, when the list knows where
    /// it is (laid out, at or below the top); false means jump instead.
    pub(super) fn glide_to_row(&mut self, row: usize) -> bool {
        let Some(bounds) = self.list.bounds_for_item(row) else { return false };
        let distance = f32::from(bounds.top() - self.list.viewport_bounds().top());
        self.glide.stop();
        self.follow_after = false;
        self.glide.push(distance);
        true
    }

    /// Follows a streaming reply by gliding: gpui's tail following pins the
    /// end on every layout, so the text jumps up as the last row grows. Instead
    /// the view holds where it is through the layout, and glides down by what
    /// was added on the frames after. Once the reply is done (`busy` false)
    /// it glides the rest of the way and follows plainly again.
    fn trail(&mut self, busy: bool) {
        if self.list.is_following_tail() {
            if !busy {
                self.follow_after = false;
                return;
            }
            // Swap the "past the end" anchor for the same spot as an offset,
            // which layout leaves alone.
            let max = self.list.max_offset_for_scrollbar().y;
            self.list.scroll_to(ListOffset { item_ix: 0, offset_in_item: px(0.) });
            self.list.scroll_by(max);
            self.follow_after = true;
        } else if !self.follow_after {
            return;
        }
        let (offset, max) = (self.offset(), f32::from(self.list.max_offset_for_scrollbar().y));
        let gap = max - offset - self.glide.remaining();
        if gap > 0.5 {
            self.glide.push(gap);
        } else if !busy && !self.glide.is_moving() {
            self.follow_after = false;
            self.list.scroll_to_end();
            self.list.set_follow_mode(FollowMode::Tail);
        }
    }

    /// Moves the list this frame's share of the glide, stopping at either end.
    fn glide_frame(&mut self) {
        let now = Instant::now();
        // The first frame after a notch: assume one 120 Hz frame went by.
        let dt = self.last_frame.map_or(1. / 120., |t| now.duration_since(t).as_secs_f32());
        self.last_frame = Some(now);
        let step = self.glide.step(dt);
        let (offset, max) = (self.offset(), f32::from(self.list.max_offset_for_scrollbar().y));
        let moved = (offset + step).clamp(0., max.max(0.)) - offset;
        if moved != 0. {
            self.list.scroll_by(px(moved));
        }
        if moved != step {
            self.glide.stop();
        }
        // Gliding down onto the end (Page Down, a jump) follows new output
        // again, as gpui's own list does; the end grows while a reply streams.
        if step > 0. && offset + moved >= max - 1. {
            self.glide.stop();
            self.follow_after = true;
        }
        if !self.glide.is_moving() {
            self.last_frame = None;
            if std::mem::take(&mut self.follow_after) {
                self.list.scroll_to_end();
                self.list.set_follow_mode(FollowMode::Tail);
            }
        }
    }
}

impl KynexApp {
    pub(super) fn render_thread(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx).clone();
        let key = self.active_key();
        let count = self.active_session().map_or(0, |s| s.items.len());
        let busy = self.active_session().is_some_and(|s| s.busy);
        if count == 0 {
            let msg = match self.saved.active {
                Active::Chat => "Start a conversation. Uses your Claude Code login, no API key needed.",
                Active::Project(_) => "Ask Claude Code to work on this project.",
            };
            // A new conversation in a project starts under its name, in its colour.
            let name = self.active_project().map(|p| {
                let colour = self.project_colour(&p.path, &t).unwrap_or(t.text_muted);
                div()
                    .text_size(px(22.))
                    .font_family(label_font())
                    .font_weight(FontWeight::BOLD)
                    .text_color(colour)
                    .child(project_name(&p.path))
            });
            return div()
                .flex_1()
                .flex()
                .flex_col()
                .gap_2()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(t.text_placeholder)
                .children(name)
                .child(msg)
                .into_any_element();
        }
        let Some(key) = key else { return div().into_any_element() };
        // Lay the rows out again when items come or go; only the rows that
        // changed (usually the last few) are measured again.
        let stale = self.threads.get(&key).is_none_or(|t| t.items != count);
        let rows = stale.then(|| self.active_session().map(|s| transcript::rows(&s.items)).unwrap_or_default());
        let thread = self.threads.entry(key).or_insert_with(|| {
            let list = ListState::new(0, ListAlignment::Top, px(1000.));
            list.set_follow_mode(FollowMode::Tail);
            Thread {
                list,
                rows: Vec::new(),
                items: 0,
                glide: Glide::default(),
                last_frame: None,
                follow_after: false,
                turn_ended: false,
            }
        });
        if let Some(rows) = rows {
            let same = thread.rows.iter().zip(&rows).take_while(|(a, b)| a == b).count();
            let top = thread.list.logical_scroll_top();
            thread.list.splice(same..thread.rows.len(), rows.len() - same);
            // gpui moves a view anchored inside a replaced row to that row's
            // top, so a reply being read or trailed jumped to its start (#88).
            // The row is the same one continued: keep the view where it was.
            if !thread.list.is_following_tail() && (same..thread.rows.len().min(rows.len())).contains(&top.item_ix) {
                thread.list.scroll_to(top);
            }
            thread.rows = rows;
            thread.items = count;
        }
        // The last row grows while a reply streams in.
        let n = thread.rows.len();
        if busy && n > 0 {
            thread.list.remeasure_items(n - 1..n);
        }
        // A turn finished: its end is where to look, even if you'd scrolled
        // up mid-turn to read earlier output.
        if std::mem::take(&mut thread.turn_ended) && !thread.at_latest() {
            thread.glide.stop();
            thread.follow_after = true;
        }
        if (busy || thread.follow_after) && !cx.reduce_motion() {
            thread.trail(busy);
        } else if thread.follow_after && !thread.glide.is_moving() {
            // The reply ended while trailing it, or motion is reduced: back
            // to plain following.
            thread.follow_after = false;
            thread.list.scroll_to_end();
            thread.list.set_follow_mode(FollowMode::Tail);
        }
        if thread.glide.is_moving() {
            thread.glide_frame();
            window.request_animation_frame();
        }
        let state = thread.list.clone();
        let at_latest = thread.at_latest();
        let uid = self.active_session().map_or(0, |s| s.uid as usize);
        let thread = list(state, cx.processor(|this, ix, window, cx| this.render_item(ix, window, cx)))
            .flex_1()
            .size_full()
            .py_2();
        div()
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            // Switching sessions fades the new one in instead of swapping in one frame.
            .child(fade_in(("thread-in", uid), thread, motion::NORMAL))
            .when(!at_latest, |d| d.child(self.render_jump_to_latest(uid, &t, cx)))
            .into_any_element()
    }

    /// The floating "Jump to latest" over a transcript scrolled up.
    fn render_jump_to_latest(&self, uid: usize, t: &Palette, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let button = ghost_button("jump-to-latest", cx)
            .bg(t.elevated)
            .border_1()
            .border_color(t.border)
            .shadow_md()
            .text_xs()
            .text_color(t.text_muted)
            .child(icon(Icon::ArrowDown, t.text_muted).size_3())
            .child("Jump to latest")
            .on_click(cx.listener(|this, _, _, cx| this.follow_tail(cx)));
        div().absolute().bottom_3().left_0().right_0().flex().justify_center().child(pop_in(
            ("jump-to-latest-in", uid),
            button,
            px(6.),
        ))
    }

    /// Scrolls the shown transcript to the bottom and follows new output.
    /// Glides there when animating; jumps when motion is reduced or the
    /// distance isn't known yet.
    pub(super) fn follow_tail(&mut self, cx: &mut Context<Self>) {
        let reduce = cx.reduce_motion();
        let Some(t) = self.active_key().and_then(|k| self.threads.get_mut(&k)) else { return };
        let (offset, max) = (t.offset(), f32::from(t.list.max_offset_for_scrollbar().y));
        if reduce || max <= offset {
            t.glide.stop();
            t.list.scroll_to_end();
            t.list.set_follow_mode(FollowMode::Tail);
        } else {
            t.glide.stop();
            t.glide.push(max - offset);
            t.follow_after = true;
            cx.notify();
        }
    }

    /// Page Up / Page Down, Home / End, ↑ / ↓: scrolls the shown transcript,
    /// gliding like the wheel. A page keeps a little of the last one in view;
    /// reaching the end follows new output again.
    pub(super) fn scroll_keys(&mut self, by: Scroll, cx: &mut Context<Self>) {
        // Not under the Git view or a modal: those keys are theirs.
        let covered = self.git_modal.is_some()
            || self.settings.is_some()
            || self.app_settings.is_some()
            || self.extensions.is_some()
            || self.about_open
            || self.viewer.is_some();
        if covered || self.active_project().is_some_and(|p| p.view == View::Git) {
            return cx.propagate();
        }
        if by == Scroll::Bottom {
            return self.follow_tail(cx);
        }
        let reduce = cx.reduce_motion();
        let Some(t) = self.active_key().and_then(|k| self.threads.get_mut(&k)) else { return cx.propagate() };
        let page = f32::from(t.list.viewport_bounds().size.height) - 2. * ARROW_STEP;
        let distance = match by {
            Scroll::Line(dir) => dir * ARROW_STEP,
            Scroll::Page(dir) => dir * page.max(ARROW_STEP),
            Scroll::Top | Scroll::Bottom => -t.offset(),
        };
        if distance < 0. {
            t.list.pause_following_tail();
        }
        t.follow_after = false;
        if reduce {
            t.glide.stop();
            if by == Scroll::Top {
                t.list.scroll_to(ListOffset { item_ix: 0, offset_in_item: px(0.) });
            } else {
                let max = f32::from(t.list.max_offset_for_scrollbar().y);
                let offset = t.offset();
                t.list.scroll_by(px((offset + distance).clamp(0., max.max(0.)) - offset));
            }
        } else {
            t.glide.push(distance);
        }
        cx.notify();
    }

    /// A turn finished in `key`'s session while it's shown; its transcript
    /// goes to the end on the next frame.
    pub(super) fn turn_ended(&mut self, key: Key) {
        if let Some(t) = self.threads.get_mut(&key) {
            t.turn_ended = true;
        }
    }

    /// Scrolling by hand (wheels glide in `ui::smooth_scroll`) takes over from
    /// a glide to the end or to a timeline step; the list scrolls itself.
    fn scrolled(&mut self, key: Key) {
        if let Some(t) = self.threads.get_mut(&key) {
            t.glide.stop();
            t.follow_after = false;
        }
    }

    /// Measures row `row` of `key`'s transcript again, after it opened or closed.
    pub(super) fn remeasure_row(&self, key: Key, row: usize) {
        if let Some(th) = self.threads.get(&key) {
            th.list.remeasure_items(row..row + 1);
        }
    }

    fn render_item(&mut self, row: usize, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(key) = self.active_key() else { return div().into_any_element() };
        let Some(thread) = self.threads.get(&key) else { return div().into_any_element() };
        let last = row + 1 == thread.rows.len();
        // Tool traffic sits tight together; messages get room to breathe.
        let (body, compact) = match thread.rows.get(row).cloned() {
            Some(Row::Tools(run)) => {
                let live = last && self.active_session().is_some_and(|s| s.busy);
                (self.render_run(&run, row, live, cx), true)
            }
            Some(Row::Item(ix)) => {
                let compact = self.active_session().is_some_and(|s| {
                    matches!(s.items.get(ix), Some(Item::ToolResult { .. } | Item::Permission { .. } | Item::Thinking))
                });
                (self.render_entry(ix, row, cx), compact)
            }
            None => return div().into_any_element(),
        };
        let t = theme(cx).clone();
        let el = div()
            .id(("row", row))
            .w_full()
            .flex()
            .justify_center()
            .px_6()
            .map(|d| if compact { d.py_0p5() } else { d.py_2() })
            // Tool blocks and results ease as they open or shut; messages, which
            // stream, follow the reply's own glide instead.
            .child(div().w_full().max_w(THREAD_WIDTH).map(|d| match compact {
                true => d.child(ease_height(("row-ease", row), body)),
                false => d.child(body),
            }))
            .on_scroll(cx.listener(move |this, _: &ScrollEvent, _, _| this.scrolled(key)));
        // Lit up for a moment when the timeline jumps here.
        self.flash_row(key, row, el, &t)
    }

    /// Item `ix` drawn on its own, as transcript row `row`.
    fn render_entry(&mut self, ix: usize, row: usize, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx).clone();
        let Some(session) = self.active_session() else { return div().into_any_element() };
        let key = session.uid;
        let session_kind = session.kind;
        let Some(item) = session.items.get(ix) else { return div().into_any_element() };
        let expanded = self.expand_tools || self.expanded.contains(&(key, ix));
        let toggle = cx.listener(move |this, _, _, cx| {
            let k = (key, ix);
            if !this.expanded.remove(&k) {
                this.expanded.insert(k);
            }
            this.remeasure_row(key, row);
            cx.notify();
        });

        match item {
            Item::User { text, images } => div()
                .flex()
                .flex_col()
                .gap_2()
                .px_3()
                .py_2()
                .rounded_lg()
                .border_1()
                .border_color(t.border_variant)
                .bg(t.element)
                .when(!images.is_empty(), |d| d.child(self.thumbnails(("user-images", ix).into(), images, 120., cx)))
                .when(!text.is_empty(), |d| match commands::parse(text) {
                    // A slash command: its name as a chip, then whatever it was given.
                    Some((name, args)) => d.child(
                        div()
                            .flex()
                            .items_start()
                            .gap_2()
                            .text_sm()
                            .child(
                                div()
                                    .flex_none()
                                    .px_1p5()
                                    .rounded_md()
                                    .bg(t.active)
                                    .font_family(mono())
                                    .text_color(t.accent)
                                    .child(format!("/{name}")),
                            )
                            .when(!args.is_empty(), |d| {
                                d.child(div().flex_1().min_w_0().child(selectable(
                                    key,
                                    Spot::new(ix, 0),
                                    args.to_string(),
                                    t.selection,
                                )))
                            }),
                    ),
                    None => {
                        d.child(div().text_sm().child(selectable(key, Spot::new(ix, 0), text.clone(), t.selection)))
                    }
                })
                .into_any_element(),
            Item::Assistant(text) => {
                let to_copy = text.clone();
                // Commands Claude suggests (```bash / ```powershell) run in the project's terminal.
                let run = (session_kind == Kind::Code).then(|| {
                    let this = cx.entity().downgrade();
                    Rc::new(move |shell, command, window: &mut Window, cx: &mut App| {
                        this.update(cx, |this, cx| this.run_in_terminal(shell, command, window, cx)).ok();
                    }) as markdown::RunCommand
                });
                div()
                    .group("assistant")
                    .relative()
                    .text_sm()
                    .child(markdown::render(("md", ix), text, key, ix, run, cx))
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .right(px(-30.))
                            .invisible()
                            .group_hover("assistant", |s| s.visible())
                            .child(
                                crate::ui::icon_button(("copy-msg", ix), Icon::Copy, cx)
                                    .tooltip(Tooltip::text("Copy message"))
                                    .on_click(move |_, _, cx| {
                                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(to_copy.clone()))
                                    }),
                            ),
                    )
                    .into_any_element()
            }
            Item::Thinking => {
                div().text_sm().italic().text_color(t.text_placeholder).child("Thinking…").into_any_element()
            }
            // Calls are laid out in runs, drawn by `render_run`.
            Item::ToolUse { .. } => div().into_any_element(),
            // A result whose call isn't known.
            Item::ToolResult { content, images, is_error, .. } => {
                let first: String = content.lines().next().unwrap_or("").chars().take(160).collect();
                let color = if *is_error { t.error } else { t.text_placeholder };
                let head = disclosure_row(("result", ix), expanded, &t)
                    .pl_6()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_family(mono())
                            .text_xs()
                            .text_color(color)
                            .child(first),
                    )
                    .on_click(toggle);
                div()
                    .flex()
                    .flex_col()
                    .child(head)
                    .when(!images.is_empty(), |d| {
                        d.child(div().pl_6().pt_1().child(self.thumbnails(
                            ("result-images", ix).into(),
                            images,
                            160.,
                            cx,
                        )))
                    })
                    .when(expanded, |d| d.child(code_box(("result-body", ix).into(), content.clone(), (key, ix), &t)))
                    .into_any_element()
            }
            // Answered in the prompt that takes the composer's place; the
            // transcript keeps a one-line record.
            Item::Permission { tool, input, state, .. } => {
                let (which, note, color) = match state {
                    PermState::Pending => (Icon::Dot, "waiting for you below".to_string(), t.warning),
                    PermState::Allowed(n) => (Icon::Check, n.clone(), t.success),
                    PermState::Denied(n) => (Icon::Close, n.clone(), t.error),
                };
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_1()
                    .text_xs()
                    .child(icon(which, color).size_3p5())
                    .child(div().text_color(t.text_muted).child(prompt::title(tool, input)))
                    .child(div().text_color(color).child(note))
                    .into_any_element()
            }
            // A clear end to the turn, with a one-line recap of it.
            Item::Result { cost, duration_ms, is_error, tools, files } => {
                let (which, color, label) =
                    if *is_error { (Icon::Close, t.error, "Stopped") } else { (Icon::Check, t.success, "Finished") };
                div()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .px_1()
                    .pt_1()
                    .text_xs()
                    .child(icon(which, color).size_3p5().flex_none())
                    .child(div().flex_none().font_weight(FontWeight::MEDIUM).text_color(color).child(label))
                    .child(div().min_w_0().truncate().text_color(t.text_muted).child(fmt::recap(
                        *duration_ms,
                        *tools,
                        files,
                        *cost,
                    )))
                    .into_any_element()
            }
            Item::Task(n) => self.render_task_notice(ix, n, cx),
            Item::Error(e) => div()
                .px_1()
                .text_sm()
                .text_color(t.error)
                .child(selectable(key, Spot::new(ix, 0), e.clone(), t.selection))
                .into_any_element(),
        }
    }
}

pub(super) fn disclosure_row(id: impl Into<gpui::ElementId>, expanded: bool, t: &Palette) -> gpui::Stateful<gpui::Div> {
    let hover = t.hover;
    div()
        .id(id)
        .flex()
        .items_center()
        .gap_1p5()
        .h(px(22.))
        .px_1()
        .rounded_md()
        .cursor_pointer()
        .relative()
        .child(wash(hover, None))
        .child(icon(if expanded { Icon::ChevronDown } else { Icon::ChevronRight }, t.text_placeholder).size_3())
}

/// A file edit drawn as a diff, in the same frame as [`code_box`].
pub(super) fn diff_box<E: IntoElement>(id: gpui::ElementId, diff: E, t: &Palette) -> impl IntoElement + use<E> {
    div()
        .id(id)
        .ml_6()
        .mt_1()
        .max_h(px(420.))
        .overflow_y_scroll()
        .rounded_md()
        .border_1()
        .border_color(t.border_variant)
        .bg(t.panel)
        .px_2()
        .py_2()
        .child(diff)
}

/// A tool's input or output, selectable as item `item` of transcript `group`.
pub(super) fn code_box(
    id: gpui::ElementId,
    text: String,
    (group, item): (Key, usize),
    t: &Palette,
) -> impl IntoElement + use<> {
    let text = selectable(group, Spot::new(item, 0), text, t.selection);
    div()
        .id(id)
        .ml_6()
        .mt_1()
        .max_h(px(320.))
        .overflow_y_scroll()
        .rounded_md()
        .border_1()
        .border_color(t.border_variant)
        .bg(t.panel)
        .px_3()
        .py_2()
        .font_family(mono())
        .text_xs()
        .text_color(t.text_muted)
        .child(text)
}
