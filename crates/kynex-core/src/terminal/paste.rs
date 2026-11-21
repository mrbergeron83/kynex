//! Helpers for pasting images and dragging files into terminal panes:
//! saving clipboard images to a pruned folder under `<data dir>/pasted/`, formatting
//! file paths safely for shells/CLIs, and recognising previewable image paths.

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Maximum saved clipboard images kept in `<data dir>/pasted/` before oldest are pruned.
const MAX_PASTED_FILES: usize = 100;
/// Maximum raw clipboard image bytes accepted when saving to disk (64 MiB).
const MAX_PASTED_BYTES: usize = 64 << 20;

static NEXT_PASTE_SEQ: AtomicU64 = AtomicU64::new(1);

/// Whether `path` has an image extension Kynex can preview.
pub fn is_image_path(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else { return false };
    matches!(ext.to_ascii_lowercase().as_str(), "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" | "ico" | "svg")
}

/// Formats one or more file paths for pasting into a shell or agent CLI prompt.
/// Paths containing whitespace or shell-special characters are double-quoted.
pub fn format_paths_for_paste(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .filter_map(|p| {
            let s = p.to_string_lossy();
            if s.is_empty() { None } else { Some(quote_path(&s)) }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn quote_path(path: &str) -> String {
    let needs_quote = path.chars().any(|c| {
        c.is_whitespace()
            || matches!(
                c,
                '"' | '\''
                    | '('
                    | ')'
                    | '&'
                    | ';'
                    | '|'
                    | '<'
                    | '>'
                    | '$'
                    | '`'
                    | '*'
                    | '?'
                    | '#'
                    | '['
                    | ']'
                    | '{'
                    | '}'
                    | '~'
            )
    });
    if !needs_quote {
        return path.to_string();
    }
    let escaped = path.replace('"', "\\\"");
    format!("\"{escaped}\"")
}

/// Folder where clipboard images pasted into terminals are saved (`<data dir>/pasted`).
pub fn pasted_dir() -> PathBuf {
    crate::paths::data_dir().join("pasted")
}

/// Saves raw clipboard image bytes into `<data dir>/pasted/` and returns the written file path.
/// Blocking: call off the UI thread.
pub fn save_pasted_image(bytes: &[u8], ext: &str) -> Result<PathBuf, String> {
    save_pasted_image_in(&pasted_dir(), bytes, ext)
}

/// Saves clipboard image bytes into `dir`, normalizing unknown formats to PNG and pruning old files.
pub fn save_pasted_image_in(dir: &Path, bytes: &[u8], ext: &str) -> Result<PathBuf, String> {
    if bytes.is_empty() {
        return Err("Clipboard image is empty".into());
    }
    if bytes.len() > MAX_PASTED_BYTES {
        return Err("Clipboard image is too large to paste".into());
    }
    let norm_ext = match ext.trim().trim_start_matches('.').to_ascii_lowercase().as_str() {
        "png" => "png",
        "jpg" | "jpeg" => "jpg",
        "webp" => "webp",
        "gif" => "gif",
        "bmp" => "bmp",
        "svg" => "svg",
        _ => "png",
    };

    // Keep recognised encoded formats as-is; re-encode unexpected/raw PNG bytes if needed.
    let (final_ext, final_bytes): (&str, std::borrow::Cow<'_, [u8]>) = if norm_ext == "svg" {
        ("svg", std::borrow::Cow::Borrowed(bytes))
    } else if image::guess_format(bytes).is_ok() {
        (norm_ext, std::borrow::Cow::Borrowed(bytes))
    } else {
        let decoded = image::load_from_memory(bytes).map_err(|e| format!("Couldn't decode clipboard image: {e}"))?;
        let mut out = Vec::new();
        decoded
            .write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
            .map_err(|e| format!("Couldn't encode clipboard image as PNG: {e}"))?;
        ("png", std::borrow::Cow::Owned(out))
    };

    std::fs::create_dir_all(dir).map_err(|e| format!("Couldn't create {}: {e}", dir.display()))?;
    let ms = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis());
    let seq = NEXT_PASTE_SEQ.fetch_add(1, Ordering::Relaxed);
    let path = dir.join(format!("paste-{ms}-{seq}.{final_ext}"));
    std::fs::write(&path, &final_bytes).map_err(|e| format!("Couldn't save pasted image: {e}"))?;
    prune_old_pasted(dir, MAX_PASTED_FILES);
    Ok(path)
}

fn prune_old_pasted(dir: &Path, keep: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<(SystemTime, PathBuf)> = entries
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            if !p.is_file() {
                return None;
            }
            let name = p.file_name()?.to_str()?;
            if !name.starts_with("paste-") {
                return None;
            }
            let mtime = e.metadata().ok()?.modified().unwrap_or(UNIX_EPOCH);
            Some((mtime, p))
        })
        .collect();
    if files.len() <= keep {
        return;
    }
    files.sort_by_key(|(t, _)| *t);
    let drop_count = files.len() - keep;
    for (_, old) in files.into_iter().take(drop_count) {
        let _ = std::fs::remove_file(old);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny_png() -> Vec<u8> {
        let mut out = Vec::new();
        image::RgbaImage::new(2, 2).write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
        out
    }

    #[test]
    fn recognizes_image_extensions_case_insensitively() {
        assert!(is_image_path(Path::new("shot.PNG")));
        assert!(is_image_path(Path::new("/tmp/a.webp")));
        assert!(is_image_path(Path::new("diagram.svg")));
        assert!(is_image_path(Path::new("pic.bmp")));
        assert!(!is_image_path(Path::new("main.rs")));
        assert!(!is_image_path(Path::new("README")));
    }

    #[test]
    fn formats_paths_with_quoting_only_when_needed() {
        assert_eq!(format_paths_for_paste(&[PathBuf::from(r"C:\work\shot.png")]), r"C:\work\shot.png");
        assert_eq!(
            format_paths_for_paste(&[PathBuf::from("/Users/me/My Shots/a (1).png")]),
            "\"/Users/me/My Shots/a (1).png\""
        );
        assert_eq!(format_paths_for_paste(&[PathBuf::from("a.png"), PathBuf::from("my b.png")]), "a.png \"my b.png\"");
    }

    #[test]
    fn saves_unique_pasted_images_and_prunes_oldest() {
        let dir = std::env::temp_dir().join(format!("kynex-paste-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let png = tiny_png();
        let p1 = save_pasted_image_in(&dir, &png, "png").unwrap();
        let p2 = save_pasted_image_in(&dir, &png, "PNG").unwrap();
        assert_ne!(p1, p2);
        assert_eq!(std::fs::read(&p1).unwrap(), png);
        assert!(save_pasted_image_in(&dir, b"", "png").is_err());

        for i in 0..5 {
            std::fs::write(dir.join(format!("paste-0-{i}.png")), &png).unwrap();
        }
        prune_old_pasted(&dir, 3);
        let remaining = std::fs::read_dir(&dir).unwrap().flatten().count();
        assert_eq!(remaining, 3);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
