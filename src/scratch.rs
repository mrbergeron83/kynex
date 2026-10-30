//! The scratch explorer: Claude's scratch files for a session, or for every
//! session of a project. Folders on the left, the chosen folder's entries on
//! the right; a file opens in a preview over it (highlighted text, a picture),
//! with Go to file (the OS file manager, the file selected) and Open.
//!
//! Its own entity, like `GitView`; the app draws the modal around it and
//! closes it. Folders are read off the UI thread when they're first shown and
//! again on Refresh.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::ui::px;
use gpui::{
    AnyElement, ClickEvent, Context, MouseButton, ObjectFit, SharedString, StyledImage, StyledText, Window, div, img,
    prelude::*,
};
use kynex_core::files::{self, Entry, Preview};
use kynex_core::fmt::{ago, bytes};
use kynex_core::highlight::Lang;
use kynex_core::open;

use crate::theme::{Palette, mono, theme};
use crate::ui::motion::{self, fade, pop};
use crate::ui::wash::row_wash;
use crate::ui::{Icon, Tooltip, code, ghost_button, icon, icon_button, scrollbar};

pub struct ScratchView {
    root: PathBuf,
    /// Folders read so far, by path.
    listed: HashMap<PathBuf, Vec<Entry>>,
    /// Folders open in the tree.
    expanded: HashSet<PathBuf>,
    /// The folder whose entries show on the right.
    folder: PathBuf,
    /// The file open in the preview, and its preview once read.
    preview: Option<(Entry, Option<Preview>)>,
    /// An error from the OS when opening something.
    error: Option<String>,
}

impl ScratchView {
    pub fn new(root: PathBuf, cx: &mut Context<Self>) -> Self {
        let mut view = Self {
            root: root.clone(),
            listed: HashMap::new(),
            expanded: HashSet::from([root.clone()]),
            folder: root.clone(),
            preview: None,
            error: None,
        };
        view.load(root, cx);
        view
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Closes the preview; false if none was open.
    pub fn close_preview(&mut self, cx: &mut Context<Self>) -> bool {
        let open = self.preview.take().is_some();
        cx.notify();
        open
    }

    /// Reads `dir`'s entries off the UI thread.
    fn load(&mut self, dir: PathBuf, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let d = dir.clone();
            let entries = cx.background_spawn(async move { files::list(&d) }).await;
            this.update(cx, |this, cx| {
                this.listed.insert(dir, entries);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Reads every folder shown again.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let shown: Vec<PathBuf> = self.listed.keys().cloned().collect();
        for dir in shown {
            self.load(dir, cx);
        }
    }

    /// Shows `dir`'s entries on the right and opens it in the tree.
    fn choose(&mut self, dir: PathBuf, cx: &mut Context<Self>) {
        let mut up = Some(dir.as_path());
        while let Some(d) = up.filter(|d| d.starts_with(&self.root)) {
            self.expanded.insert(d.to_path_buf());
            up = d.parent();
        }
        self.folder = dir.clone();
        self.load(dir, cx);
        cx.notify();
    }

    fn toggle(&mut self, dir: PathBuf, cx: &mut Context<Self>) {
        if !self.expanded.remove(&dir) {
            self.expanded.insert(dir.clone());
            if !self.listed.contains_key(&dir) {
                self.load(dir, cx);
            }
        }
        cx.notify();
    }

    fn open_file(&mut self, entry: Entry, cx: &mut Context<Self>) {
        let path = entry.path.clone();
        self.preview = Some((entry, None));
        cx.spawn(async move |this, cx| {
            let p = path.clone();
            let preview = cx.background_spawn(async move { files::preview(&p) }).await;
            this.update(cx, |this, cx| {
                if let Some((e, slot)) = &mut this.preview
                    && e.path == path
                {
                    *slot = Some(preview);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn os(&mut self, result: Result<(), String>, cx: &mut Context<Self>) {
        self.error = result.err();
        cx.notify();
    }

    /// The folder tree from the root, open folders unfolded.
    fn tree(&self, t: &Palette, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut rows = Vec::new();
        let mut stack = vec![(self.root.clone(), 0usize)];
        while let Some((dir, depth)) = stack.pop() {
            let open = self.expanded.contains(&dir);
            let name = if dir == self.root {
                "Scratch".to_string()
            } else {
                dir.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned())
            };
            let has_folders = self.listed.get(&dir).is_none_or(|e| e.iter().any(|e| e.is_dir));
            let chosen = dir == self.folder;
            let (pick, fold) = (dir.clone(), dir.clone());
            rows.push(
                div()
                    .id(SharedString::from(format!("tree-{}", dir.display())))
                    .relative()
                    .flex()
                    .items_center()
                    .gap_1()
                    .h(px(26.))
                    .pl(px(6. + depth as f32 * 14.))
                    .pr_2()
                    .rounded_md()
                    .cursor_pointer()
                    .text_sm()
                    .child(row_wash(chosen, t.hover, t.active))
                    .child(
                        div().id(SharedString::from(format!("fold-{}", dir.display()))).flex_none().size(px(14.)).when(
                            has_folders,
                            |d| {
                                d.child(
                                    icon(if open { Icon::ChevronDown } else { Icon::ChevronRight }, t.text_muted)
                                        .size_3(),
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.toggle(fold.clone(), cx);
                                }))
                            },
                        ),
                    )
                    .child(icon(Icon::Folder, if chosen { t.accent } else { t.text_muted }).size_3p5().flex_none())
                    .child(div().flex_1().min_w_0().truncate().child(name))
                    .on_click(cx.listener(move |this, _, _, cx| this.choose(pick.clone(), cx)))
                    .into_any_element(),
            );
            if open && let Some(entries) = self.listed.get(&dir) {
                for e in entries.iter().filter(|e| e.is_dir).rev() {
                    stack.push((e.path.clone(), depth + 1));
                }
            }
        }
        rows
    }

    /// The chosen folder's entries: folders (open on click) then files (preview on click).
    fn entries(&self, t: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let Some(entries) = self.listed.get(&self.folder) else {
            return div().p_3().text_sm().text_color(t.text_placeholder).child("Reading…").into_any_element();
        };
        if entries.is_empty() {
            let what = if self.folder == self.root && !self.root.exists() {
                "No scratch files yet. Claude keeps its temporary files here while it works: scripts, notes, downloads, the output of background tasks."
            } else {
                "This folder is empty."
            };
            return div().p_3().text_sm().text_color(t.text_placeholder).child(what).into_any_element();
        }
        let rows = entries.iter().enumerate().map(|(i, e)| {
            let entry = e.clone();
            div()
                .id(("entry", i))
                .relative()
                .flex()
                .items_center()
                .gap_2()
                .h(px(28.))
                .px_2()
                .rounded_md()
                .cursor_pointer()
                .text_sm()
                .child(row_wash(false, t.hover, t.active))
                .child(icon(if e.is_dir { Icon::Folder } else { Icon::File }, t.text_muted).size_3p5().flex_none())
                .child(div().flex_1().min_w_0().truncate().child(e.name.clone()))
                .child(
                    div()
                        .flex_none()
                        .w(px(72.))
                        .text_right()
                        .font_family(mono())
                        .text_xs()
                        .text_color(t.text_muted)
                        .child(if e.is_dir { String::new() } else { bytes(e.size) }),
                )
                .child(
                    div()
                        .flex_none()
                        .w(px(84.))
                        .text_right()
                        .text_xs()
                        .text_color(t.text_placeholder)
                        .child(e.modified.map(ago).unwrap_or_default()),
                )
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    if entry.is_dir {
                        this.choose(entry.path.clone(), cx);
                    } else {
                        this.open_file(entry.clone(), cx);
                    }
                }))
        });
        let handle = scrollbar::handle("scratch-entries");
        let area =
            div().id("scratch-entries").size_full().overflow_y_scroll().track_scroll(&handle).p_1().children(rows);
        scrollbar::vertical_over("scratch-entries-bar", handle, area, |d| d.size_full()).into_any_element()
    }

    /// The preview over the explorer: what the file holds, Go to file, Open.
    fn preview(&self, t: &Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (entry, preview) = self.preview.clone()?;
        let path = entry.path.clone();
        let (reveal, open_it, copy) = (path.clone(), path.clone(), path.display().to_string());
        let header = div()
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .h(px(40.))
            .px_3()
            .border_b_1()
            .border_color(t.border_variant)
            .child(icon(Icon::File, t.text_muted).flex_none())
            .child(div().flex_none().text_sm().child(entry.name.clone()))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_family(mono())
                    .text_xs()
                    .text_color(t.text_placeholder)
                    .child(format!("{} · {}", bytes(entry.size), path.display())),
            )
            .child(
                ghost_button("preview-reveal", cx)
                    .text_xs()
                    .child("Go to file")
                    .tooltip(Tooltip::text("Show it in your file manager"))
                    .on_click(cx.listener(move |this, _, _, cx| this.os(open::reveal(&reveal), cx))),
            )
            .child(
                ghost_button("preview-open", cx)
                    .text_xs()
                    .child("Open")
                    .tooltip(Tooltip::text("In the app your system opens it with"))
                    .on_click(cx.listener(move |this, _, _, cx| this.os(open::open_default(&open_it), cx))),
            )
            .child(
                icon_button("preview-copy", Icon::Copy, cx)
                    .tooltip(Tooltip::text("Copy path"))
                    .on_click(move |_, _, cx| cx.write_to_clipboard(gpui::ClipboardItem::new_string(copy.clone()))),
            )
            .child(icon_button("preview-close", Icon::Close, cx).tooltip(Tooltip::with_meta("Close", "Esc")).on_click(
                cx.listener(|this, _, _, cx| {
                    this.close_preview(cx);
                }),
            ));
        let note = |s: &str| div().p_4().text_sm().text_color(t.text_muted).child(s.to_string()).into_any_element();
        let body = match preview {
            None => note("Reading…"),
            Some(Preview::Image) => div()
                .size_full()
                .p_3()
                .flex()
                .items_center()
                .justify_center()
                .child(img(path.clone()).max_w_full().max_h_full().object_fit(ObjectFit::Contain))
                .into_any_element(),
            Some(Preview::Binary) => note("This file isn't text or a picture. Open it in another app, or go to it."),
            Some(Preview::Missing(e)) => note(&e),
            Some(Preview::Text { text, cut }) => {
                let lang = Lang::from_path(&entry.name);
                let styles = code::highlighted(lang, &text, &t.syntax);
                let handle = scrollbar::handle("scratch-preview");
                let area = div()
                    .id("scratch-preview")
                    .size_full()
                    .overflow_scroll()
                    .track_scroll(&handle)
                    .p_3()
                    .font_family(mono())
                    .text_xs()
                    .child(StyledText::new(text).with_highlights(styles))
                    .when(cut, |d| {
                        d.child(
                            div()
                                .pt_2()
                                .text_color(t.text_placeholder)
                                .child("Only the start is shown. Open the file to see all of it."),
                        )
                    });
                scrollbar::vertical_over("scratch-preview-bar", handle, area, |d| d.size_full()).into_any_element()
            }
        };
        let card = div()
            .id("scratch-preview-card")
            .occlude()
            .flex()
            .flex_col()
            .size_full()
            .rounded_lg()
            .overflow_hidden()
            .border_1()
            .border_color(t.border)
            .bg(t.editor)
            .shadow_lg()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(header)
            .child(div().flex_1().min_h_0().child(body));
        Some(
            fade(
                "scratch-preview-fade",
                div()
                    .id("scratch-preview-backdrop")
                    .absolute()
                    .inset_0()
                    .p_6()
                    .bg(gpui::black().opacity(0.35))
                    .occlude()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.close_preview(cx);
                        }),
                    )
                    .child(pop("scratch-preview-in", card, gpui::px(6.), None)),
                motion::QUICK,
                None,
            )
            .into_any_element(),
        )
    }
}

impl Render for ScratchView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx).clone();
        let tree_handle = scrollbar::handle("scratch-tree");
        let tree = div()
            .id("scratch-tree")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&tree_handle)
            .p_1()
            .children(self.tree(&t, cx));
        let crumbs = self.folder.strip_prefix(&self.root).map(|p| p.display().to_string()).unwrap_or_default();
        let folder = self.folder.clone();
        let right_head = div()
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .h(px(32.))
            .px_3()
            .border_b_1()
            .border_color(t.border_variant)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_family(mono())
                    .text_xs()
                    .text_color(t.text_muted)
                    .child(if crumbs.is_empty() { "Scratch".to_string() } else { format!("Scratch / {crumbs}") }),
            )
            .child(
                ghost_button("folder-reveal", cx)
                    .text_xs()
                    .child("Go to folder")
                    .tooltip(Tooltip::text("Open it in your file manager"))
                    .on_click(cx.listener(move |this, _, _, cx| this.os(open::reveal(&folder), cx))),
            )
            .child(
                icon_button("scratch-refresh", Icon::Refresh, cx)
                    .tooltip(Tooltip::text("Read the folders again"))
                    .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
            );
        div()
            .relative()
            .flex()
            .size_full()
            .child(
                div()
                    .flex_none()
                    .w(px(240.))
                    .h_full()
                    .border_r_1()
                    .border_color(t.border_variant)
                    .bg(t.panel)
                    .child(scrollbar::vertical_over("scratch-tree-bar", tree_handle, tree, |d| d.size_full())),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(right_head)
                    .when_some(self.error.clone(), |d, e| {
                        d.child(div().px_3().py_1().text_xs().text_color(t.error).child(e))
                    })
                    .child(div().flex_1().min_h_0().child(self.entries(&t, cx))),
            )
            .children(self.preview(&t, cx))
    }
}
