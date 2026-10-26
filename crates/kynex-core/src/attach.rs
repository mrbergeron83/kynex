//! Turning pasted, dropped or picked images into message attachments.

use std::io::Cursor;
use std::path::Path;

use base64::Engine;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shrinks_big_images() {
        let big = image::RgbaImage::new(3000, 1000);
        let mut bytes = Vec::new();
        big.write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png).unwrap();
        let png = prepare_image(&bytes).unwrap();
        let out = image::load_from_memory(&png.bytes).unwrap();
        assert_eq!((out.width(), out.height()), (MAX_IMAGE_EDGE, 523));
    }

    #[test]
    fn image_extensions() {
        assert!(is_image(Path::new("a/B.JPG")));
        assert!(!is_image(Path::new("a/b.rs")));
    }
}
