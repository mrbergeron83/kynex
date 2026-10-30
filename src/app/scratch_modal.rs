//! The file explorer (`crate::scratch`) in a modal over the conversation: a
//! session's files (its scratch folder, edited files, images, task output)
//! from the session header or the composer's files button, whose badge counts
//! scratch files changed since you last looked; or the project's scratch
//! folders (every session's) from the toolbar. Esc closes the preview first,
//! then the modal; so does a click outside.

use std::path::PathBuf;
use std::time::SystemTime;

use crate::ui::px;
use gpui::{AnyElement, Context, Entity, FontWeight, MouseButton, SharedString, Window, div, prelude::*};
use kynex_core::live::Key;
use kynex_core::session::{Item, Kind};
use kynex_core::session_files;
use kynex_core::session_log::LoggedImage;
use kynex_core::storage::{self, Roots};

use super::KynexApp;
use crate::scratch::{ScratchEvent, ScratchView, SessionExtras};
use crate::theme::{mono, theme};
use crate::ui::motion::{self, fade, pop};
use crate::ui::{Icon, Tooltip, icon, icon_button};

#[derive(Clone)]
pub(super) struct ScratchModal {
    view: Entity<ScratchView>,
    title: SharedString,
}

impl KynexApp {
    /// Opens the explorer on `root`; with `session`, also that session's own
    /// files (`extras`), whose images and changes it asks the app to show.
    fn open_scratch(
        &mut self,
        root: PathBuf,
        title: String,
        session: Option<(Key, SessionExtras)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.menu = None;
        let (key, extras) = session.map_or((None, None), |(k, e)| (Some(k), Some(e)));
        let cwd = extras.as_ref().map(|e| e.cwd.clone());
        let view = cx.new(|cx| ScratchView::new(root, extras, cx));
        cx.subscribe_in(&view, window, move |this, _, ev: &ScratchEvent, window, cx| match ev {
            ScratchEvent::OpenImage { item, index } => {
                let preview =
                    this.all_sessions().find(|s| Some(s.uid) == key).and_then(|s| match s.items.get(*item)? {
                        Item::User { images, .. } | Item::ToolResult { images, .. } => images.get(*index).cloned(),
                        _ => None,
                    });
                let Some(preview) = preview else { return };
                if let Ok(image) = preview.clone().downcast::<gpui::Image>() {
                    this.open_image(image, window, cx);
                } else if let Some(logged) = preview.downcast_ref::<LoggedImage>() {
                    this.open_logged(logged.clone(), window, cx);
                }
            }
            ScratchEvent::ShowChanges(file) => {
                let Some(cwd) = cwd.clone() else { return };
                this.scratch = None;
                this.open_git_file(cwd, file.clone(), cx);
            }
        })
        .detach();
        self.scratch = Some(ScratchModal { view, title: title.into() });
        cx.notify();
    }

    /// The shown session's files: its scratch folder, what it edited, its
    /// images and task output. Nothing for a session not saved yet.
    pub(super) fn open_session_scratch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(s) = self.active_session().filter(|s| s.kind == Kind::Code) else { return };
        let (cwd, id, key) = (s.cwd.clone(), s.session_id.clone(), s.uid);
        let Some(root) = Roots::local().zip(id.clone()).and_then(|(r, id)| storage::session_scratch(&r, &cwd, &id))
        else {
            return;
        };
        let files = session_files::gather(&s.items, &cwd);
        if let Some(id) = &id {
            self.scratch_seen.insert(id.clone(), SystemTime::now());
            self.scratch_new.remove(id);
        }
        let title = id.as_deref().and_then(|id| self.session_name(id));
        let title = title.map_or_else(|| "This session's files".into(), |t| format!("Files · {t}"));
        self.open_scratch(root, title, Some((key, SessionExtras { cwd, files })), window, cx);
    }

    /// The scratch folders of every session in the active project.
    pub(super) fn open_project_scratch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.active_project().map(|p| p.path.clone()) else { return };
        let Some(roots) = Roots::local() else { return };
        let name = kynex_core::paths::project_name(&path);
        let root = storage::project_scratch(&roots, &path);
        self.open_scratch(root, format!("Scratch · {name}, every session"), None, window, cx);
    }

    /// How many of the shown session's scratch files changed since its files
    /// were last opened, for the composer's badge; it stays, through later
    /// turns too, until they're opened. Counted off the UI thread when its
    /// conversation grows, so not every frame.
    pub(super) fn scratch_badge(&mut self, cx: &mut Context<Self>) -> Option<usize> {
        let s = self.active_session().filter(|s| s.kind == Kind::Code)?;
        let (id, cwd) = (s.session_id.clone()?, s.cwd.clone());
        let stamp = (id.clone(), s.items.len());
        if self.scratch_counted.as_ref() != Some(&stamp) {
            self.scratch_counted = Some(stamp);
            let since = self.scratch_seen.get(&id).copied().unwrap_or(SystemTime::UNIX_EPOCH);
            let root = Roots::local().and_then(|r| storage::session_scratch(&r, &cwd, &id));
            cx.spawn(async move |this, cx| {
                let n = match root {
                    Some(root) => cx.background_spawn(async move { session_files::count_newer(&root, since) }).await,
                    None => 0,
                };
                this.update(cx, |this, cx| {
                    this.scratch_new.insert(id, n);
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
        let id = self.active_session()?.session_id.clone()?;
        Some(self.scratch_new.get(&id).copied().unwrap_or(0))
    }

    /// Esc: the preview closes first, then the modal; false if neither was open.
    pub(super) fn close_scratch(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(m) = &self.scratch else { return false };
        if !m.view.update(cx, |v, cx| v.close_preview(cx)) {
            self.scratch = None;
        }
        cx.notify();
        true
    }

    pub(super) fn render_scratch_modal(
        &mut self,
        leaving: Option<f32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let m = self.scratch.clone()?;
        let t = theme(cx).clone();
        let vp = window.viewport_size();
        let root = m.view.read(cx).root().display().to_string();
        let header = div()
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .h(px(40.))
            .px_3()
            .border_b_1()
            .border_color(t.border_variant)
            .child(icon(Icon::Files, t.text_muted))
            .child(div().flex_none().text_sm().font_weight(FontWeight::MEDIUM).child(m.title.clone()))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_xs()
                    .font_family(mono())
                    .text_color(t.text_placeholder)
                    .child(root),
            )
            .child(icon_button("scratch-close", Icon::Close, cx).tooltip(Tooltip::with_meta("Close", "Esc")).on_click(
                cx.listener(|this, _, _, cx| {
                    this.scratch = None;
                    cx.notify();
                }),
            ));
        let card = div()
            .id("scratch-modal")
            .occlude()
            .flex()
            .flex_col()
            .w(gpui::px(1100.).min(vp.width - gpui::px(80.)))
            .h(vp.height - gpui::px(120.))
            .rounded_lg()
            .overflow_hidden()
            .border_1()
            .border_color(t.border)
            .bg(t.editor)
            .shadow_lg()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(header)
            .child(div().flex_1().min_h_0().child(m.view.clone()));
        Some(fade(
            "scratch-modal-fade",
            div()
                .id("scratch-modal-backdrop")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(crate::theme::theme(cx).scrim())
                .occlude()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.scratch = None;
                        cx.notify();
                    }),
                )
                .child(pop("scratch-modal-in", card, gpui::px(8.), leaving)),
            motion::QUICK,
            leaving,
        ))
    }
}
