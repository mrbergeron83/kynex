//! The file explorer for a session's or a project's files: Claude's scratch
//! folders, and for a session also the project files it edited, the images
//! in its conversation and its background tasks' output. Sections and the
//! folder tree on the left, the chosen one's entries on the right; a file
//! opens in a preview over it (`preview.rs`).
//!
//! Its own entity, like `GitView`; the app draws the modal around it, closes
//! it, and handles what it asks for ([`ScratchEvent`]: an image for the
//! viewer, a file's changes for the Git view). The scratch folder is watched
//! while it's open, so the lists keep up as Claude works.
//!
//! | file          | what                                                   |
//! |---------------|--------------------------------------------------------|
//! | `mod.rs`      | state, the folder tree, a folder's entries, layout     |
//! | `sections.rs` | a session's edited files, images and task output      |
//! | `preview.rs`  | the preview over the explorer: text, markdown, pictures |

mod preview;
mod sections;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::ui::px;
use gpui::{AnyElement, ClickEvent, Context, EventEmitter, SharedString, Window, div, prelude::*};
use kynex_core::files::{self, Entry, Preview};
use kynex_core::fmt::{ago, bytes};
use kynex_core::open;
use kynex_core::session_files::SessionFiles;
use kynex_core::watch::{self, Watch};

use crate::theme::{Palette, mono, theme};
use crate::ui::wash::row_wash;
use crate::ui::{Icon, Tooltip, ghost_button, icon, icon_button, scrollbar};

/// What the explorer asks the app to do.
pub enum ScratchEvent {
    /// Show image `index` of item `item` in the image viewer.
    OpenImage { item: usize, index: usize },
    /// Show a project file's changes in the Git view.
    ShowChanges(PathBuf),
}

impl EventEmitter<ScratchEvent> for ScratchView {}

/// A session's own files besides its scratch folder.
pub struct SessionExtras {
    pub cwd: PathBuf,
    pub files: SessionFiles,
}

/// What the right side shows.
#[derive(Clone, PartialEq, Eq)]
enum Pane {
    Folder(PathBuf),
    Edited,
    Images,
    Outputs,
}

/// A file open in the preview.
#[derive(Clone)]
struct Opened {
    path: PathBuf,
    name: String,
    size: Option<u64>,
    /// A project file the session edited: it can show its changes.
    edited: bool,
}

pub struct ScratchView {
    root: PathBuf,
    extras: Option<SessionExtras>,
    /// Edited files git still has changes for, once asked.
    changed: HashSet<PathBuf>,
    /// Folders read so far, by path.
    listed: HashMap<PathBuf, Vec<Entry>>,
    /// Folders open in the tree.
    expanded: HashSet<PathBuf>,
    pane: Pane,
    /// The file open in the preview, and its preview once read.
    preview: Option<(Opened, Option<Preview>)>,
    /// An error from the OS when opening something.
    error: Option<String>,
    /// The scratch folder being watched, while it exists.
    watch: Option<Watch>,
}

impl ScratchView {
    pub fn new(root: PathBuf, extras: Option<SessionExtras>, cx: &mut Context<Self>) -> Self {
        let mut view = Self {
            root: root.clone(),
            extras,
            changed: HashSet::new(),
            listed: HashMap::new(),
            expanded: HashSet::from([root.clone()]),
            pane: Pane::Folder(root.clone()),
            preview: None,
            error: None,
            watch: None,
        };
        view.load(root, cx);
        view.read_changes(cx);
        view.start_watching(cx);
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

    /// Watches the scratch folder, reading what's shown again when it changes.
    /// A folder that isn't there yet is watched from the next Refresh.
    fn start_watching(&mut self, cx: &mut Context<Self>) {
        if self.watch.is_some() {
            return;
        }
        let (tx, rx) = async_channel::bounded::<()>(1);
        let Ok(w) = watch::watch(&self.root, move || {
            let _ = tx.try_send(());
        }) else {
            return;
        };
        self.watch = Some(w);
        cx.spawn(async move |this, cx| {
            while rx.recv().await.is_ok() {
                if this.update(cx, |this, cx| this.refresh(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// Asks git which edited files still have changes, off the UI thread.
    fn read_changes(&mut self, cx: &mut Context<Self>) {
        let Some(extras) = &self.extras else { return };
        let (cwd, edited) = (extras.cwd.clone(), extras.files.edited.clone());
        cx.spawn(async move |this, cx| {
            let changed = cx
                .background_spawn(async move {
                    let Ok(status) = kynex_core::git::status(&cwd) else { return HashSet::new() };
                    edited
                        .into_iter()
                        .filter(|p| kynex_core::session_files::changed(&status, p))
                        .collect::<HashSet<_>>()
                })
                .await;
            this.update(cx, |this, cx| {
                this.changed = changed;
                cx.notify();
            })
            .ok();
        })
        .detach();
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

    /// Reads every folder shown again, and starts watching if the folder has appeared.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let shown: Vec<PathBuf> = self.listed.keys().cloned().collect();
        for dir in shown {
            self.load(dir, cx);
        }
        self.start_watching(cx);
        self.read_changes(cx);
    }

    /// Shows `dir`'s entries on the right and opens it in the tree.
    fn choose(&mut self, dir: PathBuf, cx: &mut Context<Self>) {
        let mut up = Some(dir.as_path());
        while let Some(d) = up.filter(|d| d.starts_with(&self.root)) {
            self.expanded.insert(d.to_path_buf());
            up = d.parent();
        }
        self.pane = Pane::Folder(dir.clone());
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

    /// Opens `path` in the preview and reads it off the UI thread.
    fn open_file(&mut self, path: PathBuf, edited: bool, cx: &mut Context<Self>) {
        let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
        let size = std::fs::metadata(&path).ok().map(|m| m.len());
        self.preview = Some((Opened { path: path.clone(), name, size, edited }, None));
        cx.spawn(async move |this, cx| {
            let p = path.clone();
            let preview = cx.background_spawn(async move { files::preview(&p) }).await;
            this.update(cx, |this, cx| {
                if let Some((o, slot)) = &mut this.preview
                    && o.path == path
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
            let chosen = self.pane == Pane::Folder(dir.clone());
            let (pick, fold) = (dir.clone(), dir.clone());
            rows.push(
                nav_row(SharedString::from(format!("tree-{}", dir.display())), chosen, depth, t)
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

    /// A folder's entries: folders (open on click) then files (preview on click).
    fn folder_entries(&self, dir: &Path, t: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let Some(entries) = self.listed.get(dir) else {
            return note("Reading…", t);
        };
        if entries.is_empty() {
            return note(
                if dir == self.root && !self.root.exists() {
                    "No scratch files yet. Claude keeps its temporary files here while it works: scripts, notes, downloads, the output of background tasks."
                } else {
                    "This folder is empty."
                },
                t,
            );
        }
        let rows = entries.iter().enumerate().map(|(i, e)| {
            let entry = e.clone();
            entry_row(("entry", i), if e.is_dir { Icon::Folder } else { Icon::File }, e.name.clone(), t)
                .child(meta(if e.is_dir { String::new() } else { bytes(e.size) }, 72., t))
                .child(meta(e.modified.map(ago).unwrap_or_default(), 84., t))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    if entry.is_dir {
                        this.choose(entry.path.clone(), cx);
                    } else {
                        this.open_file(entry.path.clone(), false, cx);
                    }
                }))
                .into_any_element()
        });
        scrolling("scratch-entries", rows.collect())
    }

    /// What the right side's header says, and the folder its Go to folder opens.
    fn pane_title(&self) -> (String, Option<PathBuf>) {
        match &self.pane {
            Pane::Folder(dir) => {
                let rel = dir.strip_prefix(&self.root).map(|p| p.display().to_string()).unwrap_or_default();
                (if rel.is_empty() { "Scratch".into() } else { format!("Scratch / {rel}") }, Some(dir.clone()))
            }
            Pane::Edited => ("Edited in the project".into(), self.extras.as_ref().map(|e| e.cwd.clone())),
            Pane::Images => ("Images in the conversation".into(), None),
            Pane::Outputs => ("Background task output".into(), None),
        }
    }
}

impl Render for ScratchView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx).clone();
        let mut left = self.section_rows(&t, cx);
        left.extend(self.tree(&t, cx));
        let tree_handle = scrollbar::handle("scratch-tree");
        let tree =
            div().id("scratch-tree").size_full().overflow_y_scroll().track_scroll(&tree_handle).p_1().children(left);
        let (title, folder) = self.pane_title();
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
                div().flex_1().min_w_0().truncate().font_family(mono()).text_xs().text_color(t.text_muted).child(title),
            )
            .when_some(folder, |d, folder| {
                d.child(
                    ghost_button("folder-reveal", cx)
                        .text_xs()
                        .child("Go to folder")
                        .tooltip(Tooltip::text("Open it in your file manager"))
                        .on_click(cx.listener(move |this, _, _, cx| this.os(open::reveal(&folder), cx))),
                )
            })
            .child(
                icon_button("scratch-refresh", Icon::Refresh, cx)
                    .tooltip(Tooltip::text("Read again (it also follows changes by itself)"))
                    .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
            );
        let body = match self.pane.clone() {
            Pane::Folder(dir) => self.folder_entries(&dir, &t, cx),
            Pane::Edited => self.edited_entries(&t, cx),
            Pane::Images => self.image_entries(&t, cx),
            Pane::Outputs => self.output_entries(&t, cx),
        };
        div()
            .relative()
            .flex()
            .size_full()
            .child(
                div()
                    .flex_none()
                    .w(px(250.))
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
                    .child(div().flex_1().min_h_0().child(body)),
            )
            .children(self.preview(&t, cx))
    }
}

/// A row on the left: a section or a folder, indented by `depth`.
fn nav_row(id: SharedString, chosen: bool, depth: usize, t: &Palette) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
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
}

/// A row on the right: an icon and a name, then whatever the caller adds.
fn entry_row(id: impl Into<gpui::ElementId>, glyph: Icon, name: String, t: &Palette) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
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
        .child(icon(glyph, t.text_muted).size_3p5().flex_none())
        .child(div().flex_1().min_w_0().truncate().child(name))
}

/// A muted column at the end of a row.
fn meta(text: String, width: f32, t: &Palette) -> gpui::Div {
    div().flex_none().w(px(width)).text_right().text_xs().text_color(t.text_placeholder).child(text)
}

fn note(text: &str, t: &Palette) -> AnyElement {
    div().p_3().text_sm().text_color(t.text_placeholder).child(text.to_string()).into_any_element()
}

fn scrolling(id: &'static str, rows: Vec<AnyElement>) -> AnyElement {
    let handle = scrollbar::handle(id);
    let area = div().id(id).size_full().overflow_y_scroll().track_scroll(&handle).p_1().children(rows);
    scrollbar::vertical_over(SharedString::from(format!("{id}-bar")), handle, area, |d| d.size_full())
        .into_any_element()
}
