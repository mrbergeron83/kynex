//! One changed file as a whole, read-only, in a modal over the window. Text shows every
//! line with the changes tinted, buttons (and keys) to jump between changes, and a caret
//! and selection to copy from (`file_text.rs`). Images show before / after, PDFs their
//! pages, other binaries a placeholder (`file_media.rs`). A file partly staged can show
//! all its changes, the staged ones or the unstaged ones.

use std::path::PathBuf;
use std::sync::Arc;

use crate::ui::px;
use gpui::{
    AnyElement, Context, EventEmitter, FocusHandle, Focusable, FontWeight, KeyDownEvent, RenderImage, ScrollStrategy,
    SharedString, UniformListScrollHandle, Window, div, prelude::*,
};
use kynex_core::file_view::caret::Caret;
use kynex_core::file_view::{self, Loaded, Side};
use kynex_core::git::{self, DiffSide, FileDiff, Staged};

use super::file_media::{PdfPages, Prepared, Shown, prepare};
use super::file_text::TextMenu;
use crate::git::diff_view::{DiffState, chip, counts, split_path};
use crate::theme::theme;
use crate::ui::wash::row_wash;
use crate::ui::{Icon, Tooltip, icon_button, spinner};

/// What the view asks of the app.
pub enum FileDiffEvent {
    Close,
    /// Open this picture in the image viewer; its own size in pixels.
    ViewImage(Arc<RenderImage>, (u32, u32)),
}

impl EventEmitter<FileDiffEvent> for FileDiffView {}

/// Rows kept above a change when jumping to it.
const CONTEXT_ABOVE: usize = 3;

/// What the body shows.
pub(super) enum Body {
    Loading,
    Failed(String),
    /// The diff's rows, in `state`.
    Text,
    Pictures {
        before: Shown,
        after: Shown,
    },
    Pdf(PdfPages),
    Binary {
        size: u64,
    },
    TooLarge {
        size: u64,
    },
}

pub struct FileDiffView {
    pub(super) root: PathBuf,
    pub(super) path: String,
    untracked: bool,
    /// How much of the file is staged: partly, and the sides can be shown apart.
    staged: Staged,
    side: DiffSide,
    pub(super) state: DiffState,
    /// The file's flags and counts, for text; its lines live in `state`.
    meta: Option<FileDiff>,
    pub(super) body: Body,
    /// Where each change starts, as row indices.
    starts: Vec<usize>,
    current: usize,
    pub(super) scroll: UniformListScrollHandle,
    pub(super) caret: Caret,
    /// A drag-selection is under way.
    pub(super) dragging: bool,
    pub(super) menu: Option<TextMenu>,
    /// The PDF's zoom, 1 being the pane's width.
    pub(super) pdf_zoom: f32,
    pub(super) focus: FocusHandle,
    focused: bool,
}

impl FileDiffView {
    pub fn new(root: PathBuf, path: String, untracked: bool, staged: Staged, cx: &mut Context<Self>) -> Self {
        // Pictures go back to the GPU's atlas when the modal closes.
        cx.on_release(|this, cx| this.release_images(cx)).detach();
        let mut view = Self {
            root,
            path,
            untracked,
            staged,
            side: DiffSide::All,
            state: DiffState::default(),
            meta: None,
            body: Body::Loading,
            starts: Vec::new(),
            current: 0,
            scroll: UniformListScrollHandle::new(),
            caret: Caret::default(),
            dragging: false,
            menu: None,
            pdf_zoom: 1.,
            focus: cx.focus_handle(),
            focused: false,
        };
        view.load(cx);
        view
    }

    fn set_body(&mut self, body: Body, cx: &mut Context<Self>) {
        self.release_images(cx);
        self.body = body;
    }

    fn release_images(&mut self, cx: &mut gpui::App) {
        let images: Vec<Arc<RenderImage>> = match &mut self.body {
            Body::Pictures { before, after } => [before, after].into_iter().filter_map(Shown::image).collect(),
            Body::Pdf(pages) => pages.take_images(),
            _ => Vec::new(),
        };
        for image in images {
            cx.drop_image(image, None);
        }
    }

    /// Reads the side of the file picked, and opens on its first change.
    fn load(&mut self, cx: &mut Context<Self>) {
        let (root, p, untracked, side) = (self.root.clone(), self.path.clone(), self.untracked, self.side);
        self.set_body(Body::Loading, cx);
        self.menu = None;
        cx.spawn(async move |this, cx| {
            let loaded =
                cx.background_spawn(async move { file_view::load(&root, &p, untracked, side).map(prepare) }).await;
            this.update(cx, |this, cx| {
                // Another side was picked meanwhile.
                if this.side != side {
                    return;
                }
                this.starts.clear();
                this.current = 0;
                this.meta = None;
                this.state = DiffState::default();
                let body = match loaded {
                    Err(e) => Body::Failed(e),
                    Ok(Prepared::Pictures(before, after)) => Body::Pictures { before, after },
                    Ok(Prepared::Other(Loaded::Text(d))) => {
                        this.show_text(d);
                        Body::Text
                    }
                    Ok(Prepared::Other(Loaded::Pdf(Side::Ready(bytes)))) => {
                        let pages = PdfPages::start(bytes, cx);
                        this.set_body(Body::Pdf(pages), cx);
                        cx.notify();
                        return;
                    }
                    Ok(Prepared::Other(Loaded::Pdf(Side::TooLarge(size)))) => Body::TooLarge { size },
                    Ok(Prepared::Other(Loaded::Pdf(Side::Failed(e)))) => Body::Failed(e),
                    Ok(Prepared::Other(Loaded::Pdf(Side::Missing))) => Body::Failed("The file isn't there".into()),
                    // Pictures come prepared; the arm is only for completeness.
                    Ok(Prepared::Other(Loaded::Pictures { before, after })) => {
                        Body::Pictures { before: before.into(), after: after.into() }
                    }
                    Ok(Prepared::Other(Loaded::Binary { size })) => Body::Binary { size },
                    Ok(Prepared::Other(Loaded::TooLarge { size })) => Body::TooLarge { size },
                };
                this.set_body(body, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn show_text(&mut self, mut d: FileDiff) {
        // A hunk's header is a row before its lines.
        let mut row = 0;
        for h in &d.hunks {
            row += 1;
            self.starts.extend(git::change_starts(&h.lines).into_iter().map(|i| row + i));
            row += h.lines.len();
        }
        self.state = DiffState::new(std::slice::from_ref(&d), false);
        d.hunks.clear();
        self.meta = Some(d);
        self.caret = Caret::start(&super::file_text::RowLines(&self.state.rows()));
        self.goto(0);
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
            self.caret = Caret::at(kynex_core::file_view::caret::Pos::new(row, 0));
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

    fn key_down(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &ev.keystroke;
        let m = &k.modifiers;
        if k.key == "escape" && self.menu.take().is_some() {
            cx.notify();
            cx.stop_propagation();
            return;
        }
        let forward = match k.key.as_str() {
            "f7" if !m.control && !m.alt => Some(!m.shift),
            "down" if m.alt && !cfg!(target_os = "macos") => Some(true),
            "up" if m.alt && !cfg!(target_os = "macos") => Some(false),
            "n" | "j" if !m.control && !m.alt && !m.platform => Some(true),
            "p" | "k" if !m.control && !m.alt && !m.platform => Some(false),
            _ => None,
        };
        if let Some(forward) = forward {
            if matches!(self.body, Body::Text) {
                self.step(forward, cx);
            }
            cx.stop_propagation();
            return;
        }
        let handled = match self.body {
            Body::Text => self.text_key(ev, window, cx),
            Body::Pdf(_) => self.pdf_key(ev, cx),
            _ => false,
        };
        if handled {
            cx.stop_propagation();
        }
    }

    fn header(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx).clone();
        let (dir, name) = split_path(&self.path);
        let status = match (&self.meta, &self.body) {
            (Some(d), _) if d.new_file => Some(("Added", t.success)),
            (Some(d), _) if d.deleted => Some(("Deleted", t.error)),
            (Some(d), _) if d.old_path.is_some() => Some(("Renamed", t.info)),
            (Some(_), _) => Some(("Modified", t.warning)),
            (None, Body::Pictures { before: Shown::Missing, .. }) => Some(("Added", t.success)),
            (None, Body::Pictures { after: Shown::Missing, .. }) => Some(("Deleted", t.error)),
            (None, Body::Pictures { .. }) => Some(("Modified", t.warning)),
            _ => None,
        };
        let text = matches!(self.body, Body::Text);
        let (added, removed) = self.meta.as_ref().map_or((0, 0), |d| (d.added, d.removed));
        let sides = self.sides(cx);
        let total = self.starts.len();
        let counter: SharedString =
            if total == 0 { "No changes".into() } else { format!("{} of {total}", self.current + 1).into() };
        let pdf = self.pdf_controls(cx);
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
            .children(pdf)
            .when(text, |d| {
                d.child(counts(added, removed, &t))
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
            })
            .child(
                icon_button("file-diff-close", Icon::Close, cx)
                    .tooltip(Tooltip::with_meta("Close", "Esc"))
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(FileDiffEvent::Close))),
            )
            .into_any_element()
    }
}

/// A message in the middle of the body.
pub(super) fn note(text: impl Into<SharedString>, color: gpui::Hsla) -> gpui::Div {
    div().flex_1().flex().items_center().justify_center().p_4().text_sm().text_color(color).child(text.into())
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
        let body = match &self.body {
            Body::Failed(e) => note(e.clone(), t.error).into_any_element(),
            Body::Loading => div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .child(spinner("file-diff-loading", t.text_placeholder, px(16.)))
                .into_any_element(),
            Body::Text if self.state.rows().is_empty() => note("Empty file", t.text_placeholder).into_any_element(),
            Body::Text => self.render_text(window, cx),
            Body::Pictures { .. } => self.render_pictures(cx),
            Body::Pdf(_) => self.render_pdf(cx),
            Body::Binary { size } => self.render_binary("Binary file", *size, cx),
            Body::TooLarge { size } => self.render_binary("Too large to show", *size, cx),
        };
        let menu = self.render_text_menu(cx);
        div()
            .size_full()
            .flex()
            .flex_col()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key_down))
            .child(header)
            .child(body)
            .children(menu)
    }
}
