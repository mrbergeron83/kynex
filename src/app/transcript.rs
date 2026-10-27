//! The conversation: a virtualized `gpui::list` per session that follows the
//! tail while a reply streams, and how each [`Item`] draws. The list's rows
//! come from `kynex_core::transcript::rows`; tool calls draw in `tool_rows.rs`.

use std::rc::Rc;

use gpui::{
    AnyElement, App, Context, FollowMode, FontWeight, ListAlignment, ListState, Pixels, Window, div, img, list,
    prelude::*, px,
};
use kynex_core::live::Key;
use kynex_core::paths::project_name;
use kynex_core::session::{Item, Kind, PermState};
use kynex_core::transcript::{self, Row};
use kynex_core::{commands, fmt};

use super::KynexApp;
use super::state::Active;
use crate::markdown;
use crate::prompt;
use crate::theme::{Palette, label_font, mono, theme};
use crate::ui::selectable::{Spot, selectable};
use crate::ui::{Icon, Tooltip, icon};

/// The transcript and composer's column width.
pub const THREAD_WIDTH: Pixels = px(780.);

/// The transcript list for one conversation, its rows, and how many items
/// they were laid out from.
pub struct Thread {
    pub list: ListState,
    pub(super) rows: Vec<Row>,
    items: usize,
}

impl KynexApp {
    pub(super) fn render_thread(&mut self, cx: &mut Context<Self>) -> AnyElement {
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
            Thread { list, rows: Vec::new(), items: 0 }
        });
        if let Some(rows) = rows {
            let same = thread.rows.iter().zip(&rows).take_while(|(a, b)| a == b).count();
            thread.list.splice(same..thread.rows.len(), rows.len() - same);
            thread.rows = rows;
            thread.items = count;
        }
        // The last row grows while a reply streams in.
        let n = thread.rows.len();
        if busy && n > 0 {
            thread.list.remeasure_items(n - 1..n);
        }
        let state = thread.list.clone();
        list(state, cx.processor(|this, ix, window, cx| this.render_item(ix, window, cx)))
            .flex_1()
            .size_full()
            .py_2()
            .into_any_element()
    }

    /// Scrolls the shown transcript to the bottom and follows new output.
    pub(super) fn follow_tail(&self) {
        if let Some(t) = self.active_key().and_then(|k| self.threads.get(&k)) {
            t.list.scroll_to_end();
            t.list.set_follow_mode(FollowMode::Tail);
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
            .w_full()
            .flex()
            .justify_center()
            .px_6()
            .map(|d| if compact { d.py_0p5() } else { d.py_2() })
            .child(div().w_full().max_w(THREAD_WIDTH).child(body));
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
                .when(!images.is_empty(), |d| {
                    // Previews are the `gpui::Image`s the composer attached.
                    let images = images.iter().filter_map(|i| i.clone().downcast::<gpui::Image>().ok());
                    d.child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_2()
                            .children(images.map(|i| img(i).max_h(px(120.)).max_w(px(240.)).rounded_md())),
                    )
                })
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
            Item::ToolResult { content, is_error, .. } => {
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
        .hover(move |s| s.bg(hover))
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
