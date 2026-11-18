//! PDF pages (by `hayro`) and SVGs (by `resvg`) as pictures, drawn in a child process:
//! Kynex runs itself with [`CHILD_FLAG`] or [`SVG_FLAG`], hands it the file on stdin and
//! reads the pictures back from stdout. A file that makes the renderer panic, overflow
//! its stack or run out of memory takes only that process down, and one that takes too
//! long is killed; either way the viewer shows an error.
//!
//! The child writes `KYNEXPDF <pages>\n`, then for each page `P <width> <height>\n`
//! and that many RGBA pixels, or `E <message>\n` when it can't go on.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use image::RgbaImage;

/// The argument that makes Kynex a PDF renderer instead of opening its window.
pub const CHILD_FLAG: &str = "--render-pdf";
/// The same for an SVG: one picture, drawn by [`super::media::render_svg`].
pub const SVG_FLAG: &str = "--render-svg";
/// The most pixels kept over all of a document's pages (about 190 MB); pages past it
/// aren't drawn.
const MAX_TOTAL_PIXELS: u64 = 48_000_000;
/// The biggest PDF read.
pub const MAX_BYTES: u64 = 64 << 20;
/// The most pages drawn.
pub const MAX_PAGES: usize = 20;
/// The most pixels in one drawn page.
const MAX_PAGE_PIXELS: u64 = 16_000_000;
/// The longest side of a drawn page.
const MAX_SIDE: u32 = 8192;
/// How long one page may take before the child is killed.
const PAGE_TIMEOUT: Duration = Duration::from_secs(30);

/// One drawn page.
pub struct Page {
    /// Its position in the document, from 0.
    pub index: usize,
    pub rgba: RgbaImage,
}

/// What the child sends, as read back.
#[derive(Debug, PartialEq)]
pub(crate) enum Frame {
    Count(usize),
    Page { width: u32, height: u32, rgba: Vec<u8> },
    Error(String),
}

/// Reads the child's frames from `r`, giving each to `on`. Stops at the end of the
/// stream, at a malformed frame (an error) or when `on` returns false.
pub(crate) fn read_frames(r: &mut impl BufRead, mut on: impl FnMut(Frame) -> bool) -> Result<(), String> {
    let mut line = Vec::new();
    loop {
        line.clear();
        // A header line is short; anything longer isn't one.
        let n = r.by_ref().take(512).read_until(b'\n', &mut line).map_err(|e| e.to_string())?;
        if n == 0 {
            return Ok(());
        }
        let text = String::from_utf8_lossy(&line);
        let text = text.trim_end_matches('\n');
        let (tag, rest) = text.split_once(' ').unwrap_or((text, ""));
        let frame = match tag {
            "KYNEXPDF" => Frame::Count(rest.trim().parse().map_err(|_| bad())?),
            "E" => Frame::Error(rest.to_string()),
            "P" => {
                let mut it = rest.split(' ').map(str::parse::<u32>);
                let (Some(Ok(width)), Some(Ok(height)), None) = (it.next(), it.next(), it.next()) else {
                    return Err(bad());
                };
                if width == 0
                    || height == 0
                    || width.max(height) > MAX_SIDE
                    || u64::from(width) * u64::from(height) > MAX_PAGE_PIXELS
                {
                    return Err(bad());
                }
                let mut rgba = vec![0; width as usize * height as usize * 4];
                r.read_exact(&mut rgba).map_err(|_| bad())?;
                Frame::Page { width, height, rgba }
            }
            _ => return Err(bad()),
        };
        if !on(frame) {
            return Ok(());
        }
    }
}

fn bad() -> String {
    "Couldn't read this file".into()
}

/// Draws the pages of `bytes` `width` pixels wide (the first [`MAX_PAGES`], within a
/// memory budget), in a child process running `exe`, handing each to `on_page` as it
/// comes. Returns how many pages the document has. `cancel` stops it. Blocking.
pub fn render(
    exe: &Path,
    bytes: Vec<u8>,
    width: u32,
    cancel: &Arc<AtomicBool>,
    on_page: impl FnMut(Page),
) -> Result<usize, String> {
    match run_child(exe, CHILD_FLAG, bytes, width, cancel, on_page)? {
        0 => Err("This PDF has no pages".into()),
        n => Ok(n),
    }
}

/// Draws an SVG in a child process running `exe` (see [`super::media::render_svg`]).
/// Blocking.
pub fn render_svg(exe: &Path, bytes: Vec<u8>) -> Result<RgbaImage, String> {
    let mut picture = None;
    run_child(exe, SVG_FLAG, bytes, 0, &Arc::default(), |page| picture = Some(page.rgba))?;
    picture.ok_or_else(bad)
}

fn run_child(
    exe: &Path,
    flag: &str,
    bytes: Vec<u8>,
    width: u32,
    cancel: &Arc<AtomicBool>,
    mut on_page: impl FnMut(Page),
) -> Result<usize, String> {
    let mut cmd = Command::new(exe);
    cmd.args([flag, &width.to_string()]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    crate::process::dies_with_parent(&mut cmd);
    let mut child = cmd.spawn().map_err(|e| format!("Couldn't start the PDF renderer: {e}"))?;
    let (Some(mut stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
        let _ = child.kill();
        return Err("Couldn't start the PDF renderer".into());
    };
    std::thread::spawn(move || {
        let _ = stdin.write_all(&bytes);
    });
    let child = Arc::new(Mutex::new(child));
    let start = Instant::now();
    // Milliseconds since `start` at the last page; the watchdog kills a child that stalls.
    let progress = Arc::new(AtomicU64::new(0));
    let done = Arc::new(AtomicBool::new(false));
    let timed_out = Arc::new(AtomicBool::new(false));
    let watchdog = {
        let (child, progress, done, timed_out) = (child.clone(), progress.clone(), done.clone(), timed_out.clone());
        let cancel = cancel.clone();
        std::thread::spawn(move || {
            while !done.load(Ordering::Relaxed) {
                let since = (start.elapsed().as_millis() as u64).saturating_sub(progress.load(Ordering::Relaxed));
                let stop = cancel.load(Ordering::Relaxed);
                if stop || since > PAGE_TIMEOUT.as_millis() as u64 {
                    timed_out.store(!stop, Ordering::Relaxed);
                    if let Ok(mut c) = child.lock() {
                        let _ = c.kill();
                    }
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        })
    };
    let mut total = None;
    let mut drawn = 0;
    let mut pixels = 0u64;
    let mut full = false;
    let mut error = None;
    let mut reader = BufReader::new(stdout);
    let read = read_frames(&mut reader, |frame| {
        progress.store(start.elapsed().as_millis() as u64, Ordering::Relaxed);
        if cancel.load(Ordering::Relaxed) {
            return false;
        }
        match frame {
            Frame::Count(n) => total = Some(n),
            Frame::Page { width, height, rgba } => {
                pixels += u64::from(width) * u64::from(height);
                if pixels > MAX_TOTAL_PIXELS && drawn > 0 {
                    full = true;
                    return false;
                }
                if let Some(rgba) = RgbaImage::from_raw(width, height, rgba) {
                    on_page(Page { index: drawn, rgba });
                }
                drawn += 1;
            }
            Frame::Error(e) => {
                error = Some(e);
                return false;
            }
        }
        true
    });
    done.store(true, Ordering::Relaxed);
    if let Ok(mut c) = child.lock() {
        let _ = c.kill();
        let _ = c.wait();
    }
    let _ = watchdog.join();
    if cancel.load(Ordering::Relaxed) {
        return Err(crate::git::CANCELLED.into());
    }
    if let Some(e) = error {
        return Err(e);
    }
    if timed_out.load(Ordering::Relaxed) {
        return Err("This file took too long to draw".into());
    }
    read?;
    match total {
        // As many pages as fit the budget: the viewer says "first N of M".
        Some(n) if full || drawn >= n.min(MAX_PAGES) => Ok(n),
        // The child died partway: a page it couldn't draw.
        _ => Err(bad()),
    }
}

/// Kynex's entry for [`CHILD_FLAG`]: `Some(exit code)` when started as the renderer,
/// `None` otherwise. Call it first thing in `main`, before any window opens.
pub fn child_main() -> Option<i32> {
    let mut args = std::env::args().skip(1);
    let svg = match args.next().as_deref() {
        Some(CHILD_FLAG) => false,
        Some(SVG_FLAG) => true,
        _ => return None,
    };
    let width = args.next().and_then(|w| w.parse().ok()).unwrap_or(1200u32).clamp(64, 4096);
    let mut bytes = Vec::new();
    let mut out = std::io::BufWriter::new(std::io::stdout().lock());
    if std::io::stdin().lock().take(MAX_BYTES + 1).read_to_end(&mut bytes).is_err() || bytes.len() as u64 > MAX_BYTES {
        let _ = writeln!(out, "E This file is too big to show");
        return Some(1);
    }
    let drawn = if svg { draw_svg(&bytes, &mut out) } else { draw(&bytes, width, &mut out) };
    let code = match drawn {
        Ok(()) => 0,
        Err(e) => {
            let _ = writeln!(out, "E {}", e.replace('\n', " "));
            1
        }
    };
    let _ = out.flush();
    Some(code)
}

/// Draws an SVG into `out` in the child's format: one page.
pub(crate) fn draw_svg(bytes: &[u8], out: &mut impl Write) -> Result<(), String> {
    let p = super::media::render_svg(bytes)?;
    let (w, h) = p.rgba.dimensions();
    writeln!(out, "KYNEXPDF 1\nP {w} {h}").map_err(|e| e.to_string())?;
    out.write_all(p.rgba.as_raw()).map_err(|e| e.to_string())?;
    out.flush().map_err(|e| e.to_string())
}

/// Draws the pages of `bytes` into `out` in the child's format.
pub(crate) fn draw(bytes: &[u8], width: u32, out: &mut impl Write) -> Result<(), String> {
    use hayro::hayro_interpret::InterpreterSettings;
    use hayro::hayro_syntax::Pdf;
    use hayro::vello_cpu::color::palette::css::WHITE;
    use hayro::{PixmapSettings, RenderCache, RenderSettings};

    let pdf = Pdf::new(bytes.to_vec()).map_err(|e| match format!("{e:?}") {
        e if e.contains("Decrypt") || e.contains("Encrypt") => "This PDF is password-protected".to_string(),
        _ => "Couldn't read this PDF".to_string(),
    })?;
    let pages = pdf.pages();
    writeln!(out, "KYNEXPDF {}", pages.len()).map_err(|e| e.to_string())?;
    let cache = RenderCache::new();
    let interp = InterpreterSettings::default();
    let settings = RenderSettings::default();
    for page in pages.iter().take(MAX_PAGES) {
        let (pw, ph) = page.render_dimensions();
        if !(pw.is_finite() && ph.is_finite()) || pw < 1. || ph < 1. {
            return Err("This PDF has a page with no size".into());
        }
        let mut scale = width as f32 / pw;
        // Keep a tall page within the limits too.
        let pixels = (pw * scale) as f64 * (ph * scale) as f64;
        if pixels > MAX_PAGE_PIXELS as f64 {
            scale *= (MAX_PAGE_PIXELS as f64 / pixels).sqrt() as f32;
        }
        if ph * scale > MAX_SIDE as f32 {
            scale = MAX_SIDE as f32 / ph;
        }
        let pixmap = hayro::render(
            page,
            &cache,
            &interp,
            &settings,
            &PixmapSettings { x_scale: scale, y_scale: scale, bg_color: WHITE },
        );
        let (w, h) = (u32::from(pixmap.width()), u32::from(pixmap.height()));
        if w == 0 || h == 0 {
            return Err("This PDF has a page with no size".into());
        }
        // On white, premultiplied pixels are the plain ones.
        writeln!(out, "P {w} {h}").map_err(|e| e.to_string())?;
        out.write_all(pixmap.data_as_u8_slice()).map_err(|e| e.to_string())?;
        out.flush().map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A one-page PDF, 200 × 100 points, with a black square.
    fn tiny_pdf() -> Vec<u8> {
        let content = "0 0 0 rg 10 10 50 50 re f";
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Contents 4 0 R >>".to_string(),
            format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
        ];
        let mut pdf = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objects.iter().enumerate() {
            offsets.push(pdf.len());
            pdf.extend(format!("{} 0 obj\n{o}\nendobj\n", i + 1).bytes());
        }
        let xref = pdf.len();
        pdf.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).bytes());
        for o in offsets {
            pdf.extend(format!("{o:010} 00000 n \n").bytes());
        }
        pdf.extend(
            format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).bytes(),
        );
        pdf
    }

    #[test]
    fn draws_pages_and_reads_them_back() {
        let mut out = Vec::new();
        draw(&tiny_pdf(), 400, &mut out).unwrap();
        let mut frames = Vec::new();
        read_frames(&mut out.as_slice(), |f| {
            frames.push(f);
            true
        })
        .unwrap();
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0], Frame::Count(1));
        let Frame::Page { width, height, rgba } = &frames[1] else { panic!("{:?}", frames[1]) };
        assert_eq!((*width, *height), (400, 200));
        // White page, black square near the bottom left (PDF's y goes up).
        assert_eq!(&rgba[..4], &[255, 255, 255, 255]);
        let at = |x: usize, y: usize| &rgba[(y * 400 + x) * 4..][..4];
        assert_eq!(at(60, 140), &[0, 0, 0, 255]);
    }

    #[test]
    fn svgs_come_back_as_one_page() {
        let mut out = Vec::new();
        draw_svg(br#"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="2"/>"#, &mut out).unwrap();
        let mut frames = Vec::new();
        read_frames(&mut out.as_slice(), |f| {
            frames.push(f);
            true
        })
        .unwrap();
        assert!(matches!(frames[..], [Frame::Count(1), Frame::Page { width: 2048, height: 1024, .. }]));
        assert!(draw_svg(b"<nope", &mut Vec::new()).is_err());
    }

    #[test]
    fn garbage_is_an_error_not_a_panic() {
        let mut out = Vec::new();
        assert!(draw(b"%PDF-1.4 nothing here", 400, &mut out).is_err());
        assert!(draw(b"", 400, &mut out).is_err());
    }

    #[test]
    fn frames_are_checked() {
        let read = |b: &[u8]| {
            let mut v = Vec::new();
            read_frames(&mut &b[..], |f| {
                v.push(f);
                true
            })
            .map(|()| v)
        };
        assert_eq!(read(b"KYNEXPDF 3\nE locked\n").unwrap(), [Frame::Count(3), Frame::Error("locked".into())]);
        assert!(read(b"P 2 1\nabc").is_err(), "short pixels");
        assert!(read(b"P 99999 99999\n").is_err(), "too big");
        assert!(read(b"P 0 5\n").is_err());
        assert!(read(b"hello\n").is_err());
        assert_eq!(read(b"").unwrap(), []);
    }
}
