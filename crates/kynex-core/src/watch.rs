//! Watching a folder (and everything in it) for changes, so a file explorer
//! stays current while Claude works. A burst of changes rings once, after
//! things have been quiet for [`QUIET`]. No polling: the OS tells us
//! (ReadDirectoryChangesW, inotify, FSEvents).

use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use notify::{RecursiveMode, Watcher};

/// How long changes must stop before `on_change` rings.
pub const QUIET: Duration = Duration::from_millis(120);

/// A folder being watched; dropping it stops the watching.
pub struct Watch {
    _watcher: notify::RecommendedWatcher,
}

/// Watches `dir` and what's under it; `on_change` rings (on a thread of its
/// own) once a burst of changes settles. Errors if `dir` can't be watched,
/// e.g. it doesn't exist yet.
pub fn watch(dir: &Path, on_change: impl Fn() + Send + 'static) -> Result<Watch, String> {
    let (tx, rx) = mpsc::channel::<()>();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if event.is_ok_and(|e| !e.kind.is_access()) {
            let _ = tx.send(());
        }
    })
    .map_err(|e| format!("Couldn't watch {}: {e}", dir.display()))?;
    watcher.watch(dir, RecursiveMode::Recursive).map_err(|e| format!("Couldn't watch {}: {e}", dir.display()))?;
    // Ends when the watcher (and with it the sender) is dropped.
    std::thread::Builder::new()
        .name("kynex-watch".into())
        .spawn(move || {
            while rx.recv().is_ok() {
                loop {
                    match rx.recv_timeout(QUIET) {
                        Ok(()) => continue,
                        Err(mpsc::RecvTimeoutError::Timeout) => break,
                        Err(mpsc::RecvTimeoutError::Disconnected) => return,
                    }
                }
                on_change();
            }
        })
        .map_err(|e| format!("Couldn't start watching: {e}"))?;
    Ok(Watch { _watcher: watcher })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Instant;

    use super::*;

    #[test]
    fn a_burst_of_changes_rings_once() {
        let dir = std::env::temp_dir().join(format!("kynex-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        let rings = Arc::new(AtomicUsize::new(0));
        let r = rings.clone();
        let w = watch(&dir, move || {
            r.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap();
        for i in 0..5 {
            std::fs::write(dir.join("sub").join(format!("f{i}")), "x").unwrap();
        }
        let start = Instant::now();
        while rings.load(Ordering::SeqCst) == 0 && start.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(20));
        }
        std::thread::sleep(QUIET * 3);
        assert_eq!(rings.load(Ordering::SeqCst), 1);
        drop(w);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_folder_says_so() {
        assert!(watch(Path::new("/no/such/kynex/folder"), || {}).is_err());
    }
}
