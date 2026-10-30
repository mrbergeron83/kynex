//! The preview over the explorer: highlighted text (markdown rendered),
//! pictures, or a note for anything else; with Go to file (shown in the OS
//! file manager), Open, Open in editor, Copy path, and for a project file the
//! session edited, Show changes.

use gpui::{AnyElement, Context, MouseButton, ObjectFit, StyledImage, StyledText, div, img, prelude::*};
use kynex_core::files::Preview;
use kynex_core::fmt::bytes;
use kynex_core::highlight::Lang;
use kynex_core::open;

use super::{ScratchEvent, ScratchView};
use crate::theme::{Palette, mono};
use crate::ui::motion::{self, fade, pop};
use crate::ui::{Icon, Tooltip, code, ghost_button, icon, icon_button, px, scrollbar};

/// The markdown preview's selection group: apart from every transcript's.
const PREVIEW_GROUP: u64 = u64::MAX - 7;

impl ScratchView {
    pub(super) fn preview(&self, t: &Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (opened, preview) = self.preview.clone()?;
        let path = opened.path.clone();
        let (reveal, open_it, edit, copy, changes) =
            (path.clone(), path.clone(), path.clone(), path.display().to_string(), path.clone());
        let is_text = matches!(preview, Some(Preview::Text { .. }));
        let header = div()
            .flex()
            .flex_none()
            .items_center()
            .gap_1()
            .h(px(40.))
            .px_3()
            .border_b_1()
            .border_color(t.border_variant)
            .child(icon(Icon::File, t.text_muted).flex_none())
            .child(div().flex_none().pl_1().text_sm().child(opened.name.clone()))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .px_2()
                    .truncate()
                    .font_family(mono())
                    .text_xs()
                    .text_color(t.text_placeholder)
                    .child(match opened.size {
                        Some(n) => format!("{} · {}", bytes(n), path.display()),
                        None => path.display().to_string(),
                    }),
            )
            .when(opened.edited, |d| {
                d.child(
                    ghost_button("preview-changes", cx)
                        .text_xs()
                        .child("Show changes")
                        .tooltip(Tooltip::text("Its diff, in the Git view"))
                        .on_click(cx.listener(move |_, _, _, cx| cx.emit(ScratchEvent::ShowChanges(changes.clone())))),
                )
            })
            .child(
                ghost_button("preview-reveal", cx)
                    .text_xs()
                    .child("Go to file")
                    .tooltip(Tooltip::text("Show it in your file manager"))
                    .on_click(cx.listener(move |this, _, _, cx| this.os(open::reveal(&reveal), cx))),
            )
            .when(is_text, |d| {
                d.child(
                    ghost_button("preview-edit", cx)
                        .text_xs()
                        .child("Open in editor")
                        .on_click(cx.listener(move |this, _, _, cx| this.os(open::open_file(&edit, None, None), cx))),
                )
            })
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
                let markdown = matches!(
                    path.extension().map(|e| e.to_string_lossy().to_lowercase()).as_deref(),
                    Some("md" | "markdown")
                );
                let content = if markdown {
                    div().text_sm().child(crate::markdown::render(
                        "scratch-md",
                        &text,
                        PREVIEW_GROUP,
                        0,
                        None,
                        None,
                        cx,
                    ))
                } else {
                    let styles = code::highlighted(Lang::from_path(&opened.name), &text, &t.syntax);
                    div().font_family(mono()).text_xs().child(StyledText::new(text).with_highlights(styles))
                };
                let handle = scrollbar::handle("scratch-preview");
                let area = div()
                    .id("scratch-preview")
                    .size_full()
                    .overflow_scroll()
                    .track_scroll(&handle)
                    .p_3()
                    .child(content)
                    .when(cut, |d| {
                        d.child(
                            div()
                                .pt_2()
                                .text_xs()
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
        Some(fade(
            "scratch-preview-fade",
            div()
                .id("scratch-preview-backdrop")
                .absolute()
                .inset_0()
                .p_6()
                .bg(crate::theme::theme(cx).scrim())
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
        ))
    }
}
