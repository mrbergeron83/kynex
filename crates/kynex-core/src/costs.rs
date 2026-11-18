//! What each conversation has cost so far, kept across closing and reopening it: the `claude`
//! CLI's `total_cost_usd` starts over with every process, and its log doesn't record it. One
//! small file per session id under `<data dir>/costs/`.

use std::path::{Path, PathBuf};

/// Where the costs are kept.
pub fn dir() -> PathBuf {
    crate::paths::data_dir().join("costs")
}

/// A session id fit for a file name (the CLI's are UUIDs); anything else isn't kept.
fn file(dir: &Path, id: &str) -> Option<PathBuf> {
    let fits = !id.is_empty() && id.len() <= 128 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    fits.then(|| dir.join(id))
}

/// What session `id` had cost when last saved; 0 if nothing was. Blocks on a small file read.
pub fn load_in(dir: &Path, id: &str) -> f64 {
    file(dir, id)
        .and_then(|f| std::fs::read_to_string(f).ok())
        .and_then(|s| s.trim().parse::<f64>().ok())
        .filter(|c| c.is_finite() && *c >= 0.0)
        .unwrap_or(0.0)
}

/// Saves `cost` as what session `id` has cost so far. Blocks on a small file write.
pub fn save_in(dir: &Path, id: &str, cost: f64) {
    let Some(f) = file(dir, id) else { return };
    if !cost.is_finite() || cost <= 0.0 {
        return;
    }
    let _ = std::fs::create_dir_all(dir);
    let _ = std::fs::write(f, cost.to_string());
}

/// [`save_in`] in the data dir, on a thread of its own so the caller doesn't wait on the disk.
pub fn save(id: String, cost: f64) {
    let _ = std::thread::Builder::new().name("save-cost".into()).spawn(move || save_in(&dir(), &id, cost));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cost_comes_back_for_its_session_only() {
        let dir = std::env::temp_dir().join(format!("kynex-costs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(load_in(&dir, "a1b2"), 0.0, "nothing saved yet");
        save_in(&dir, "a1b2", 1.25);
        save_in(&dir, "c3d4", 0.5);
        assert_eq!(load_in(&dir, "a1b2"), 1.25);
        assert_eq!(load_in(&dir, "c3d4"), 0.5);
        save_in(&dir, "a1b2", 2.0);
        assert_eq!(load_in(&dir, "a1b2"), 2.0, "the latest wins");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_id_that_isnt_a_plain_name_is_never_a_path() {
        let dir = std::env::temp_dir().join(format!("kynex-costs-bad-{}", std::process::id()));
        for bad in ["", "../x", "a/b", r"a\b", "a.b", &"x".repeat(200)] {
            save_in(&dir, bad, 1.0);
            assert_eq!(load_in(&dir, bad), 0.0, "{bad:?}");
        }
        assert!(!dir.exists(), "nothing was written");
    }

    #[test]
    fn junk_reads_as_nothing() {
        let dir = std::env::temp_dir().join(format!("kynex-costs-junk-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        for (id, text) in [("j1", "abc"), ("j2", "-3"), ("j3", "NaN"), ("j4", "inf")] {
            std::fs::write(dir.join(id), text).unwrap();
            assert_eq!(load_in(&dir, id), 0.0, "{text}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
