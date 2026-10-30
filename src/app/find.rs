//! Find in the transcript (Ctrl+F): a small bar over the conversation's top
//! right with the query, "3 of 12" and previous / next. Matches are washed in
//! the text (`ui::find_marks`), the current one stronger; going to one opens
//! the folded run or tool output it's in and scrolls it into view.

use std::rc::Rc;

use crate::ui::px;
use gpui::{AnyElement, Context, Entity, Focusable, Window, div, prelude::*};
use kynex_core::find::{self, Hit};
use kynex_core::live::Key;
use kynex_core::session::Item;
use kynex_core::transcript::{Row, row_of};

use super::KynexApp;
use super::keys::{FindInTranscript, Interrupt};
use crate::theme::theme;
use crate::ui::input::{self, TextInput};
use crate::ui::motion::{self, fade_in};
use crate::ui::{Icon, Tooltip, find_marks, icon, icon_button};

pub(super) struct FindBar {
    input: Entity<TextInput>,
    /// What `hits` were found for: the query, session and its revision.
    query: String,
    key: Option<Key>,
    revision: (usize, u64),
    hits: Rc<Vec<Hit>>,
    current: usize,
}

impl KynexApp {
    /// Ctrl+F: opens the bar, or puts the keyboard back in it.
    pub(super) fn open_find(&mut self, _: &FindInTranscript, window: &mut Window, cx: &mut Context<Self>) {
        if self.active_session().is_none_or(|s| s.items.is_empty()) {
            return cx.propagate();
        }
        let input = match &self.find {
            Some(f) => f.input.clone(),
            None => {
                let input = cx.new(|cx| TextInput::new(cx).placeholder("Find in conversation"));
                self.find = Some(FindBar {
                    input: input.clone(),
                    query: String::new(),
                    key: None,
                    revision: (0, 0),
                    hits: Rc::default(),
                    current: 0,
                });
                input
            }
        };
        input.read(cx).focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.find = None;
        find_marks::clear(cx);
        self.focus_input(window, cx);
        cx.notify();
    }

    /// Finds again when the query or the conversation changed; a new query
    /// goes to its first match from the top of the view down.
    fn refresh_find(&mut self, cx: &mut Context<Self>) {
        let key = self.active_key();
        let (count, revision) = self.active_session().map_or((0, 0), |s| (s.items.len(), s.revision()));
        let top = key.and_then(|k| self.threads.get(&k)).map(|th| {
            let row = th.list.logical_scroll_top().item_ix;
            th.rows.get(row).map_or(0, first_item)
        });
        let Some(bar) = &mut self.find else { return };
        let query = bar.input.read(cx).text().to_string();
        let new_query = bar.query != query || bar.key != key;
        if !new_query && bar.revision == (count, revision) {
            return;
        }
        let hits = Rc::new(find::find(self.code_items(), &query));
        let bar = self.find.as_mut().unwrap();
        bar.hits = hits;
        bar.revision = (count, revision);
        bar.key = key;
        bar.query = query;
        if new_query {
            let from = top.unwrap_or(0);
            bar.current = bar.hits.iter().position(|h| h.item >= from).unwrap_or(0);
            if let Some(&hit) = bar.hits.get(bar.current) {
                self.reveal(hit, cx);
            }
        } else {
            bar.current = bar.current.min(bar.hits.len().saturating_sub(1));
        }
    }

    /// The shown session's items, or none.
    fn code_items(&self) -> &[Item] {
        self.active_session().map_or(&[], |s| &s.items)
    }

    /// Goes `by` matches on (back if negative), round the ends.
    fn find_step(&mut self, by: isize, cx: &mut Context<Self>) {
        let Some(bar) = &mut self.find else { return };
        let n = bar.hits.len() as isize;
        if n == 0 {
            return;
        }
        bar.current = (bar.current as isize + by).rem_euclid(n) as usize;
        let hit = bar.hits[bar.current];
        self.reveal(hit, cx);
        cx.notify();
    }

    /// Opens whatever folds `hit` (a run of tool calls, a call's output, a
    /// thought) and scrolls its row into view.
    fn reveal(&mut self, hit: Hit, cx: &mut Context<Self>) {
        let Some(key) = self.active_key() else { return };
        let Some(th) = self.threads.get(&key) else { return };
        let Some(row) = row_of(&th.rows, hit.item) else { return };
        match &th.rows[row] {
            Row::Tools(run) => {
                self.open_runs.insert((key, run.first()));
                if let Some(call) = run.calls.iter().find(|c| c.result == Some(hit.item) || c.ix == hit.item) {
                    self.expanded.insert((key, call.ix));
                }
            }
            Row::Item(_) => {
                self.expanded.insert((key, hit.item));
            }
        }
        self.remeasure_row(key, row);
        self.jump_to_item(hit.item, cx);
    }

    /// The bar, while it's open over a conversation.
    pub(super) fn render_find(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.find.as_ref()?;
        self.refresh_find(cx);
        let t = theme(cx).clone();
        let key = self.active_key();
        let bar = self.find.as_ref()?;
        let (n, current) = (bar.hits.len(), bar.current);
        let current_hit = bar.hits.get(current).map(|h| (h.item, h.nth));
        let colors = (t.warning.opacity(0.25), t.warning.opacity(0.6));
        match key {
            Some(k) if !bar.query.trim().is_empty() => find_marks::set(k, &bar.query, current_hit, colors, cx),
            _ => find_marks::clear(cx),
        }
        let count = match (n, bar.query.trim().is_empty()) {
            (_, true) => String::new(),
            (0, false) => "No results".into(),
            _ => format!("{} of {n}", current + 1),
        };
        let input = bar.input.clone();
        let el = div()
            .absolute()
            .top_2()
            .right(px(24.))
            .flex()
            .items_center()
            .gap_1()
            .h(px(34.))
            .pl_2()
            .pr_1()
            .rounded_lg()
            .border_1()
            .border_color(t.border)
            .bg(t.elevated)
            .shadow_lg()
            .occlude()
            .capture_action(cx.listener(|this, _: &input::Enter, _, cx| {
                this.find_step(1, cx);
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(|this, _: &input::Newline, _, cx| {
                this.find_step(-1, cx);
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(|this, _: &Interrupt, window, cx| {
                this.close_find(window, cx);
                cx.stop_propagation();
            }))
            .child(icon(Icon::Search, t.text_muted).size_3p5().flex_none())
            .child(div().w(px(220.)).text_sm().child(input))
            .child(
                div()
                    .flex_none()
                    .min_w(px(64.))
                    .text_xs()
                    .text_color(if n == 0 { t.text_placeholder } else { t.text_muted })
                    .child(count),
            )
            .child(
                icon_button("find-prev", Icon::ArrowUp, cx)
                    .tooltip(Tooltip::with_meta("Previous match", "Shift+Enter"))
                    .on_click(cx.listener(|this, _, _, cx| this.find_step(-1, cx))),
            )
            .child(
                icon_button("find-next", Icon::ArrowDown, cx)
                    .tooltip(Tooltip::with_meta("Next match", "Enter"))
                    .on_click(cx.listener(|this, _, _, cx| this.find_step(1, cx))),
            )
            .child(
                icon_button("find-close", Icon::Close, cx)
                    .tooltip(Tooltip::with_meta("Close", "Esc"))
                    .on_click(cx.listener(|this, _, window, cx| this.close_find(window, cx))),
            );
        Some(fade_in("find-bar", el, motion::QUICK).into_any_element())
    }
}

/// The first item a row draws.
fn first_item(row: &Row) -> usize {
    match row {
        Row::Item(ix) => *ix,
        Row::Tools(run) => run.first(),
    }
}
