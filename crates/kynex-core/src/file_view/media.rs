//! Pictures for the file viewer: raster images decoded and SVGs drawn, both to RGBA,
//! within limits so a huge picture or a decompression bomb gives a message instead of
//! eating the machine. Nothing in a file is ever run or fetched: an SVG's scripts are
//! never read, and its references to other files (images, fonts by URL) are refused.

use std::io::Cursor;
use std::sync::{Arc, OnceLock};

use image::{ImageReader, Limits, RgbaImage};

/// The most pixels a raster image may have to be decoded (about 8000 × 8000).
pub const MAX_PIXELS: u64 = 64_000_000;
/// The longest side a picture is kept at for drawing; bigger ones are scaled down.
pub const DRAW_SIDE: u32 = 4096;
/// The longest side an SVG is drawn at.
const SVG_SIDE: f32 = 2048.;

/// A picture decoded for drawing.
#[derive(Clone, Debug)]
pub struct Picture {
    /// Straight (not premultiplied) RGBA, at most [`DRAW_SIDE`] on its longest side.
    pub rgba: RgbaImage,
    /// Its own size in pixels, before any scaling down.
    pub size: (u32, u32),
}

/// Decodes a raster image (png, jpeg, gif's first frame, webp, bmp, ico). Blocking.
pub fn decode_image(bytes: &[u8]) -> Result<Picture, String> {
    let reader = || {
        ImageReader::new(Cursor::new(bytes)).with_guessed_format().map_err(|e| format!("Couldn't read the image: {e}"))
    };
    let (w, h) = reader()?.into_dimensions().map_err(|e| format!("Couldn't read the image: {e}"))?;
    if u64::from(w) * u64::from(h) > MAX_PIXELS {
        return Err(format!("The image is too big to show ({w} × {h} pixels)"));
    }
    let mut r = reader()?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(w.max(1));
    limits.max_image_height = Some(h.max(1));
    limits.max_alloc = Some(MAX_PIXELS * 8);
    r.limits(limits);
    let img = r.decode().map_err(|e| format!("Couldn't read the image: {e}"))?;
    Ok(Picture { rgba: fit_side(img).to_rgba8(), size: (w, h) })
}

fn fit_side(img: image::DynamicImage) -> image::DynamicImage {
    let (w, h) = (img.width(), img.height());
    if w.max(h) <= DRAW_SIDE {
        return img;
    }
    let scale = DRAW_SIDE as f32 / w.max(h) as f32;
    let (nw, nh) = (((w as f32 * scale).round() as u32).max(1), ((h as f32 * scale).round() as u32).max(1));
    img.resize_exact(nw, nh, image::imageops::FilterType::Triangle)
}

/// The system's fonts, read once, for text in SVGs.
fn fonts() -> Arc<resvg::usvg::fontdb::Database> {
    static FONTS: OnceLock<Arc<resvg::usvg::fontdb::Database>> = OnceLock::new();
    FONTS
        .get_or_init(|| {
            let mut db = resvg::usvg::fontdb::Database::new();
            db.load_system_fonts();
            Arc::new(db)
        })
        .clone()
}

/// Draws an SVG as a static picture, at most 2048 pixels on its longest side. Its
/// `<image>`s are left out, whatever they point at (a file, a URL, inline data), so
/// reading it touches nothing else. Blocking (the first call reads the system's fonts).
pub fn render_svg(bytes: &[u8]) -> Result<Picture, String> {
    render_svg_with(bytes, fonts())
}

fn render_svg_with(bytes: &[u8], fontdb: Arc<resvg::usvg::fontdb::Database>) -> Result<Picture, String> {
    use resvg::usvg::{ImageHrefResolver, Options, Tree};
    let opts = Options {
        resources_dir: None,
        image_href_resolver: ImageHrefResolver {
            resolve_data: Box::new(|_, _, _| None),
            resolve_string: Box::new(|_, _| None),
        },
        fontdb,
        ..Options::default()
    };
    let tree = Tree::from_data(bytes, &opts).map_err(|e| format!("Couldn't read the SVG: {e}"))?;
    let size = tree.size();
    let (w, h) = (size.width(), size.height());
    if !(w.is_finite() && h.is_finite()) || w <= 0. || h <= 0. {
        return Err("The SVG has no size".into());
    }
    let scale = SVG_SIDE / w.max(h);
    let (pw, ph) = (((w * scale).round() as u32).max(1), ((h * scale).round() as u32).max(1));
    let mut pixmap = resvg::tiny_skia::Pixmap::new(pw, ph).ok_or("Couldn't draw the SVG")?;
    resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    // tiny-skia keeps premultiplied pixels.
    let mut data = pixmap.take();
    for px in data.as_chunks_mut::<4>().0 {
        let a = u32::from(px[3]);
        if a > 0 && a < 255 {
            for c in &mut px[..3] {
                *c = ((u32::from(*c) * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    let rgba = RgbaImage::from_raw(pw, ph, data).ok_or("Couldn't draw the SVG")?;
    Ok(Picture { rgba, size: (w.round() as u32, h.round() as u32) })
}

/// Fuzzing: an SVG drawn without the system's fonts (reading them is slow).
pub(crate) fn fuzz_svg(bytes: &[u8]) {
    let _ = render_svg_with(bytes, Arc::new(resvg::usvg::fontdb::Database::new()));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let mut out = Vec::new();
        RgbaImage::new(w, h).write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
        out
    }

    #[test]
    fn decodes_and_scales_down_big_images() {
        let p = decode_image(&png(3, 2)).unwrap();
        assert_eq!((p.size, p.rgba.dimensions()), ((3, 2), (3, 2)));
        let p = decode_image(&png(DRAW_SIDE * 2, 10)).unwrap();
        assert_eq!((p.size, p.rgba.dimensions()), ((DRAW_SIDE * 2, 10), (DRAW_SIDE, 5)));
    }

    #[test]
    fn refuses_bombs_and_garbage() {
        // A PNG header claiming 60000 × 60000 pixels, with no data behind it.
        let mut bomb = png(1, 1);
        bomb[16..20].copy_from_slice(&60000u32.to_be_bytes());
        bomb[20..24].copy_from_slice(&60000u32.to_be_bytes());
        assert!(decode_image(&bomb).is_err());
        assert!(decode_image(b"not an image").is_err());
        assert!(decode_image(&png(4, 4)[..40]).is_err());
    }

    #[test]
    fn draws_svgs_without_reaching_outside() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="20" height="10">
            <script>alert(1)</script>
            <rect width="20" height="10" fill="#ff0000"/>
            <image href="C:/Windows/win.ini" width="5" height="5"/>
            <image xlink:href="https://example.com/x.png" width="5" height="5"/>
        </svg>"##;
        let p = fuzz_and_render(svg);
        assert_eq!(p.size, (20, 10));
        assert_eq!(p.rgba.dimensions(), (2048, 1024));
        assert_eq!(p.rgba.get_pixel(1000, 500).0, [255, 0, 0, 255]);
        assert!(render_svg_with(b"<svg", Arc::default()).is_err());
        assert!(
            render_svg_with(br#"<svg xmlns="http://www.w3.org/2000/svg" width="0" height="0"/>"#, Arc::default())
                .is_err()
        );
    }

    fn fuzz_and_render(svg: &[u8]) -> Picture {
        fuzz_svg(svg);
        render_svg_with(svg, Arc::default()).unwrap()
    }
}
