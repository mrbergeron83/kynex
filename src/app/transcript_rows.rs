//! How each transcript row draws: a message, a thinking line, a result, a
//! permission note, the turn's end. The list and its scrolling are in
//! `transcript.rs`; tool calls draw in `tool_rows.rs`.

use std::rc::Rc;

use crate::ui::px;
use gpui::{AnyElement, App, Context, FontWeight, ScrollEvent, Window, div, prelude::*};
use kynex_core::live::Key;
use kynex_core::session::{Item, Kind, PermState};
use kynex_core::transcript::Row;
use kynex_core::{commands, fmt, links};

use super::KynexApp;
use super::transcript::ROW_PADDING;
use crate::markdown;
use crate::prompt;
use crate::theme::{Palette, mono, theme};
use crate::ui::ease_height::ease_height;
use crate::ui::scrollbar;
use crate::ui::selectable::{Spot, selectable};
use crate::ui::wash::wash;
use crate::ui::{Icon, Tooltip, icon};

impl KynexApp {
    pub(super) fn render_item(&mut self, key: Key, row: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(thread) = self.threads.get(&key) else { return div().into_any_element() };
        let last = row + 1 == thread.rows.len();
        // Tool traffic sits tight together; messages get room to breathe.
        // Tool calls and results (diffs, output) may use the wider column.
        let (body, compact, wide) = match thread.rows.get(row).cloned() {
            Some(Row::Tools(run)) => {
                let live = last && self.session_by_key(key).is_some_and(|s| s.busy);
                (self.render_run(key, &run, row, live, cx), true, true)
            }
            Some(Row::Item(ix)) => {
                let item = self.session_by_key(key).and_then(|s| s.items.get(ix));
                let compact =
                    matches!(item, Some(Item::ToolResult { .. } | Item::Permission { .. } | Item::Thinking(_)));
                let wide = matches!(item, Some(Item::ToolResult { .. }));
                (self.render_entry(key, ix, row, cx), compact, wide)
            }
            None => return div().into_any_element(),
        };
        let t = theme(cx).clone();
        // Tool blocks and results ease as they open or shut; messages, which
        // stream, follow the reply's own glide instead.
        let body = match compact {
            true => ease_height(("row-ease", row), body).into_any_element(),
            false => body,
        };
        let setting = self.saved.column;
        let inner = match self.row_columns(key) {
            // Tool rows start where prose does and grow to the right (#122).
            Some(c) => div().flex_none().w(px(c.prose)).map(|d| match wide {
                true => d.child(div().flex_none().w(px(c.tools)).child(body)),
                false => d.child(body),
            }),
            None => div().w_full().when_some(setting.max_prose(), |d, w| d.max_w(px(w))).child(body),
        };
        let el = div()
            .id(("row", row))
            .w_full()
            .flex()
            .justify_center()
            .px(px(ROW_PADDING))
            .map(|d| if compact { d.py_0p5() } else { d.py_2() })
            .child(inner)
            .on_scroll(cx.listener(move |this, _: &ScrollEvent, _, _| this.scrolled(key)));
        // Lit up for a moment when the timeline jumps here.
        self.flash_row(key, row, el, &t)
    }

    /// Item `ix` drawn on its own, as transcript row `row`.
    fn render_entry(&mut self, key: Key, ix: usize, row: usize, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx).clone();
        let Some(session) = self.session_by_key(key) else { return div().into_any_element() };
        let session_kind = session.kind;
        let Some(item) = session.items.get(ix) else { return div().into_any_element() };
        let expanded = self.pane.expand_tools || self.expanded.contains(&(key, ix));
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
                                    t.text_selection,
                                )))
                            }),
                    ),
                    None => d.child(div().text_sm().child(selectable(
                        key,
                        Spot::new(ix, 0),
                        text.clone(),
                        t.text_selection,
                    ))),
                })
                .into_any_element(),
            Item::Assistant(text) => {
                // Commands Claude suggests (```bash / ```powershell) run in the project's terminal.
                let run = (session_kind == Kind::Code).then(|| {
                    let this = cx.entity().downgrade();
                    Rc::new(move |shell, command, window: &mut Window, cx: &mut App| {
                        this.update(cx, |this, cx| this.run_in_terminal(shell, command, window, cx)).ok();
                    }) as markdown::RunCommand
                });
                // Code and tables may widen into the tool column (#138).
                let wide = self.row_columns(key).map(|c| c.tools).filter(|_| self.saved.column.max_prose().is_some());
                div()
                    .text_sm()
                    .child(markdown::render_wide(("md", ix), text, key, ix, run, Some(&session.cwd), wide, cx))
                    .into_any_element()
            }
            // "Thinking… 4s" while it streams; after, "Thought for 6s", which
            // opens to the text when the CLI shared it.
            Item::Thinking(thought) => {
                let label = div().text_xs().text_color(t.text_placeholder).child(thought.label());
                // Nothing to open: the same 22 px line, without the chevron.
                let plain = |label: gpui::Div| div().flex().items_center().h(px(22.)).px_1().child(label);
                // Its seconds count up on `tick_clocks`.
                if thought.is_live() {
                    return plain(label.italic()).into_any_element();
                }
                if thought.text.trim().is_empty() {
                    return plain(label).into_any_element();
                }
                let head = disclosure_row(("thought", ix), expanded, &t).child(label).on_click(toggle);
                div()
                    .flex()
                    .flex_col()
                    .child(head)
                    .when(expanded, |d| {
                        d.child(div().pl_6().pr_1().pt_1().text_sm().text_color(t.text_muted).child(selectable(
                            key,
                            Spot::new(ix, 0),
                            thought.text.clone(),
                            t.text_selection,
                        )))
                    })
                    .into_any_element()
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
            // A quiet rule ends the turn: how long it took and what it cost, and
            // a chip per file it edited, each opening that file's diff.
            Item::Result { cost, duration_ms, is_error, tools, files } => {
                const MAX_CHIPS: usize = 6;
                let mut recap = vec![fmt::duration(*duration_ms)];
                if *tools > 0 {
                    recap.push(format!("{tools} tool{}", if *tools == 1 { "" } else { "s" }));
                }
                // The CLI prices every turn as Claude; on another provider that's wrong.
                if session.provider().provider().knows_cost() {
                    recap.push(format!("${cost:.2}"));
                }
                let color = if *is_error { t.error } else { t.text_placeholder };
                let cwd = session.cwd.clone();
                let chips = files.iter().take(MAX_CHIPS).enumerate().map(|(n, f)| {
                    let path = links::resolve(&cwd, f);
                    let cwd = cwd.clone();
                    div()
                        .id(("turn-file", ix * 64 + n))
                        .flex_none()
                        .flex()
                        .items_center()
                        .h(px(18.))
                        .px_1p5()
                        .rounded_sm()
                        .bg(t.element)
                        .cursor_pointer()
                        .hover(|s| s.bg(t.hover).text_color(t.text))
                        .font_family(mono())
                        .text_size(px(11.))
                        .text_color(t.text_muted)
                        .child(fmt::file_name(f).to_string())
                        .tooltip(Tooltip::with_meta("Show its changes", path.display().to_string()))
                        .on_click(cx.listener(move |this, _, _, cx| this.open_git_file(cwd.clone(), path.clone(), cx)))
                });
                let more = files.len().saturating_sub(MAX_CHIPS);
                let rule = || div().flex_1().min_w(px(16.)).h(px(1.)).bg(t.border_variant);
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .pt_1()
                    .text_xs()
                    .child(rule())
                    .when(*is_error, |d| {
                        d.child(div().flex_none().font_weight(FontWeight::MEDIUM).text_color(color).child("Stopped"))
                    })
                    .child(div().flex_none().text_color(color).child(recap.join(" · ")))
                    .when(!files.is_empty(), |d| {
                        d.child(div().flex().flex_wrap().gap_1().children(chips).when(more > 0, |d| {
                            d.child(div().text_color(t.text_placeholder).child(format!("+{more} more")))
                        }))
                    })
                    .child(rule())
                    .into_any_element()
            }
            Item::Task(n) => self.render_task_notice(ix, n, cx),
            Item::Error(e) => div()
                .px_1()
                .text_sm()
                .text_color(t.error)
                .child(selectable(key, Spot::new(ix, 0), e.clone(), t.text_selection))
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
    let scroll = scrollbar::handle(id.clone());
    let bar = gpui::ElementId::NamedChild(std::sync::Arc::new(id.clone()), "bar".into());
    let area = div().id(id).max_h(px(420.)).overflow_scroll().track_scroll(&scroll).py_1().child(diff);
    scrollbar::vertical_over(bar, scroll, area, |d| {
        d.ml_6().mt_1().rounded_md().overflow_hidden().border_1().border_color(t.border_variant).bg(t.panel)
    })
}

/// A tool's input or output, selectable as item `item` of transcript `group`.
pub(super) fn code_box(
    id: gpui::ElementId,
    text: String,
    (group, item): (Key, usize),
    t: &Palette,
) -> impl IntoElement + use<> {
    let text = selectable(group, Spot::new(item, 0), text, t.text_selection);
    let scroll = scrollbar::handle(id.clone());
    let bar = gpui::ElementId::NamedChild(std::sync::Arc::new(id.clone()), "bar".into());
    let area = div()
        .id(id)
        .max_h(px(320.))
        .overflow_y_scroll()
        .track_scroll(&scroll)
        .px_3()
        .py_2()
        .font_family(mono())
        .text_xs()
        .text_color(t.text_muted)
        .child(text);
    scrollbar::vertical_over(bar, scroll, area, |d| {
        d.ml_6().mt_1().rounded_md().overflow_hidden().border_1().border_color(t.border_variant).bg(t.panel)
    })
}
