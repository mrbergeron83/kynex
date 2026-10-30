//! Files for the scratch explorer: a folder's entries and a file's preview.
//! Everything here touches the disk and blocks; the app calls it off the UI
//! thread.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Bytes of a file read for its preview at most.
pub const PREVIEW_CAP: u64 = 256 * 1024;

/// One entry of a folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    /// Bytes for a file; 0 for a folder.
    pub size: u64,
    pub modified: Option<SystemTime>,
}

/// `dir`'s entries: folders first, then files, each by name (any case). An
/// unreadable folder lists as empty.
pub fn list(dir: &Path) -> Vec<Entry> {
    let mut entries: Vec<Entry> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| {
            let meta = e.metadata().ok();
            let is_dir = meta.as_ref().is_some_and(|m| m.is_dir());
            Entry {
                name: e.file_name().to_string_lossy().into_owned(),
                path: e.path(),
                is_dir,
                size: meta.as_ref().filter(|_| !is_dir).map_or(0, |m| m.len()),
                modified: meta.and_then(|m| m.modified().ok()),
            }
        })
        .collect();
    entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    entries
}

/// What a file's preview shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Preview {
    /// Text, cut at [`PREVIEW_CAP`] when `cut`.
    Text { text: String, cut: bool },
    /// A picture the app can draw.
    Image,
    /// Neither: open it in another app.
    Binary,
    /// It couldn't be read (gone, no access).
    Missing(String),
}

/// Extensions drawn as pictures.
const IMAGES: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "ico", "tif", "tiff"];

/// The preview of the file at `path`.
pub fn preview(path: &Path) -> Preview {
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    if IMAGES.contains(&ext.as_str()) {
        return if path.is_file() { Preview::Image } else { Preview::Missing("It isn't there any more.".into()) };
    }
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) => return Preview::Missing(format!("Couldn't read it: {e}")),
    };
    let len = file.metadata().map_or(0, |m| m.len());
    let mut bytes = Vec::new();
    if let Err(e) = file.take(PREVIEW_CAP).read_to_end(&mut bytes) {
        return Preview::Missing(format!("Couldn't read it: {e}"));
    }
    text_of(&bytes, len > PREVIEW_CAP)
}

/// `bytes` as text if they are some: no NUL bytes, UTF-8 apart from a
/// character the cap cut in two.
fn text_of(bytes: &[u8], cut: bool) -> Preview {
    if bytes.contains(&0) {
        return Preview::Binary;
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => Preview::Text { text: s.to_string(), cut },
        // Cut mid-character at the cap: keep what's whole.
        Err(e) if cut && e.error_len().is_none() => {
            Preview::Text { text: String::from_utf8_lossy(&bytes[..e.valid_up_to()]).into_owned(), cut }
        }
        Err(_) => Preview::Binary,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_folders_first_then_by_name() {
        let dir = std::env::temp_dir().join(format!("kynex-files-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("zeta")).unwrap();
        std::fs::create_dir_all(dir.join("Alpha")).unwrap();
        std::fs::write(dir.join("b.txt"), "12345").unwrap();
        std::fs::write(dir.join("A.md"), "").unwrap();
        let got = list(&dir);
        let names: Vec<&str> = got.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["Alpha", "zeta", "A.md", "b.txt"]);
        assert!(got[0].is_dir && got[0].size == 0);
        assert_eq!(got[3].size, 5);
        assert!(list(&dir.join("nope")).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn previews_text_images_and_binaries() {
        let dir = std::env::temp_dir().join(format!("kynex-preview-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("note.md"), "# hi\n").unwrap();
        std::fs::write(dir.join("shot.PNG"), [137, 80, 78, 71]).unwrap();
        std::fs::write(dir.join("blob.bin"), [1, 0, 2]).unwrap();
        std::fs::write(dir.join("big.txt"), "x".repeat(PREVIEW_CAP as usize + 10)).unwrap();
        assert_eq!(preview(&dir.join("note.md")), Preview::Text { text: "# hi\n".into(), cut: false });
        assert_eq!(preview(&dir.join("shot.PNG")), Preview::Image);
        assert_eq!(preview(&dir.join("blob.bin")), Preview::Binary);
        assert!(
            matches!(preview(&dir.join("big.txt")), Preview::Text { cut: true, text } if text.len() == PREVIEW_CAP as usize)
        );
        assert!(matches!(preview(&dir.join("gone.txt")), Preview::Missing(_)));
        assert!(matches!(preview(&dir.join("gone.png")), Preview::Missing(_)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_character_cut_at_the_cap_is_dropped() {
        let bytes = "aé".as_bytes();
        assert_eq!(text_of(&bytes[..2], true), Preview::Text { text: "a".into(), cut: true });
        assert_eq!(text_of(&bytes[..2], false), Preview::Binary);
        assert_eq!(text_of(&[0xff, b'a'], true), Preview::Binary);
    }
}
