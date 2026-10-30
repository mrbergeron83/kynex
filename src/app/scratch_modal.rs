//! The scratch explorer (`crate::scratch`) in a modal over the conversation:
//! a session's scratch folder from the session header, or the project's (every
//! session's) from the toolbar and Ctrl+K. Esc closes the preview first, then
//! the modal; so does a click outside.

use std::path::PathBuf;

use crate::ui::px;
use gpui::{AnyElement, Context, Entity, FontWeight, MouseButton, SharedString, Window, div, prelude::*};
use kynex_core::session::Kind;
use kynex_core::storage::{self, Roots};

use super::KynexApp;
use crate::scratch::ScratchView;
use crate::theme::{mono, theme};
use crate::ui::motion::{self, fade, pop};
use crate::ui::{Icon, Tooltip, icon, icon_button};

#[derive(Clone)]
pub(super) struct ScratchModal {
    view: Entity<ScratchView>,
    title: SharedString,
}

impl KynexApp {
    fn open_scratch(&mut self, root: PathBuf, title: String, cx: &mut Context<Self>) {
        self.menu = None;
        let view = cx.new(|cx| ScratchView::new(root, cx));
        self.scratch = Some(ScratchModal { view, title: title.into() });
        cx.notify();
    }

    /// The shown session's scratch folder; nothing for a session not saved yet.
    pub(super) fn open_session_scratch(&mut self, cx: &mut Context<Self>) {
        let Some(s) = self.active_session().filter(|s| s.kind == Kind::Code) else { return };
        let (cwd, id) = (s.cwd.clone(), s.session_id.clone());
        let Some(root) = Roots::local().zip(id).and_then(|(r, id)| storage::session_scratch(&r, &cwd, &id)) else {
            return;
        };
        let title = self.active_session().and_then(|s| s.session_id.as_deref()).and_then(|id| self.session_name(id));
        self.open_scratch(
            root,
            title.map_or_else(|| "This session's scratch".into(), |t| format!("Scratch · {t}")),
            cx,
        );
    }

    /// The scratch folders of every session in the active project.
    pub(super) fn open_project_scratch(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.active_project().map(|p| p.path.clone()) else { return };
        let Some(roots) = Roots::local() else { return };
        let name = kynex_core::paths::project_name(&path);
        self.open_scratch(storage::project_scratch(&roots, &path), format!("Scratch · {name}, every session"), cx);
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
                .bg(gpui::black().opacity(0.45))
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
