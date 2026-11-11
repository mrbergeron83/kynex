//! Terminal scrollback kept across a restart: when Kynex quits, each shell pane's text is
//! written to `<data dir>/scrollback/<id>.txt` (the last [`MAX_LINES`] lines, at most
//! [`MAX_BYTES`]), and the pane shows it again above a fresh shell when it reopens. The file
//! goes when its pane, or the workspace or project holding it, closes.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::paths::data_dir;

/// The most lines kept per pane.
pub const MAX_LINES: usize = 2000;
/// The most bytes kept per pane.
pub const MAX_BYTES: usize = 256 * 1024;

fn dir() -> PathBuf {
    data_dir().join("scrollback")
}

/// `id`'s file, if `id` is one Kynex made (it comes from a layout file).
fn file(id: &str) -> Option<PathBuf> {
    let ok = !id.is_empty() && id.len() <= 40 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    ok.then(|| dir().join(format!("{id}.txt")))
}

/// A fresh id for a pane's scrollback.
pub fn new_id() -> String {
    static N: AtomicU32 = AtomicU32::new(0);
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or_default();
    format!("{now:x}-{:x}-{:x}", std::process::id(), N.fetch_add(1, Ordering::Relaxed))
}

/// The line [`replay`] puts under what it brings back.
const BANNER: &str = "── restored from last time ──";

/// What of `text` is kept: its last lines within the bounds, no blank lines at the end, and
/// no banner from an earlier restore.
pub fn trim(text: &str) -> String {
    let lines: Vec<&str> = text.trim_end().lines().filter(|l| l.trim() != BANNER).collect();
    let mut kept = lines[lines.len().saturating_sub(MAX_LINES)..].join("\n");
    if kept.len() > MAX_BYTES {
        let mut cut = kept.len() - MAX_BYTES;
        while !kept.is_char_boundary(cut) {
            cut += 1;
        }
        // From the start of the next whole line.
        let next = kept[cut..].find('\n').map_or(kept.len(), |i| cut + i + 1);
        kept = kept[next..].to_string();
    }
    kept
}

/// `text` as bytes a terminal shows: its lines, then a faint line saying where they came
/// from, before the shell's own output.
pub fn replay(text: &str) -> Vec<u8> {
    let mut out = String::new();
    for line in text.lines() {
        // Only what the screen held: no escape can come back to drive the terminal.
        out.extend(line.chars().filter(|c| !c.is_control()));
        out.push_str("\r\n");
    }
    out.push_str(&format!("\x1b[2m{BANNER}\x1b[0m\r\n"));
    out.into_bytes()
}

/// Forgets every pane's text but `keep`'s (the ids some saved layout still names). Blocks
/// on the folder.
pub fn prune(keep: &[String]) {
    let Ok(entries) = std::fs::read_dir(dir()) else { return };
    for e in entries.flatten() {
        let path = e.path();
        let id = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        if path.extension().is_some_and(|x| x == "txt") && !keep.contains(&id) {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Keeps `text` (trimmed) as `id`'s. Blocks on the file.
pub fn save(id: &str, text: &str) -> Result<(), String> {
    let path = file(id).ok_or("Not a scrollback id.")?;
    std::fs::create_dir_all(dir()).map_err(|e| e.to_string())?;
    std::fs::write(path, trim(text)).map_err(|e| e.to_string())
}

/// `id`'s saved text, if any. Blocks on the file.
pub fn load(id: &str) -> Option<String> {
    std::fs::read_to_string(file(id)?).ok().filter(|t| !t.trim().is_empty())
}

/// Forgets `id`'s text. Blocks on the file.
pub fn remove(id: &str) {
    if let Some(path) = file(id) {
        let _ = std::fs::remove_file(path);
    }
}

/// Forgets every pane's text (the setting was turned off). Blocks on the folder.
pub fn clear_all() {
    let _ = std::fs::remove_dir_all(dir());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_last_lines_are_kept_without_trailing_blanks() {
        let text: String = (0..MAX_LINES + 10).map(|i| format!("line {i}\n")).collect::<String>() + "\n\n   \n";
        let kept = trim(&text);
        assert!(kept.starts_with("line 10\n"));
        assert!(kept.ends_with(&format!("line {}", MAX_LINES + 9)));
        assert_eq!(kept.lines().count(), MAX_LINES);
    }

    #[test]
    fn a_long_text_is_cut_to_whole_lines_within_the_bytes() {
        let wide = "é".repeat(400);
        let text: String = (0..1000).map(|_| format!("{wide}\n")).collect();
        let kept = trim(&text);
        assert!(kept.len() <= MAX_BYTES);
        assert!(kept.lines().all(|l| l == wide), "no line was cut in the middle");
    }

    #[test]
    fn a_banner_from_an_earlier_restore_is_not_kept_again() {
        let back = String::from_utf8(replay("ls")).unwrap().replace("\x1b[2m", "").replace("\x1b[0m", "");
        assert_eq!(trim(&format!("{back}$ pwd\n")), "ls\n$ pwd");
    }

    #[test]
    fn replayed_text_carries_no_escapes() {
        let bytes = replay("ok\x1b]0;evil\x07\nnext");
        let s = String::from_utf8(bytes).unwrap();
        assert!(s.starts_with("ok]0;evil\r\nnext\r\n"));
        assert!(s.contains("restored from last time"));
    }

    #[test]
    fn only_kynex_ids_name_files() {
        assert!(file("../../etc/passwd").is_none());
        assert!(file("").is_none());
        assert!(file(&new_id()).is_some());
        assert_ne!(new_id(), new_id());
    }
}
