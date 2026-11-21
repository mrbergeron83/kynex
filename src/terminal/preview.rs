//! Terminal image helpers: GPU texture cache for Kitty inline images, floating thumbnail
//! previews on `Ctrl+Hover` over image file paths, and clipboard image / file-drop pasting.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{
    AnyElement, ClipboardEntry, Context, ExternalPaths, ImageFormat, ObjectFit, RenderImage, Window, div, img,
    prelude::*,
};
use kynex_core::file_view::{self, Kind, media};
use kynex_core::terminal::{self, KittyImageData, LinkSpan};

use super::view::TerminalView;
use crate::theme::{Palette as Theme, mono};
use crate::ui::image_viewer::render_image;
use crate::ui::px;

/// Maximum pixel dimension for hover thumbnail previews (crisp on HiDPI, light on memory).
const THUMB_MAX_SIDE: u32 = 360;
/// Maximum cached GPU textures per terminal for Kitty inline images.
const MAX_TEXTURES: usize = 64;

type TextureKey = (u32, u64, Option<(u32, u32, u32, u32)>);

/// Per-terminal cache converting decoded Kitty RGBA buffers to GPUI `RenderImage`s once.
#[derive(Default)]
pub(super) struct ImageTextureCache {
    entries: HashMap<TextureKey, Arc<RenderImage>>,
    retired: Vec<Arc<RenderImage>>,
}

impl ImageTextureCache {
    pub(super) fn get_or_insert(
        &mut self,
        data: &KittyImageData,
        src_rect: Option<(u32, u32, u32, u32)>,
    ) -> Arc<RenderImage> {
        let key = (data.id, data.generation, src_rect);
        if let Some(img) = self.entries.get(&key) {
            return Arc::clone(img);
        }
        // Evict stale generations for the same image id and cap total cache entries.
        let stale: Vec<TextureKey> = self
            .entries
            .keys()
            .copied()
            .filter(|&(id, generation, _)| id == data.id && generation != data.generation)
            .collect();
        for k in stale {
            if let Some(old) = self.entries.remove(&k) {
                self.retired.push(old);
            }
        }
        if self.entries.len() >= MAX_TEXTURES
            && let Some(&first) = self.entries.keys().next()
            && let Some(old) = self.entries.remove(&first)
        {
            self.retired.push(old);
        }
        let rgba = match src_rect {
            Some((x, y, w, h)) => image::imageops::crop_imm(&data.rgba, x, y, w, h).to_image(),
            None => data.rgba.clone(),
        };
        let rendered = render_image(rgba);
        self.entries.insert(key, Arc::clone(&rendered));
        rendered
    }

    /// Drops retired GPU textures from the window's sprite atlas.
    pub(super) fn flush_retired(&mut self, window: &mut Window) {
        for img in self.retired.drain(..) {
            let _ = window.drop_image(img);
        }
    }

    pub(super) fn clear(&mut self) {
        self.retired.extend(self.entries.drain().map(|(_, v)| v));
    }
}

/// A decoded thumbnail ready to display over a `Ctrl+Hover`ed image link.
#[derive(Clone)]
pub(super) struct HoveredImage {
    pub path: PathBuf,
    pub image: Arc<RenderImage>,
    pub dimensions: (u32, u32),
    pub anchor_line: usize,
    pub anchor_col: usize,
}

/// Decodes an image file on a background thread into a compact thumbnail for hover preview.
pub(super) fn load_hover_preview(path: &Path) -> Option<(Arc<RenderImage>, (u32, u32))> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    let kind = Kind::of(&path.to_string_lossy(), &[]);
    let limit = match kind {
        Kind::Image => file_view::MAX_IMAGE,
        Kind::Svg => file_view::MAX_TEXT,
        _ => return None,
    };
    if meta.len() == 0 || meta.len() > limit {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    let pic = match kind {
        Kind::Image => media::decode_image(&bytes).ok()?,
        Kind::Svg => media::render_svg(&bytes).ok()?,
        _ => return None,
    };
    let (w, h) = pic.rgba.dimensions();
    let rgba = if w.max(h) > THUMB_MAX_SIDE {
        let scale = THUMB_MAX_SIDE as f32 / w.max(h) as f32;
        let nw = ((w as f32 * scale).round() as u32).max(1);
        let nh = ((h as f32 * scale).round() as u32).max(1);
        image::DynamicImage::ImageRgba8(pic.rgba).resize_exact(nw, nh, image::imageops::FilterType::Triangle).to_rgba8()
    } else {
        pic.rgba
    };
    Some((render_image(rgba), pic.size))
}

fn image_format_ext(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Png => "png",
        ImageFormat::Jpeg => "jpg",
        ImageFormat::Webp => "webp",
        ImageFormat::Gif => "gif",
        ImageFormat::Bmp => "bmp",
        ImageFormat::Svg => "svg",
        _ => "png",
    }
}

impl TerminalView {
    /// Handles clipboard paste: prefers text, then copied file paths (`ExternalPaths`),
    /// then clipboard images (saved off the UI thread to `<data dir>/pasted/` and pasted as a path).
    pub(super) fn paste_from_clipboard(&mut self, cx: &mut Context<Self>) {
        if self.terminal.is_none() {
            return;
        }
        let Some(item) = cx.read_from_clipboard() else { return };

        let mut text = String::new();
        let mut paths: Vec<PathBuf> = Vec::new();
        let mut image_entry: Option<(Vec<u8>, &'static str)> = None;

        for entry in item.entries() {
            match entry {
                ClipboardEntry::String(s) if !s.text().is_empty() => text.push_str(s.text()),
                ClipboardEntry::ExternalPaths(p) => paths.extend(p.paths().iter().cloned()),
                ClipboardEntry::Image(img) if image_entry.is_none() => {
                    image_entry = Some((img.bytes.clone(), image_format_ext(img.format)));
                }
                _ => {}
            }
        }

        if !text.is_empty() {
            if let Some(t) = &self.terminal {
                t.paste(&text);
            }
            return;
        }
        if !paths.is_empty() {
            let formatted = terminal::format_paths_for_paste(&paths);
            if !formatted.is_empty()
                && let Some(t) = &self.terminal
            {
                t.paste(&formatted);
            }
            return;
        }
        if let Some((bytes, ext)) = image_entry {
            cx.spawn(async move |this, cx| {
                let saved = cx.background_spawn(async move { terminal::save_pasted_image(&bytes, ext) }).await;
                this.update(cx, |this, cx| match saved {
                    Ok(path) => {
                        let formatted = terminal::format_paths_for_paste(&[path]);
                        if let Some(t) = &this.terminal {
                            t.paste(&formatted);
                        }
                        cx.notify();
                    }
                    Err(e) => eprintln!("kynex: image paste failed: {e}"),
                })
                .ok();
            })
            .detach();
        }
    }

    /// Pastes dropped files/images from the OS file manager into the terminal PTY.
    pub(super) fn drop_external_paths(&mut self, paths: &ExternalPaths, window: &mut Window, cx: &mut Context<Self>) {
        self.focus.focus(window, cx);
        let formatted = terminal::format_paths_for_paste(paths.paths());
        if !formatted.is_empty()
            && let Some(t) = &self.terminal
        {
            t.paste(&formatted);
            cx.notify();
        }
    }

    /// Loads or reuses a cached hover thumbnail when `Ctrl+Hover`ing an image file link.
    pub(super) fn load_hovered_image(&mut self, path: PathBuf, spans: &[LinkSpan], seq: u64, cx: &mut Context<Self>) {
        if !terminal::is_image_path(&path) {
            self.hovered_image = None;
            return;
        }
        let (anchor_line, anchor_col) = spans.first().map_or((0, 0), |s| (s.line, s.start_col));
        if let Some(prev) = &mut self.hovered_image
            && prev.path == path
        {
            prev.anchor_line = anchor_line;
            prev.anchor_col = anchor_col;
            return;
        }
        cx.spawn(async move |this, cx| {
            let loaded = cx
                .background_spawn({
                    let path = path.clone();
                    async move { load_hover_preview(&path) }
                })
                .await;
            this.update(cx, |this, cx| {
                if this.hover_gen != seq {
                    return;
                }
                if let Some((image, dimensions)) = loaded {
                    this.hovered_image = Some(HoveredImage { path, image, dimensions, anchor_line, anchor_col });
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Renders the floating thumbnail card anchored near the hovered image path link.
    pub(super) fn render_hover_preview(&self, t: &Theme) -> Option<AnyElement> {
        let preview = self.hovered_image.as_ref()?;
        let (_, cell) = self.layout?;
        let name = preview
            .path
            .file_name()
            .map_or_else(|| preview.path.display().to_string(), |n| n.to_string_lossy().into_owned());
        let meta = format!("{} × {} · Click to open", preview.dimensions.0, preview.dimensions.1);
        let pad = gpui::px(10.);
        let card_h = gpui::px(216.);
        let anchor_y = pad + cell.height * preview.anchor_line as f32;
        let anchor_x = (pad + cell.width * preview.anchor_col as f32).max(gpui::px(8.));
        let place_below = anchor_y < card_h + gpui::px(12.);
        let top = if place_below { anchor_y + cell.height + gpui::px(6.) } else { anchor_y - card_h - gpui::px(6.) };

        Some(
            div()
                .absolute()
                .top(top)
                .left(anchor_x)
                .w(px(268.))
                .p_1p5()
                .rounded_md()
                .bg(t.elevated)
                .border_1()
                .border_color(t.border)
                .shadow_md()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .w_full()
                        .h(px(164.))
                        .rounded_sm()
                        .bg(t.panel)
                        .overflow_hidden()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            img(Arc::clone(&preview.image)).max_w_full().max_h(px(164.)).object_fit(ObjectFit::Contain),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .px_0p5()
                        .child(div().text_xs().font_family(mono()).text_color(t.text).truncate().child(name))
                        .child(div().text_size(px(10.)).text_color(t.text_muted).child(meta)),
                )
                .into_any_element(),
        )
    }
}
