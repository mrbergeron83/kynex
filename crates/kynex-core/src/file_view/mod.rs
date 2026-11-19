//! What the Git pane's file viewer shows for a changed file: its whole diff when it's
//! text, its pictures before and after when it's an image, its pages when it's a PDF,
//! and only its size when it's any other binary. Within size limits, so a huge file
//! gives a placeholder instead of freezing the app; nothing in a file is ever run.
//!
//! | file        | what                                                   |
//! |-------------|--------------------------------------------------------|
//! | `mod.rs`    | kinds of file, limits, loading a file's sides          |
//! | `caret.rs`  | the caret and selection the viewer moves and copies    |
//! | `media.rs`  | raster images and SVGs, decoded to RGBA                |
//! | `pdf.rs`    | PDF pages, drawn in a child process                    |

pub mod caret;
pub mod media;
pub mod pdf;

use std::path::Path;

use crate::git::{self, Blob, DiffSide, FileDiff, Rev};

pub use media::Picture;

/// The biggest text file shown as text.
pub const MAX_TEXT: u64 = 8 << 20;
/// The biggest image file read.
pub const MAX_IMAGE: u64 = 64 << 20;

/// What kind of file a path holds, for picking how to show it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Text,
    Image,
    Svg,
    Pdf,
    Binary,
}

impl Kind {
    /// By the name, then the first bytes (`head`, a few KB) for names that don't say.
    pub fn of(path: &str, head: &[u8]) -> Kind {
        let ext = Path::new(path).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        match ext.as_str() {
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico" => return Kind::Image,
            "svg" => return Kind::Svg,
            "pdf" => return Kind::Pdf,
            _ => {}
        }
        if head.starts_with(b"%PDF-") {
            Kind::Pdf
        } else if image::guess_format(head).is_ok_and(|f| {
            use image::ImageFormat as F;
            matches!(f, F::Png | F::Jpeg | F::Gif | F::WebP | F::Bmp | F::Ico)
        }) {
            Kind::Image
        } else if head.contains(&0) {
            Kind::Binary
        } else {
            Kind::Text
        }
    }

    /// The most bytes of this kind read to show it.
    pub fn limit(self) -> u64 {
        match self {
            Kind::Text | Kind::Svg => MAX_TEXT,
            Kind::Image => MAX_IMAGE,
            Kind::Pdf => pdf::MAX_BYTES,
            Kind::Binary => 0,
        }
    }
}

/// One side of a file: before or after the change.
#[derive(Debug)]
pub enum Side<T> {
    /// The file isn't on this side (a new file has no before, a deleted one no after).
    Missing,
    TooLarge(u64),
    Failed(String),
    Ready(T),
}

impl<T> Side<T> {
    pub fn map<U>(self, f: impl FnOnce(T) -> Result<U, String>) -> Side<U> {
        match self {
            Side::Ready(t) => match f(t) {
                Ok(u) => Side::Ready(u),
                Err(e) => Side::Failed(e),
            },
            Side::Missing => Side::Missing,
            Side::TooLarge(n) => Side::TooLarge(n),
            Side::Failed(e) => Side::Failed(e),
        }
    }

    pub fn is_missing(&self) -> bool {
        matches!(self, Side::Missing)
    }
}

/// A file read for the viewer.
#[derive(Debug)]
pub enum Loaded {
    Text(FileDiff),
    /// A picture before and after the change; one side is missing for a new or deleted file.
    Pictures {
        before: Side<Picture>,
        after: Side<Picture>,
    },
    /// A PDF's bytes (the after side, or the before of a deleted file), to draw with [`pdf::render`].
    Pdf(Side<Vec<u8>>),
    /// A file that can't be shown, this many bytes long.
    Binary {
        size: u64,
    },
    /// Text over [`MAX_TEXT`].
    TooLarge {
        size: u64,
    },
}

/// Where the before and after of `side` come from: git's HEAD or index, or the working
/// tree (`None`).
fn sources(side: DiffSide) -> (Option<Rev>, Option<Rev>) {
    match side {
        DiffSide::All => (Some(Rev::Head), None),
        DiffSide::Staged => (Some(Rev::Head), Some(Rev::Index)),
        DiffSide::Unstaged => (Some(Rev::Index), None),
    }
}

/// Reads one side: from git at `rev`, or the working tree.
fn read_side(root: &Path, path: &str, rev: Option<Rev>, max: u64) -> Side<Vec<u8>> {
    match rev {
        Some(rev) => match git::blob(root, rev, path, max) {
            Ok(Blob::Bytes(b)) => Side::Ready(b),
            Ok(Blob::Missing) => Side::Missing,
            Ok(Blob::TooLarge(n)) => Side::TooLarge(n),
            Err(e) => Side::Failed(e),
        },
        None => {
            let file = root.join(path);
            match std::fs::metadata(&file) {
                Err(_) => Side::Missing,
                Ok(m) if m.len() > max => Side::TooLarge(m.len()),
                Ok(_) => match std::fs::read(&file) {
                    Ok(b) => Side::Ready(b),
                    Err(e) => Side::Failed(format!("Couldn't read {path}: {e}")),
                },
            }
        }
    }
}

/// The first few KB of the file, from whichever side has it, for [`Kind::of`].
fn head_of(root: &Path, path: &str, side: DiffSide) -> (Vec<u8>, u64) {
    use std::io::Read;
    let file = root.join(path);
    if side != DiffSide::Staged
        && let Ok(f) = std::fs::File::open(&file)
    {
        let size = f.metadata().map_or(0, |m| m.len());
        let mut head = Vec::new();
        let _ = f.take(8192).read_to_end(&mut head);
        return (head, size);
    }
    // Staged, or gone from the working tree: git's copy, if it's small enough to sniff.
    let (before, after) = sources(side);
    for rev in [after, before].into_iter().flatten() {
        match git::blob(root, rev, path, 1 << 20) {
            Ok(Blob::Bytes(b)) => {
                let size = b.len() as u64;
                return (b.into_iter().take(8192).collect(), size);
            }
            Ok(Blob::TooLarge(n)) => return (Vec::new(), n),
            _ => {}
        }
    }
    (Vec::new(), 0)
}

/// A raster image decoded, a panic in the decoder (a malformed file) turned into an error.
fn decode_caught(bytes: &[u8]) -> Result<Picture, String> {
    std::panic::catch_unwind(|| media::decode_image(bytes)).unwrap_or_else(|_| Err("Couldn't read the image".into()))
}

/// An SVG drawn in a child process (this executable, started with [`pdf::SVG_FLAG`]), so a
/// hostile one that hangs or eats memory takes down only that process.
fn svg_in_child(bytes: Vec<u8>) -> Result<Picture, String> {
    let exe = std::env::current_exe().map_err(|e| format!("Couldn't draw the SVG: {e}"))?;
    let rgba = pdf::render_svg(&exe, bytes)?;
    Ok(Picture { size: rgba.dimensions(), rgba })
}

/// Reads `path` (repo-relative) in `root` for the viewer: one `side` of its changes.
/// Raster images are decoded here and SVGs drawn in a child process (the app's `main`
/// must call [`pdf::child_main`]); a PDF's pages are drawn afterwards. Blocking.
pub fn load(root: &Path, path: &str, untracked: bool, side: DiffSide) -> Result<Loaded, String> {
    let (head, size) = head_of(root, path, side);
    let kind = Kind::of(path, &head);
    let (before, after) = sources(side);
    let read = |rev: Option<Rev>| read_side(root, path, rev, kind.limit());
    match kind {
        Kind::Image | Kind::Svg => {
            let decode = |b: Vec<u8>| if kind == Kind::Svg { svg_in_child(b) } else { decode_caught(&b) };
            let before = if untracked { Side::Missing } else { read(before).map(decode) };
            let after = read(after).map(decode);
            Ok(Loaded::Pictures { before, after })
        }
        Kind::Pdf => {
            let after = read(after);
            Ok(Loaded::Pdf(if after.is_missing() && !untracked { read(before) } else { after }))
        }
        Kind::Binary => Ok(Loaded::Binary { size }),
        Kind::Text if size > MAX_TEXT => Ok(Loaded::TooLarge { size }),
        Kind::Text => {
            let d = git::whole_file_diff_side(root, path, untracked, side)?;
            Ok(if d.binary { Loaded::Binary { size } } else { Loaded::Text(d) })
        }
    }
}

/// Reads any file on disk for the viewer, as it is, outside git: text with every line
/// context, images and SVGs as an "after" alone, PDFs, binaries. For a file linked
/// from a session's text. SVGs are drawn in a child process, as in [`load`]. Blocking.
pub fn load_file(file: &Path) -> Result<Loaded, String> {
    use std::io::Read;
    let shown = file.display().to_string();
    let meta = std::fs::metadata(file).map_err(|e| format!("Couldn't open {shown}: {e}"))?;
    if !meta.is_file() {
        return Err(format!("{shown} isn't a file"));
    }
    let size = meta.len();
    let mut head = Vec::new();
    let f = std::fs::File::open(file).map_err(|e| format!("Couldn't open {shown}: {e}"))?;
    let _ = f.take(8192).read_to_end(&mut head);
    let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let kind = Kind::of(&name, &head);
    let read = || -> Side<Vec<u8>> {
        if size > kind.limit() {
            return Side::TooLarge(size);
        }
        match std::fs::read(file) {
            Ok(b) => Side::Ready(b),
            Err(e) => Side::Failed(format!("Couldn't read {shown}: {e}")),
        }
    };
    match kind {
        Kind::Image => Ok(Loaded::Pictures { before: Side::Missing, after: read().map(|b| decode_caught(&b)) }),
        Kind::Svg => Ok(Loaded::Pictures { before: Side::Missing, after: read().map(svg_in_child) }),
        Kind::Pdf => Ok(Loaded::Pdf(read())),
        Kind::Binary => Ok(Loaded::Binary { size }),
        Kind::Text if size > MAX_TEXT => Ok(Loaded::TooLarge { size }),
        Kind::Text => match read() {
            Side::Ready(bytes) => {
                let d = git::plain_file_diff(&name, &bytes);
                Ok(if d.binary { Loaded::Binary { size } } else { Loaded::Text(d) })
            }
            Side::TooLarge(size) => Ok(Loaded::TooLarge { size }),
            Side::Failed(e) => Err(e),
            Side::Missing => Err(format!("{shown} isn't there")),
        },
    }
}

/// Opening this file with its default app could run it: a program, a script, a shortcut.
/// The viewer offers only "Show in folder" for these.
pub fn could_run(path: &str) -> bool {
    let ext = Path::new(path).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    const RUNNABLE: &[&str] = &[
        "exe",
        "com",
        "bat",
        "cmd",
        "ps1",
        "psm1",
        "vbs",
        "vbe",
        "js",
        "jse",
        "wsf",
        "wsh",
        "msi",
        "msp",
        "scr",
        "pif",
        "lnk",
        "url",
        "hta",
        "cpl",
        "jar",
        "reg",
        "inf",
        "sh",
        "bash",
        "zsh",
        "command",
        "app",
        "py",
        "pyw",
        "rb",
        "pl",
        "appimage",
        "run",
        "bin",
        "deb",
        "rpm",
        "pkg",
        "dmg",
        "desktop",
        "applescript",
        "scpt",
        "workflow",
        "action",
        "html",
        "htm",
        "xhtml",
        "svg",
        "mht",
        "mhtml",
        "xml",
        "chm",
        "dll",
        "so",
        "dylib",
        "sys",
        "drv",
        "ocx",
        "application",
        "appref-ms",
        "gadget",
        "msc",
        "settingcontent-ms",
    ];
    ext.is_empty() || RUNNABLE.contains(&ext.as_str())
}

/// `1.2 MB`, `340 KB`, `12 bytes`.
pub fn size_label(bytes: u64) -> String {
    match bytes {
        b if b >= 1 << 30 => format!("{:.1} GB", b as f64 / (1u64 << 30) as f64),
        b if b >= 1 << 20 => format!("{:.1} MB", b as f64 / (1u64 << 20) as f64),
        b if b >= 1 << 10 => format!("{} KB", b.div_ceil(1 << 10)),
        1 => "1 byte".into(),
        b => format!("{b} bytes"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn any_file_on_disk_loads_as_it_is() {
        let dir = std::env::temp_dir().join(format!("kynex-load-file-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let text = dir.join("note.txt");
        std::fs::write(&text, "one\ntwo\n").unwrap();
        let Ok(Loaded::Text(d)) = load_file(&text) else { panic!("text") };
        let lines = &d.hunks[0].lines;
        assert_eq!(lines.len(), 2);
        assert!(lines.iter().all(|l| l.kind == git::LineKind::Context), "no change tints");
        assert_eq!((d.added, d.removed), (0, 0));
        let bin = dir.join("blob.dat");
        std::fs::write(&bin, [0u8, 1, 2]).unwrap();
        assert!(matches!(load_file(&bin), Ok(Loaded::Binary { size: 3 })));
        assert!(load_file(&dir.join("missing.txt")).is_err());
        assert!(load_file(&dir).is_err(), "a folder isn't a file");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn kinds_by_name_then_content() {
        assert_eq!(Kind::of("a/b.PNG", b""), Kind::Image);
        assert_eq!(Kind::of("logo.svg", b"<svg"), Kind::Svg);
        assert_eq!(Kind::of("doc.pdf", b""), Kind::Pdf);
        assert_eq!(Kind::of("noext", b"%PDF-1.7\n"), Kind::Pdf);
        assert_eq!(Kind::of("noext", b"\x89PNG\r\n\x1a\n\0\0"), Kind::Image);
        assert_eq!(Kind::of("app.exe", b"MZ\x90\0"), Kind::Binary);
        assert_eq!(Kind::of("main.rs", b"fn main() {}"), Kind::Text);
        assert_eq!(Kind::of("empty", b""), Kind::Text);
    }

    #[test]
    fn programs_and_pages_only_show_in_their_folder() {
        for p in ["setup.exe", "run.BAT", "x.ps1", "a.lnk", "page.html", "pic.svg", "Makefile", "lib.dll"] {
            assert!(could_run(p), "{p}");
        }
        for p in ["data.zip", "font.ttf", "pic.png", "doc.pdf", "db.sqlite"] {
            assert!(!could_run(p), "{p}");
        }
    }

    #[test]
    fn sizes() {
        assert_eq!(size_label(1), "1 byte");
        assert_eq!(size_label(500), "500 bytes");
        assert_eq!(size_label(1500), "2 KB");
        assert_eq!(size_label(3 << 20), "3.0 MB");
    }

    #[test]
    fn loads_each_kind_from_a_repo() {
        let root = std::env::temp_dir().join(format!("kynex-file-view-{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        std::fs::create_dir_all(&root).unwrap();
        let git = |args: &[&str]| {
            let ok = std::process::Command::new("git").arg("-C").arg(&root).args(args).output().unwrap();
            assert!(ok.status.success(), "{args:?}: {}", String::from_utf8_lossy(&ok.stderr));
        };
        let png = |w: u32| {
            let mut out = Vec::new();
            image::RgbaImage::new(w, 1).write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
            out
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@example.com"]);
        git(&["config", "user.name", "t"]);
        std::fs::write(root.join("pic.png"), png(2)).unwrap();
        std::fs::write(root.join("a.txt"), "one\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-qm", "x"]);
        std::fs::write(root.join("pic.png"), png(5)).unwrap();
        std::fs::write(root.join("a.txt"), "two\n").unwrap();
        std::fs::write(root.join("blob.dat"), b"\0\x01\x02").unwrap();
        std::fs::write(root.join("new.png"), png(3)).unwrap();

        let Loaded::Pictures { before: Side::Ready(b), after: Side::Ready(a) } =
            load(&root, "pic.png", false, DiffSide::All).unwrap()
        else {
            panic!("pictures")
        };
        assert_eq!((b.size, a.size), ((2, 1), (5, 1)));
        let Loaded::Pictures { before: Side::Missing, after: Side::Ready(a) } =
            load(&root, "new.png", true, DiffSide::All).unwrap()
        else {
            panic!("new picture")
        };
        assert_eq!(a.size, (3, 1));
        assert!(matches!(load(&root, "a.txt", false, DiffSide::All).unwrap(), Loaded::Text(d) if d.added == 1));
        assert!(matches!(load(&root, "blob.dat", true, DiffSide::All).unwrap(), Loaded::Binary { size: 3 }));
        std::fs::remove_dir_all(&root).ok();
    }
}
