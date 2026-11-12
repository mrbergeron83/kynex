//! One changed file as a whole: every line of it, the changes tinted, with buttons (and
//! keys) to jump to the previous / next change. A file partly staged can show all its
//! changes, the staged ones or the unstaged ones. Shown in a modal over the window.

use std::path::PathBuf;

use crate::ui::px;
use gpui::{
    AnyElement, Context, EventEmitter, FocusHandle, Focusable, FontWeight, KeyDownEvent, ListHorizontalSizingBehavior,
    ScrollStrategy, SharedString, UniformListScrollHandle, Window, div, prelude::*, uniform_list,
};
use kynex_core::git::{self, DiffSide, FileDiff, Staged};

use crate::git::diff_view::{DiffState, chip, counts, render_row, split_path};
use crate::theme::theme;
use crate::ui::wash::row_wash;
use crate::ui::{Icon, Tooltip, icon_button, scrollbar, spinner};

/// What the view asks of the app.
pub enum FileDiffEvent {
    Close,
}

impl EventEmitter<FileDiffEvent> for FileDiffView {}

/// Rows kept above a change when jumping to it.
const CONTEXT_ABOVE: usize = 3;

pub struct FileDiffView {
    root: PathBuf,
    path: String,
    untracked: bool,
    /// How much of the file is staged: partly, and the sides can be shown apart.
    staged: Staged,
    side: DiffSide,
    state: DiffState,
    /// The file's flags and counts; its lines live in `state`.
    meta: Option<FileDiff>,
    loading: bool,
    error: Option<String>,
    /// Where each change starts, as row indices.
    starts: Vec<usize>,
    current: usize,
    scroll: UniformListScrollHandle,
    focus: FocusHandle,
    focused: bool,
}

impl FileDiffView {
    pub fn new(root: PathBuf, path: String, untracked: bool, staged: Staged, cx: &mut Context<Self>) -> Self {
        let mut view = Self {
            root,
            path,
            untracked,
            staged,
            side: DiffSide::All,
            state: DiffState::loading(),
            meta: None,
            loading: true,
            error: None,
            starts: Vec::new(),
            current: 0,
            scroll: UniformListScrollHandle::new(),
            focus: cx.focus_handle(),
            focused: false,
        };
        view.load(cx);
        view
    }

    /// Reads the side of the file's changes picked, and opens on its first change.
    fn load(&mut self, cx: &mut Context<Self>) {
        let (root, p, untracked, side) = (self.root.clone(), self.path.clone(), self.untracked, self.side);
        self.loading = true;
        self.error = None;
        cx.spawn(async move |this, cx| {
            let diff = cx.background_spawn(async move { git::whole_file_diff_side(&root, &p, untracked, side) }).await;
            this.update(cx, |this, cx| {
                // Another side was picked meanwhile.
                if this.side != side {
                    return;
                }
                this.loading = false;
                this.starts.clear();
                this.current = 0;
                match diff {
                    Ok(mut d) => {
                        // A hunk's header is a row before its lines.
                        let mut row = 0;
                        for h in &d.hunks {
                            row += 1;
                            this.starts.extend(git::change_starts(&h.lines).into_iter().map(|i| row + i));
                            row += h.lines.len();
                        }
                        this.state = DiffState::new(std::slice::from_ref(&d), false);
                        d.hunks.clear();
                        this.meta = Some(d);
                        this.goto(0);
                    }
                    Err(e) => this.error = Some(e),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn show_side(&mut self, side: DiffSide, cx: &mut Context<Self>) {
        if self.side != side {
            self.side = side;
            self.load(cx);
            cx.notify();
        }
    }

    /// All / Staged / Unstaged, for a file with changes on both sides.
    fn sides(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.staged != Staged::Partly {
            return None;
        }
        let t = theme(cx).clone();
        let button =
            |id: &'static str, label: &'static str, side: DiffSide, tip: &'static str, cx: &mut Context<Self>| {
                let on = self.side == side;
                div()
                    .id(id)
                    .relative()
                    .flex()
                    .items_center()
                    .h(px(22.))
                    .px_2()
                    .rounded_md()
                    .text_xs()
                    .cursor_pointer()
                    .text_color(if on { t.text } else { t.text_muted })
                    .child(row_wash(on, t.hover, t.active))
                    .child(label)
                    .tooltip(Tooltip::text(tip))
                    .on_click(cx.listener(move |this, _, _, cx| this.show_side(side, cx)))
            };
        Some(
            div()
                .flex()
                .flex_none()
                .gap_0p5()
                .child(button("file-diff-all", "All", DiffSide::All, "Every change since the last commit", cx))
                .child(button("file-diff-staged", "Staged", DiffSide::Staged, "What the next commit gets", cx))
                .child(button("file-diff-unstaged", "Unstaged", DiffSide::Unstaged, "What isn't staged yet", cx))
                .into_any_element(),
        )
    }
    fn goto(&mut self, n: usize) {
        if let Some(&row) = self.starts.get(n) {
            self.current = n;
            self.scroll.scroll_to_item(row.saturating_sub(CONTEXT_ABOVE), ScrollStrategy::Top);
        }
    }

    fn step(&mut self, forward: bool, cx: &mut Context<Self>) {
        let n = self.starts.len();
        if n == 0 {
            return;
        }
        self.goto(if forward { (self.current + 1) % n } else { (self.current + n - 1) % n });
        cx.notify();
    }

    fn key_down(&mut self, ev: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let k = &ev.keystroke;
        let m = &k.modifiers;
        let forward = match k.key.as_str() {
            "f7" if !m.control && !m.alt => !m.shift,
            "down" if m.alt => true,
            "up" if m.alt => false,
            "n" | "j" if !m.control && !m.alt && !m.platform => true,
            "p" | "k" if !m.control && !m.alt && !m.platform => false,
            _ => return,
        };
        self.step(forward, cx);
        cx.stop_propagation();
    }

    fn header(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx).clone();
        let (dir, name) = split_path(&self.path);
        let status = self.meta.as_ref().map(|d| {
            if d.new_file {
                ("Added", t.success)
            } else if d.deleted {
                ("Deleted", t.error)
            } else if d.old_path.is_some() {
                ("Renamed", t.info)
            } else {
                ("Modified", t.warning)
            }
        });
        let (added, removed) = self.meta.as_ref().map_or((0, 0), |d| (d.added, d.removed));
        let sides = self.sides(cx);
        let total = self.starts.len();
        let counter: SharedString =
            if total == 0 { "No changes".into() } else { format!("{} of {total}", self.current + 1).into() };
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .h(px(40.))
            .px_3()
            .border_b_1()
            .border_color(t.border_variant)
            .child(div().flex_none().text_sm().font_weight(FontWeight::MEDIUM).child(name))
            .child(div().min_w_0().truncate().text_xs().text_color(t.text_placeholder).child(dir))
            .when_some(status, |d, (label, color)| d.child(chip(label, color, &t)))
            .child(div().flex_1())
            .children(sides)
            .child(counts(added, removed, &t))
            .child(div().flex_none().text_xs().text_color(t.text_muted).child(counter))
            .child(
                icon_button("file-diff-prev", Icon::ArrowUp, cx)
                    .tooltip(Tooltip::with_meta("Previous change", "P · K · Shift+F7"))
                    .on_click(cx.listener(|this, _, _, cx| this.step(false, cx))),
            )
            .child(
                icon_button("file-diff-next", Icon::ArrowDown, cx)
                    .tooltip(Tooltip::with_meta("Next change", "N · J · F7"))
                    .on_click(cx.listener(|this, _, _, cx| this.step(true, cx))),
            )
            .child(
                icon_button("file-diff-close", Icon::Close, cx)
                    .tooltip(Tooltip::with_meta("Close", "Esc"))
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(FileDiffEvent::Close))),
            )
            .into_any_element()
    }
}

impl Focusable for FileDiffView {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for FileDiffView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.focused {
            self.focused = true;
            window.focus(&self.focus, cx);
        }
        let t = theme(cx).clone();
        let header = self.header(cx);
        let note = |text: SharedString, color| {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .p_4()
                .text_sm()
                .text_color(color)
                .child(text)
                .into_any_element()
        };
        let rows = self.state.rows();
        let binary = self.meta.as_ref().is_some_and(|d| d.binary);
        let body = if let Some(e) = &self.error {
            note(e.clone().into(), t.error)
        } else if self.loading {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .child(spinner("file-diff-loading", t.text_placeholder, px(16.)))
                .into_any_element()
        } else if binary {
            note("Binary file".into(), t.text_placeholder)
        } else if rows.is_empty() {
            note("Empty file".into(), t.text_placeholder)
        } else {
            let t2 = t.clone();
            let scroll = self.scroll.clone();
            let list = uniform_list("file-diff-rows", rows.len(), move |range, _, _| {
                // The gutter stays put by moving back as far as the list moved sideways.
                let scrolled = -f32::from(scroll.0.borrow().base_handle.offset().x);
                range.map(|i| render_row(&rows[i], scrolled, &t2)).collect()
            })
            .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
            .with_width_from_item(Some(self.state.widest()))
            .track_scroll(&self.scroll)
            .flex_1();
            div()
                .relative()
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .child(list)
                .child(scrollbar::vertical("file-diff-bar", self.scroll.clone()))
                .child(scrollbar::scrollbar("file-diff-hbar", self.scroll.clone(), gpui::Axis::Horizontal))
                .into_any_element()
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key_down))
            .child(header)
            .child(body)
    }
}
