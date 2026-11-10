//! The conversation: a virtualized `gpui::list` per session that follows the
//! tail while a reply streams, and how each [`Item`] draws. The list's rows
//! come from `kynex_core::transcript::rows`; tool calls draw in `tool_rows.rs`.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Instant;

use crate::ui::px;
use gpui::{
    AnyElement, Context, FollowMode, FontWeight, ListAlignment, ListOffset, ListState, Window, canvas, div, list,
    prelude::*,
};
use kynex_core::live::Key;
use kynex_core::motion::Glide;
use kynex_core::paths::project_name;
use kynex_core::transcript::{self, Row};

use super::KynexApp;
use super::state::{Active, View};
use crate::theme::{Palette, label_font, theme};
use crate::ui::motion::{self, fade_in, pop_in};
use crate::ui::scrollbar;
use crate::ui::{Icon, ghost_button, icon};

/// Room either side of a transcript row (`px_6`). The column's widths are
/// `kynex_core::column`, from the Transcript width setting.
pub(super) const ROW_PADDING: f32 = 24.;

/// The transcript list for one conversation, its rows, and how many items
/// they were laid out from.
pub struct Thread {
    pub list: ListState,
    pub(super) rows: Vec<Row>,
    items: usize,
    /// The session's [`Session::revision`] they were laid out at.
    ///
    /// [`Session::revision`]: kynex_core::session::Session::revision
    revision: u64,
    /// Smooth scrolling in progress, and when its last frame ran.
    glide: Glide,
    last_frame: Option<Instant>,
    /// Gliding to the end: follow new output once there.
    follow_after: bool,
    /// A turn just finished here while shown: go to its end.
    turn_ended: bool,
    /// The list's width from the last layout; 0 before the first.
    width: Rc<Cell<f32>>,
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
            self.list.scroll_to(ListOffset { item_ix: 0, offset_in_item: gpui::px(0.) });
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
        // A held scrollbar thumb has the list; any glide gives way to it.
        if self.list.is_scrollbar_dragging() {
            self.glide.stop();
            self.follow_after = false;
        }
        let now = Instant::now();
        // The first frame after a notch: assume one 120 Hz frame went by.
        let dt = self.last_frame.map_or(1. / 120., |t| now.duration_since(t).as_secs_f32());
        self.last_frame = Some(now);
        let step = self.glide.step(dt);
        let (offset, max) = (self.offset(), f32::from(self.list.max_offset_for_scrollbar().y));
        let moved = (offset + step).clamp(0., max.max(0.)) - offset;
        if moved != 0. {
            self.list.scroll_by(gpui::px(moved));
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
    pub(super) fn render_thread(&mut self, key: Key, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx).clone();
        let count = self.session_by_key(key).map_or(0, |s| s.items.len());
        let busy = self.session_by_key(key).is_some_and(|s| s.busy);
        // A session opening: its log is read back off this thread, newest turns first.
        if count == 0 && self.session_by_key(key).is_some_and(|s| s.loading()) {
            return div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(t.text_placeholder)
                .child(fade_in("transcript-loading", div().child("Loading the conversation…"), motion::NORMAL))
                .into_any_element();
        }
        if count == 0 {
            return self.render_empty_thread(cx);
        }
        if self.session_by_key(key).is_none() {
            return div().into_any_element();
        }
        self.render_thread_rows(key, count, busy, window, cx)
    }

    /// What the transcript area shows with nothing in it: the prompt to start.
    pub(super) fn render_empty_thread(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx).clone();
        let on_openrouter = self.active_provider().is_openrouter();
        let on_grok = self.active_provider() == kynex_core::provider::Provider::Grok;
        let chat_only = self.active_provider() == kynex_core::provider::Provider::OpenRouterChat;
        let msg = match self.saved.active {
            _ if chat_only => format!(
                "Chat with {} through OpenRouter. Chat only: it can't see, run or change anything here.",
                self.active_model_name()
            ),
            Active::Chat if on_openrouter => {
                format!("Start a conversation. Runs on {} through OpenRouter.", self.active_model_name())
            }
            Active::Chat if on_grok => format!("Start a conversation with {}.", self.active_model_name()),
            Active::Chat => "Start a conversation. Uses your Claude Code login, no API key needed.".to_string(),
            Active::Project(_) if on_openrouter => {
                format!("Ask Claude Code to work on this project, on {} through OpenRouter.", self.active_model_name())
            }
            Active::Project(_) if on_grok => {
                format!("Ask Grok to work on this project, on {}.", self.active_model_name())
            }
            Active::Project(_) => "Ask Claude Code to work on this project.".to_string(),
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
        div()
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
            .into_any_element()
    }

    /// The transcript list of session `key`, with its pulse and scrollbar.
    fn render_thread_rows(
        &mut self,
        key: Key,
        count: usize,
        busy: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx).clone();
        self.tick_clocks(cx);
        // Lay the rows out again when items come or go, or one changed how it
        // lays out; only the rows that changed (usually the last few) are measured again.
        let revision = self.session_by_key(key).map_or(0, |s| s.revision());
        let stale = self.threads.get(&key).is_none_or(|t| t.items != count || t.revision != revision);
        let rows = stale.then(|| self.session_by_key(key).map(|s| transcript::rows(&s.items)).unwrap_or_default());
        let thread = self.threads.entry(key).or_insert_with(|| {
            let list = ListState::new(0, ListAlignment::Top, gpui::px(1000.));
            list.set_follow_mode(FollowMode::Tail);
            Thread {
                list,
                rows: Vec::new(),
                items: 0,
                revision: 0,
                glide: Glide::default(),
                last_frame: None,
                follow_after: false,
                turn_ended: false,
                width: Rc::default(),
            }
        });
        if let Some(rows) = rows {
            let (old, new) = (thread.rows.len(), rows.len());
            let same = thread.rows.iter().zip(&rows).take_while(|(a, b)| a == b).count();
            // A row that changed in place (a reply streaming, a tool run
            // growing) is the same row continued: measure it again. Splicing
            // it would drop its height until the next layout, so the end of
            // the list moved up for a frame and a glide there jumped back
            // (#105); and a view anchored in it went to its top (#88).
            thread.list.remeasure_items(same..old.min(new));
            match new.cmp(&old) {
                std::cmp::Ordering::Greater => thread.list.splice(old..old, new - old),
                std::cmp::Ordering::Less => thread.list.splice(new..old, 0),
                std::cmp::Ordering::Equal => {}
            }
            thread.rows = rows;
            thread.items = count;
            thread.revision = revision;
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
        let (state, width) = (thread.list.clone(), thread.width.clone());
        let at_latest = thread.at_latest();
        let uid = key as usize;
        // No padding on the list itself: gpui scrolls past it but leaves it
        // out of `max_offset_for_scrollbar`, so the glides here stopped short
        // of the end and pulled back from it (#105).
        // The list's width, for the rows' columns. Rows can't ask the list (it's
        // borrowed while it lays them out), so this reads it from layout first,
        // and has the rows measured again when it changes.
        let measure = state.clone();
        let probe = canvas(
            move |bounds, window, _| {
                let w = f32::from(bounds.size.width);
                if (width.get() - w).abs() >= 0.5 {
                    width.set(w);
                    measure.remeasure();
                    window.refresh();
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0();
        let bar = scrollbar::vertical(("thread-bar", uid), state.clone());
        let pulse = busy.then(|| self.render_pulse(key, &t));
        let thread =
            list(state, cx.processor(move |this, ix, _, cx| this.render_item(key, ix, cx))).flex_1().size_full();
        div()
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .py_2()
            .child(probe)
            // Switching sessions fades the new one in instead of swapping in one frame.
            .child(fade_in(("thread-in", uid), thread, motion::NORMAL))
            .children(pulse)
            .child(bar)
            .when(!at_latest, |d| d.child(self.render_jump_to_latest(uid, &t, cx)))
            .into_any_element()
    }

    /// One quiet line under the transcript while Claude works, saying what's
    /// going on when nothing above shows it: "Starting Claude…", "Waiting for
    /// Claude… 3s", "Working… 5s" (#157, #177). Text only, no spinner (#115).
    /// Its room stays for the whole turn, so the transcript doesn't hop as
    /// steps start and end.
    fn render_pulse(&self, key: Key, t: &Palette) -> impl IntoElement + use<> {
        let label = self.session_by_key(key).and_then(|s| {
            let who = match s.provider().provider() {
                kynex_core::provider::Provider::Grok => "Grok",
                _ => "Claude",
            };
            s.pulse().map(|p| p.label(who))
        });
        let available = self.threads.get(&key).map_or(0., |th| th.width.get());
        // Measured in pixels; the columns are in sizes at 100%, which `px` scales back.
        let available = available / self.saved.zoom;
        let column = (available > 0.).then(|| self.saved.column.columns(available - ROW_PADDING * 2.));
        let line = div()
            .flex()
            .items_center()
            .h(px(22.))
            .px_1()
            .text_xs()
            .italic()
            .text_color(t.text_placeholder)
            .children(label)
            .map(|d| match column {
                Some(c) => d.flex_none().w(px(c.prose)),
                None => d.w_full().when_some(self.saved.column.max_prose(), |d, w| d.max_w(px(w))),
            });
        div().flex_none().w_full().flex().justify_center().px(px(ROW_PADDING)).child(line)
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
            .occlude()
            .on_click(cx.listener(|this, _, _, cx| this.follow_tail(cx)));
        div().absolute().bottom_3().left_0().right_0().flex().justify_center().child(pop_in(
            ("jump-to-latest-in", uid),
            button,
            gpui::px(6.),
        ))
    }

    /// Scrolls the shown transcript to the bottom and follows new output.
    /// Glides there when animating; jumps when motion is reduced or the
    /// distance isn't known yet.
    pub(super) fn follow_tail(&mut self, cx: &mut Context<Self>) {
        let reduce = cx.reduce_motion();
        let Some(t) = self.focused_key().and_then(|k| self.threads.get_mut(&k)) else { return };
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
        let Some(t) = self.focused_key().and_then(|k| self.threads.get_mut(&k)) else { return cx.propagate() };
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
                t.list.scroll_to(ListOffset { item_ix: 0, offset_in_item: gpui::px(0.) });
            } else {
                let max = f32::from(t.list.max_offset_for_scrollbar().y);
                let offset = t.offset();
                t.list.scroll_by(gpui::px((offset + distance).clamp(0., max.max(0.)) - offset));
            }
        } else {
            t.glide.push(distance);
        }
        cx.notify();
    }

    /// `n` items of history came in front of `key`'s transcript. What's keyed by
    /// item moves down with them, and so do the list's rows: the view stays on
    /// what it showed instead of jumping to the oldest message.
    pub(super) fn history_came(&mut self, key: Key, n: usize) {
        let shift = |set: &mut std::collections::HashSet<(Key, usize)>| {
            *set = set.drain().map(|(k, ix)| if k == key { (k, ix + n) } else { (k, ix) }).collect();
        };
        shift(&mut self.expanded);
        shift(&mut self.open_runs);
        // Turns are counted from the first; work them out again.
        self.timelines.remove(&key);
        self.folds.remove(&key);
        let Some((rows, count)) = self.session_for(key).map(|s| (transcript::rows(&s.items), s.items.len())) else {
            return;
        };
        let Some(thread) = self.threads.get_mut(&key) else { return };
        let added = rows.len().saturating_sub(thread.rows.len());
        thread.list.splice(0..0, added);
        thread.rows = rows;
        thread.items = count;
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
    pub(super) fn scrolled(&mut self, key: Key) {
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

    /// The column's widths in the list's width, less the row's padding. Until
    /// the list has been laid out once its width isn't known: `None`.
    pub(super) fn row_columns(&self, key: Key) -> Option<kynex_core::column::Columns> {
        let available = self.threads.get(&key).map_or(0., |th| th.width.get()) / self.saved.zoom;
        (available > 0.).then(|| self.saved.column.columns(available - ROW_PADDING * 2.))
    }
}
