//! Watching a folder (and everything in it) for changes, so a file explorer
//! stays current while Claude works. A burst of changes rings once, after
//! things have been quiet for [`QUIET`]. No polling: the OS tells us
//! (ReadDirectoryChangesW, inotify, FSEvents).

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use notify::{RecursiveMode, Watcher};

/// How long changes must stop before `on_change` rings.
pub const QUIET: Duration = Duration::from_millis(120);

/// How long a repository's git state must stay quiet before [`GitWatch`] rings: a commit
/// or a `git worktree add` writes several files, and refreshing once is enough.
pub const GIT_QUIET: Duration = Duration::from_millis(400);

/// A folder being watched; dropping it stops the watching.
pub struct Watch {
    _watcher: notify::RecommendedWatcher,
}

/// A thread that rings `on_change` once changes signalled on `rx` have been quiet for
/// `quiet`. Ends when every sender is dropped.
fn ring_when_quiet(
    rx: mpsc::Receiver<()>,
    quiet: Duration,
    on_change: impl Fn() + Send + 'static,
) -> Result<(), String> {
    std::thread::Builder::new()
        .name("kynex-watch".into())
        .spawn(move || {
            while rx.recv().is_ok() {
                loop {
                    match rx.recv_timeout(quiet) {
                        Ok(()) => continue,
                        Err(mpsc::RecvTimeoutError::Timeout) => break,
                        Err(mpsc::RecvTimeoutError::Disconnected) => return,
                    }
                }
                on_change();
            }
        })
        .map(drop)
        .map_err(|e| format!("Couldn't start watching: {e}"))
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
    ring_when_quiet(rx, QUIET, on_change)?;
    Ok(Watch { _watcher: watcher })
}

/// What changes in a repository's git folder when its worktrees, branches or checked-out
/// commits change; dropping it stops the watching.
///
/// Only the small, rarely written parts are watched: `HEAD`, `index` and `packed-refs`
/// in the git folder (not recursively), and `worktrees` and `refs` under it (a worktree
/// added or removed, each one's own `HEAD` and `index`, a branch moved by a commit,
/// fetch or switch). Files in the working tree are not: Claude writes those all day.
pub struct GitWatch {
    watcher: notify::RecommendedWatcher,
    git_dir: PathBuf,
    watched: Vec<PathBuf>,
}

/// Whether a change at `path` is one [`GitWatch`] cares about: the first part of its
/// path under `git_dir` is `worktrees` or `refs` (anything below), or `HEAD`, `index`
/// or `packed-refs` (themselves; their `.lock` files aren't).
fn git_relevant(git_dir: &Path, path: &Path) -> bool {
    let Ok(rel) = path.strip_prefix(git_dir) else { return false };
    let mut parts = rel.components();
    match parts.next().and_then(|c| c.as_os_str().to_str()) {
        Some("worktrees" | "refs") => true,
        Some("HEAD" | "index" | "packed-refs") => parts.next().is_none(),
        _ => false,
    }
}

/// Watches the git state of the repository whose own folder is `root`; `on_change`
/// rings (on a thread of its own) once a burst of changes settles. `None` when `root`
/// has no `.git` folder: not a repository, or a linked worktree (its `.git` is a file),
/// where there's nothing to watch and the fallback poll does the job.
pub fn watch_git(root: &Path, on_change: impl Fn() + Send + 'static) -> Option<GitWatch> {
    let git_dir = root.join(".git");
    if !git_dir.is_dir() {
        return None;
    }
    let (tx, rx) = mpsc::channel::<()>();
    let dir = git_dir.clone();
    let watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        let ring = match event {
            // A lost event (the OS's buffer overflowed) or an error: look again.
            Err(_) => true,
            Ok(e) => e.need_rescan() || (!e.kind.is_access() && e.paths.iter().any(|p| git_relevant(&dir, p))),
        };
        if ring {
            let _ = tx.send(());
        }
    })
    .ok()?;
    ring_when_quiet(rx, GIT_QUIET, on_change).ok()?;
    let mut w = GitWatch { watcher, git_dir, watched: Vec::new() };
    w.rewatch();
    Some(w)
}

impl GitWatch {
    /// Watches what exists now and lets go of what's gone: `worktrees` appears with the
    /// first linked worktree, and a watch on a folder that was deleted ends with it.
    /// Cheap; call it after every refresh.
    pub fn rewatch(&mut self) {
        let gone: Vec<PathBuf> = self.watched.iter().filter(|p| !p.exists()).cloned().collect();
        for p in gone {
            let _ = self.watcher.unwatch(&p);
            self.watched.retain(|w| *w != p);
        }
        let wanted = [
            (self.git_dir.clone(), RecursiveMode::NonRecursive),
            (self.git_dir.join("worktrees"), RecursiveMode::Recursive),
            (self.git_dir.join("refs"), RecursiveMode::Recursive),
        ];
        for (path, mode) in wanted {
            if !self.watched.contains(&path) && path.exists() && self.watcher.watch(&path, mode).is_ok() {
                self.watched.push(path);
            }
        }
    }
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
    fn only_the_small_git_files_count() {
        let g = Path::new("/r/.git");
        for p in [
            "/r/.git/HEAD",
            "/r/.git/index",
            "/r/.git/packed-refs",
            "/r/.git/worktrees",
            "/r/.git/worktrees/x/HEAD",
            "/r/.git/refs/heads/main",
        ] {
            assert!(git_relevant(g, Path::new(p)), "{p}");
        }
        for p in [
            "/r/.git/index.lock",
            "/r/.git/HEAD.lock",
            "/r/.git/objects/ab/cdef",
            "/r/.git/FETCH_HEAD",
            "/r/.git/HEAD/x",
            "/elsewhere/HEAD",
        ] {
            assert!(!git_relevant(g, Path::new(p)), "{p}");
        }
    }

    /// Waits until `rings` is at least `n`, or five seconds pass.
    fn wait_for(rings: &AtomicUsize, n: usize) -> usize {
        let start = Instant::now();
        while rings.load(Ordering::SeqCst) < n && start.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(20));
        }
        rings.load(Ordering::SeqCst)
    }

    #[test]
    fn a_repository_rings_for_its_git_state_but_not_its_files() {
        let root = std::env::temp_dir().join(format!("kynex-git-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let rings = Arc::new(AtomicUsize::new(0));
        let r = rings.clone();
        let mut w = watch_git(&root, move || {
            r.fetch_add(1, Ordering::SeqCst);
        })
        .expect("a .git folder is watchable");

        // Files in the working tree and git's object store don't ring.
        std::fs::write(root.join("file.txt"), "x").unwrap();
        std::fs::write(root.join(".git").join("FETCH_HEAD"), "x").unwrap();
        std::thread::sleep(GIT_QUIET * 3);
        assert_eq!(rings.load(Ordering::SeqCst), 0);

        // HEAD does.
        std::fs::write(root.join(".git").join("HEAD"), "ref: refs/heads/main\n").unwrap();
        assert_eq!(wait_for(&rings, 1), 1);

        // The first linked worktree makes `worktrees`: that rings, and after a rewatch
        // so does what happens inside it.
        std::fs::create_dir_all(root.join(".git").join("worktrees").join("x")).unwrap();
        assert_eq!(wait_for(&rings, 2), 2);
        std::thread::sleep(GIT_QUIET * 3);
        w.rewatch();
        std::fs::write(root.join(".git").join("worktrees").join("x").join("HEAD"), "ref: refs/heads/x\n").unwrap();
        assert_eq!(wait_for(&rings, 3), 3);

        // Removing every linked worktree removes the folder; watching carries on.
        std::fs::remove_dir_all(root.join(".git").join("worktrees")).unwrap();
        std::thread::sleep(GIT_QUIET * 3);
        w.rewatch();
        std::fs::write(root.join(".git").join("index"), "x").unwrap();
        assert!(wait_for(&rings, 5) >= 4);
        drop(w);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn no_git_folder_nothing_to_watch() {
        let dir = std::env::temp_dir().join(format!("kynex-git-watch-none-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(watch_git(&dir, || {}).is_none());
        // A linked worktree's `.git` is a file.
        std::fs::write(dir.join(".git"), "gitdir: /elsewhere\n").unwrap();
        assert!(watch_git(&dir, || {}).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_folder_says_so() {
        assert!(watch(Path::new("/no/such/kynex/folder"), || {}).is_err());
    }
}
