//! Image conversion helper for the Git pane's file viewer.

use std::sync::Arc;

use gpui::RenderImage;

/// An RGBA picture as gpui draws it (BGRA).
pub fn render_image(mut rgba: image::RgbaImage) -> Arc<RenderImage> {
    for pixel in rgba.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    Arc::new(RenderImage::new(vec![image::Frame::new(rgba)]))
}
