//! Image decoding, thumbnail generation, base64 encoding, lazy `.jsonl` image
//! loading, and deterministic GPU `RenderImage` texture cache cleanup via
//! `cx.drop_image`.

use std::collections::{HashMap, HashSet};
use std::io::Cursor;
use std::path::Path;
use std::sync::Arc;

use gpui::{
    AnyElement, App, AppContext, Context, Image as GpuiImage, ImageFormat as GpuiImageFormat, ObjectFit, RenderImage,
    SharedString, div, img, prelude::*,
};
use image::{DynamicImage, ImageFormat, RgbaImage};
use kynex_core::claude::history::{LoggedImageRef, load_logged_image};
use kynex_core::file_view::{self, Kind, media as core_media};

use super::ClaudeChatView;
use super::state::{ImageKey, LoggedKey, RowImage};
use crate::theme::Palette;
use crate::ui::image_viewer::render_image;
use crate::ui::{Tooltip, px};

/// Maximum pixel dimension for composer / transcript thumbnails.
const THUMB_MAX_SIDE: u32 = 220;

/// A decoded image cached for rendering (thumbnail + optional full-res lightbox texture).
#[derive(Clone)]
pub struct CachedImage {
    pub thumb: Arc<RenderImage>,
    pub full: Option<Arc<RenderImage>>,
    pub size: (u32, u32),
    pub label: String,
}

/// Decoded attachment ready to be placed in the composer and encoded for the CLI.
pub struct DecodedAttachment {
    pub media_type: String,
    pub base64: String,
    pub size: (u32, u32),
    pub thumb: Arc<RenderImage>,
    pub full: Option<Arc<RenderImage>>,
    pub label: String,
}

/// Per-view GPU texture cache with deterministic `cx.drop_image` cleanup.
#[derive(Default)]
pub struct ImageCache {
    inline: HashMap<u64, CachedImage>,
    logged: HashMap<LoggedKey, CachedImage>,
    loading_logged: HashSet<LoggedKey>,
    retired: Vec<Arc<RenderImage>>,
}

impl ImageCache {
    pub fn insert_inline(&mut self, id: u64, img: CachedImage) {
        if let Some(old) = self.inline.insert(id, img) {
            self.retire_cached(old);
        }
    }

    pub fn insert_logged(&mut self, key: LoggedKey, img: CachedImage) {
        self.loading_logged.remove(&key);
        if let Some(old) = self.logged.insert(key, img) {
            self.retire_cached(old);
        }
    }

    pub fn finish_logged_loading(&mut self, key: &LoggedKey) {
        self.loading_logged.remove(key);
    }

    pub fn mark_logged_loading(&mut self, key: LoggedKey) -> bool {
        if self.logged.contains_key(&key) {
            return false;
        }
        self.loading_logged.insert(key)
    }

    pub fn get(&self, key: &ImageKey) -> Option<&CachedImage> {
        match key {
            ImageKey::Inline(id) => self.inline.get(id),
            ImageKey::Logged(k) => self.logged.get(k),
        }
    }

    fn retire_cached(&mut self, img: CachedImage) {
        self.retired.push(img.thumb);
        if let Some(full) = img.full {
            self.retired.push(full);
        }
    }

    /// Drops retired GPU textures from the app's sprite atlases.
    pub fn flush_retired(&mut self, cx: &mut App) {
        for img in self.retired.drain(..) {
            cx.drop_image(img, None);
        }
    }

    /// Drops EVERY cached and retired `RenderImage` texture deterministically.
    pub fn release_all(&mut self, cx: &mut App) {
        for (_, img) in self.inline.drain() {
            cx.drop_image(img.thumb, None);
            if let Some(full) = img.full {
                cx.drop_image(full, None);
            }
        }
        for (_, img) in self.logged.drain() {
            cx.drop_image(img.thumb, None);
            if let Some(full) = img.full {
                cx.drop_image(full, None);
            }
        }
        self.loading_logged.clear();
        self.flush_retired(cx);
    }
}

fn downscale_rgba(rgba: &RgbaImage, max_side: u32) -> RgbaImage {
    let (w, h) = rgba.dimensions();
    if w.max(h) <= max_side {
        return rgba.clone();
    }
    let scale = max_side as f32 / w.max(h) as f32;
    let nw = ((w as f32 * scale).round() as u32).max(1);
    let nh = ((h as f32 * scale).round() as u32).max(1);
    DynamicImage::ImageRgba8(rgba.clone()).resize_exact(nw, nh, image::imageops::FilterType::Triangle).to_rgba8()
}

fn build_cached(rgba: RgbaImage, size: (u32, u32), label: String) -> CachedImage {
    let (w, h) = rgba.dimensions();
    if w.max(h) <= THUMB_MAX_SIDE {
        let tex = render_image(rgba);
        CachedImage { thumb: tex, full: None, size, label }
    } else {
        let thumb = render_image(downscale_rgba(&rgba, THUMB_MAX_SIDE));
        let full = render_image(rgba);
        CachedImage { thumb, full: Some(full), size, label }
    }
}

fn detect_media_type(bytes: &[u8], hint: Option<&str>) -> &'static str {
    if let Some(h) = hint {
        match h.to_ascii_lowercase().as_str() {
            "image/png" => return "image/png",
            "image/jpeg" | "image/jpg" => return "image/jpeg",
            "image/webp" => return "image/webp",
            "image/gif" => return "image/gif",
            _ => {}
        }
    }
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]) {
        "image/png"
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        "image/jpeg"
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        "image/gif"
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        "image/webp"
    } else {
        "image/png"
    }
}

fn ensure_anthropic_format(bytes: &[u8], rgba: &RgbaImage, hint: Option<&str>) -> Result<(String, Vec<u8>), String> {
    let mt = detect_media_type(bytes, hint);
    // If bytes are already a valid Anthropic-supported raster format, keep original bytes when not BMP/ICO/SVG.
    let is_native = matches!(
        (
            bytes.starts_with(&[0x89, b'P', b'N', b'G']),
            bytes.starts_with(&[0xFF, 0xD8, 0xFF]),
            bytes.starts_with(b"GIF8"),
            bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP",
        ),
        (true, _, _, _) | (_, true, _, _) | (_, _, true, _) | (_, _, _, true)
    );
    if is_native {
        return Ok((mt.to_string(), bytes.to_vec()));
    }
    // Re-encode BMP / ICO / SVG / raw into PNG for Claude's base64 image API.
    let mut png = Vec::new();
    DynamicImage::ImageRgba8(rgba.clone())
        .write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
        .map_err(|e| format!("Couldn't encode image as PNG: {e}"))?;
    Ok(("image/png".to_string(), png))
}

/// Decodes an image from raw bytes into a composer attachment (blocking: run off UI thread).
pub fn decode_attachment_from_bytes(
    bytes: &[u8],
    media_type_hint: Option<&str>,
    label: String,
) -> Result<DecodedAttachment, String> {
    if bytes.is_empty() {
        return Err("Image is empty".into());
    }
    if bytes.len() as u64 > file_view::MAX_IMAGE {
        return Err(format!("Image is too large ({})", file_view::size_label(bytes.len() as u64)));
    }
    let pic = core_media::decode_image(bytes)?;
    let (media_type, encoded_bytes) = ensure_anthropic_format(bytes, &pic.rgba, media_type_hint)?;
    let b64 = encode_base64(&encoded_bytes);
    let cached = build_cached(pic.rgba, pic.size, label.clone());
    Ok(DecodedAttachment { media_type, base64: b64, size: cached.size, thumb: cached.thumb, full: cached.full, label })
}

/// Decodes a clipboard `gpui::Image` into a composer attachment (blocking: run off UI thread).
pub fn decode_clipboard_image(img: &GpuiImage, index: usize) -> Result<DecodedAttachment, String> {
    let hint = match img.format {
        GpuiImageFormat::Png => Some("image/png"),
        GpuiImageFormat::Jpeg => Some("image/jpeg"),
        GpuiImageFormat::Webp => Some("image/webp"),
        GpuiImageFormat::Gif => Some("image/gif"),
        _ => None,
    };
    decode_attachment_from_bytes(&img.bytes, hint, format!("Clipboard image #{index}"))
}

/// Decodes an image file from disk into a composer attachment (blocking: run off UI thread).
pub fn decode_attachment_from_path(path: &Path) -> Result<DecodedAttachment, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("Couldn't read {}: {e}", path.display()))?;
    if !meta.is_file() {
        return Err(format!("{} is not a file", path.display()));
    }
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("image").to_string();
    let kind = Kind::of(&path.to_string_lossy(), &[]);
    match kind {
        Kind::Image => {
            if meta.len() > file_view::MAX_IMAGE {
                return Err(format!("{name} is too large ({})", file_view::size_label(meta.len())));
            }
            let bytes = std::fs::read(path).map_err(|e| format!("Couldn't read {name}: {e}"))?;
            decode_attachment_from_bytes(&bytes, None, name)
        }
        Kind::Svg => {
            if meta.len() > file_view::MAX_TEXT {
                return Err(format!("{name} is too large"));
            }
            let bytes = std::fs::read(path).map_err(|e| format!("Couldn't read {name}: {e}"))?;
            let pic = core_media::render_svg(&bytes)?;
            let (media_type, png) = ensure_anthropic_format(&[], &pic.rgba, Some("image/png"))?;
            let b64 = encode_base64(&png);
            let cached = build_cached(pic.rgba, pic.size, name.clone());
            Ok(DecodedAttachment {
                media_type,
                base64: b64,
                size: cached.size,
                thumb: cached.thumb,
                full: cached.full,
                label: name,
            })
        }
        _ => Err(format!("{name} is not a supported image")),
    }
}

/// Decodes a base64 image from a live tool result (blocking: run off UI thread).
pub fn decode_inline_base64(b64: &str, label: String) -> Option<CachedImage> {
    let bytes = decode_base64(b64)?;
    let pic = core_media::decode_image(&bytes).ok()?;
    Some(build_cached(pic.rgba, pic.size, label))
}

/// Lazily reads & decodes a `.jsonl` `LoggedImageRef` off the UI thread.
pub fn load_logged_image_cached(img_ref: &LoggedImageRef, label: String) -> Option<CachedImage> {
    let (_mt, bytes) = load_logged_image(img_ref)?;
    let pic = core_media::decode_image(&bytes).ok()?;
    Some(build_cached(pic.rgba, pic.size, label))
}

/// Returns `true` when `path` looks like an image file we can attach.
pub fn is_supported_image_path(path: &Path) -> bool {
    matches!(Kind::of(&path.to_string_lossy(), &[]), Kind::Image | Kind::Svg)
}

impl ClaudeChatView {
    pub(super) fn ensure_logged_image_loaded(
        &mut self,
        img_ref: LoggedImageRef,
        label: String,
        cx: &mut Context<Self>,
    ) {
        let key = LoggedKey::from(&img_ref);
        if !self.image_cache.mark_logged_loading(key.clone()) {
            return;
        }
        cx.spawn(async move |this, cx| {
            let loaded = cx.background_spawn(async move { load_logged_image_cached(&img_ref, label) }).await;
            this.update(cx, |this, cx| {
                if let Some(cached) = loaded {
                    this.image_cache.insert_logged(key, cached);
                } else {
                    this.image_cache.finish_logged_loading(&key);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn release_all_images(&mut self, cx: &mut App) {
        self.close_lightbox(cx);
        for att in self.attachments.drain(..) {
            cx.drop_image(att.thumb, None);
            if let Some(full) = att.full {
                cx.drop_image(full, None);
            }
        }
        self.image_cache.release_all(cx);
    }

    pub(super) fn render_row_images(
        &mut self,
        row_id: u64,
        images: &[RowImage],
        t: &Palette,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut out = Vec::new();
        for (idx, img_ref) in images.iter().enumerate() {
            if let Some(cached) = self.image_cache.get(&img_ref.key).cloned() {
                let thumb = cached.thumb.clone();
                let full = cached.full.clone().unwrap_or_else(|| cached.thumb.clone());
                let size = cached.size;
                let label = cached.label.clone();
                out.push(
                    div()
                        .id(SharedString::from(format!("rimg-{row_id}-{idx}")))
                        .w(px(72.))
                        .h(px(54.))
                        .rounded_md()
                        .border_1()
                        .border_color(t.border)
                        .bg(t.element)
                        .overflow_hidden()
                        .cursor_pointer()
                        .hover(|s| s.border_color(t.border_focused))
                        .tooltip(Tooltip::with_meta(label.clone(), format!("{} × {} · Click to zoom", size.0, size.1)))
                        .child(img(thumb).size_full().object_fit(ObjectFit::Cover))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.open_lightbox(full.clone(), size, label.clone(), false, cx);
                        }))
                        .into_any_element(),
                );
            } else {
                if let Some(logged) = &img_ref.logged_ref {
                    self.ensure_logged_image_loaded(logged.clone(), img_ref.label.clone(), cx);
                }
                out.push(
                    div()
                        .w(px(72.))
                        .h(px(54.))
                        .rounded_md()
                        .border_1()
                        .border_color(t.border_variant)
                        .bg(t.element)
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(10.))
                        .text_color(t.text_placeholder)
                        .child("Loading…")
                        .into_any_element(),
                );
            }
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Compact RFC 4648 standard Base64 encoder/decoder (zero extra crate dep)
// ---------------------------------------------------------------------------

const B64_TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn encode_base64(input: &[u8]) -> String {
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = if chunk.len() > 1 { u32::from(chunk[1]) } else { 0 };
        let b2 = if chunk.len() > 2 { u32::from(chunk[2]) } else { 0 };
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(B64_TABLE[((n >> 18) & 0x3F) as usize] as char);
        out.push(B64_TABLE[((n >> 12) & 0x3F) as usize] as char);
        out.push(if chunk.len() > 1 { B64_TABLE[((n >> 6) & 0x3F) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { B64_TABLE[(n & 0x3F) as usize] as char } else { '=' });
    }
    out
}

pub fn decode_base64(input: &str) -> Option<Vec<u8>> {
    let mut vals = Vec::with_capacity(input.len());
    for b in input.bytes() {
        if b.is_ascii_whitespace() {
            continue;
        }
        if b == b'=' {
            vals.push(0xFFu8);
            continue;
        }
        let v = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        vals.push(v);
    }
    if !vals.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::with_capacity(vals.len() / 4 * 3);
    for &[a, b, c, d] in vals.as_chunks::<4>().0 {
        if a == 0xFF || b == 0xFF {
            return None;
        }
        out.push((a << 2) | (b >> 4));
        if c != 0xFF {
            out.push((b << 4) | (c >> 2));
            if d != 0xFF {
                out.push((c << 6) | d);
            }
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trips_rfc4648_vectors() {
        for (plain, expected) in [
            (&b""[..], ""),
            (b"f", "Zg=="),
            (b"fo", "Zm8="),
            (b"foo", "Zm9v"),
            (b"foob", "Zm9vYg=="),
            (b"fooba", "Zm9vYmE="),
            (b"foobar", "Zm9vYmFy"),
        ] {
            let enc = encode_base64(plain);
            assert_eq!(enc, expected);
            assert_eq!(decode_base64(&enc).unwrap(), plain);
        }
        assert!(decode_base64("Zm9v\nYmFy").unwrap() == b"foobar");
        assert!(decode_base64("!!!").is_none());
    }
}
