//! A session's files besides its scratch folder, as sections at the top of
//! the explorer's left side: the project files it edited (marked while git
//! still has changes for them), the images in its conversation, and its
//! background tasks' output.

use gpui::{AnyElement, Context, SharedString, div, prelude::*};
use kynex_core::fmt::ago;

use super::{Pane, ScratchEvent, ScratchView, entry_row, meta, nav_row, note, scrolling};
use crate::theme::{Palette, mono};
use crate::ui::{Icon, icon};

impl ScratchView {
    /// The sections, for a session; nothing for a project's explorer.
    pub(super) fn section_rows(&self, t: &Palette, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(extras) = &self.extras else { return Vec::new() };
        let f = &extras.files;
        let sections = [
            (Pane::Edited, Icon::Code, "Edited in the project", f.edited.len()),
            (Pane::Images, Icon::File, "Images", f.images.len()),
            (Pane::Outputs, Icon::Terminal, "Task output", f.outputs.len()),
        ];
        let mut rows: Vec<AnyElement> = sections
            .into_iter()
            .map(|(pane, glyph, label, n)| {
                let chosen = self.pane == pane;
                nav_row(SharedString::from(format!("section-{label}")), chosen, 0, t)
                    .child(div().flex_none().size(gpui::px(14.)))
                    .child(icon(glyph, if chosen { t.accent } else { t.text_muted }).size_3p5().flex_none())
                    .child(div().flex_1().min_w_0().truncate().child(label))
                    .child(div().flex_none().text_xs().text_color(t.text_placeholder).child(n.to_string()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.pane = pane.clone();
                        cx.notify();
                    }))
                    .into_any_element()
            })
            .collect();
        rows.push(div().my_1().mx_2().h(gpui::px(1.)).bg(t.border_variant).into_any_element());
        rows
    }

    pub(super) fn edited_entries(&self, t: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let Some(extras) = &self.extras else { return note("", t) };
        if extras.files.edited.is_empty() {
            return note("This session hasn't edited any project files.", t);
        }
        let rows = extras.files.edited.iter().enumerate().map(|(i, path)| {
            let rel = path.strip_prefix(&extras.cwd).unwrap_or(path).display().to_string();
            let changed = self.changed.contains(path);
            let gone = !path.exists();
            let p = path.clone();
            entry_row(("edited", i), Icon::File, rel, t)
                .font_family(mono())
                .child(meta(
                    match (gone, changed) {
                        (true, _) => "deleted".into(),
                        (false, true) => "changed".into(),
                        (false, false) => "committed".into(),
                    },
                    84.,
                    t,
                ))
                .on_click(cx.listener(move |this, _, _, cx| this.open_file(p.clone(), true, cx)))
                .into_any_element()
        });
        scrolling("scratch-edited", rows.collect())
    }

    pub(super) fn image_entries(&self, t: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let Some(extras) = &self.extras else { return note("", t) };
        if extras.files.images.is_empty() {
            return note("No images in this conversation: screenshots, pictures Claude read, ones you pasted.", t);
        }
        let rows = extras.files.images.iter().enumerate().map(|(i, img)| {
            let (item, index) = (img.item, img.index);
            entry_row(("image", i), Icon::File, img.label.clone(), t)
                .child(meta("open".into(), 60., t))
                .on_click(cx.listener(move |_, _, _, cx| cx.emit(ScratchEvent::OpenImage { item, index })))
                .into_any_element()
        });
        scrolling("scratch-images", rows.collect())
    }

    pub(super) fn output_entries(&self, t: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let Some(extras) = &self.extras else { return note("", t) };
        if extras.files.outputs.is_empty() {
            return note("No background tasks have written output in this session.", t);
        }
        let rows = extras.files.outputs.iter().enumerate().map(|(i, path)| {
            let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into());
            let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok();
            let p = path.clone();
            entry_row(("output", i), Icon::Terminal, name, t)
                .child(meta(modified.map(ago).unwrap_or_else(|| "gone".into()), 84., t))
                .on_click(cx.listener(move |this, _, _, cx| this.open_file(p.clone(), false, cx)))
                .into_any_element()
        });
        scrolling("scratch-outputs", rows.collect())
    }
}
