//! Turning pasted, dropped or picked images into message attachments, and
//! finding the images in messages read back.

use std::io::Cursor;
use std::path::Path;

use base64::Engine;
use serde_json::Value;

/// Claude downsizes anything bigger, so don't send it.
pub const MAX_IMAGE_EDGE: u32 = 1568;

/// An image ready to send: PNG, no longer than [`MAX_IMAGE_EDGE`] on either side.
pub struct Png {
    pub bytes: Vec<u8>,
    pub base64: String,
}

/// Decodes any common image format, shrinks it if needed and re-encodes it as PNG.
pub fn prepare_image(bytes: &[u8]) -> Result<Png, String> {
    let img = image::load_from_memory(bytes).map_err(|e| format!("Couldn't read image: {e}"))?;
    let img = if img.width().max(img.height()) > MAX_IMAGE_EDGE {
        img.resize(MAX_IMAGE_EDGE, MAX_IMAGE_EDGE, image::imageops::FilterType::Triangle)
    } else {
        img
    };
    let mut png = Vec::new();
    img.to_rgba8()
        .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|e| format!("Couldn't encode image: {e}"))?;
    let base64 = base64::engine::general_purpose::STANDARD.encode(&png);
    Ok(Png { bytes: png, base64 })
}

/// Whether a dropped file should attach as an image (by extension).
pub fn is_image(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp"))
}

/// An image found in a message: its media type (`image/png`) and decoded bytes.
pub struct Found {
    pub media_type: String,
    pub bytes: Vec<u8>,
}

/// The base64 images among a message's content blocks, as sent images and
/// tool results (a `Read` of a PNG, a browser screenshot) carry them. Blocks
/// that aren't images, or don't decode, are skipped.
pub fn images_in(content: &Value) -> Vec<Found> {
    let engine = &base64::engine::general_purpose::STANDARD;
    content
        .as_array()
        .into_iter()
        .flatten()
        .filter(|b| b["type"] == "image" && b["source"]["type"] == "base64")
        .filter_map(|b| {
            let media_type = b["source"]["media_type"].as_str()?.to_string();
            let bytes = engine.decode(b["source"]["data"].as_str()?).ok()?;
            Some(Found { media_type, bytes })
        })
        .collect()
}

/// An image's width and height in pixels, read from its header.
pub fn dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    image::ImageReader::new(Cursor::new(bytes)).with_guessed_format().ok()?.into_dimensions().ok()
}

/// The size to draw an image of `size` in a `max_w` × `max_h` box: scaled down
/// to fit, keeping its shape, never up.
pub fn fit((w, h): (u32, u32), max_w: f32, max_h: f32) -> (f32, f32) {
    let (w, h) = (w.max(1) as f32, h.max(1) as f32);
    let scale = (max_w / w).min(max_h / h).min(1.);
    (w * scale, h * scale)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        image::RgbaImage::new(w, h).write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png).unwrap();
        bytes
    }

    #[test]
    fn fits_without_growing() {
        assert_eq!(fit((1600, 800), 320., 160.), (320., 160.));
        assert_eq!(fit((800, 1600), 320., 160.), (80., 160.));
        assert_eq!(fit((100, 50), 320., 160.), (100., 50.), "small images keep their size");
        assert_eq!(fit((0, 0), 320., 160.), (1., 1.));
    }

    #[test]
    fn finds_base64_images() {
        let data = base64::engine::general_purpose::STANDARD.encode(png(3, 2));
        let content = json!([
            { "type": "text", "text": "look" },
            { "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": data } },
            { "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": "not base64!" } },
            { "type": "image", "source": { "type": "url", "url": "https://x" } },
        ]);
        let found = images_in(&content);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].media_type, "image/png");
        assert_eq!(dimensions(&found[0].bytes), Some((3, 2)));
        assert!(images_in(&json!("text only")).is_empty());
    }

    #[test]
    fn dimensions_of_junk() {
        assert_eq!(dimensions(b"nope"), None);
    }

    #[test]
    fn shrinks_big_images() {
        let png = prepare_image(&png(3000, 1000)).unwrap();
        let out = image::load_from_memory(&png.bytes).unwrap();
        assert_eq!((out.width(), out.height()), (MAX_IMAGE_EDGE, 523));
    }

    #[test]
    fn image_extensions() {
        assert!(is_image(Path::new("a/B.JPG")));
        assert!(!is_image(Path::new("a/b.rs")));
    }
}
