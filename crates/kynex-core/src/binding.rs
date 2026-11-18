//! A workspace stays bound to its branch: the one it was made on (or first seen on). A
//! session that switches the branch of the folder it runs in leaves the workspace drifted;
//! the app warns about it rather than refusing, since the switch is sometimes intended.

use std::path::Path;

use crate::project::normalize;
use crate::workspace::{Workspace, Workspaces};

/// A workspace that isn't on the branch it's bound to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Drift {
    pub bound: String,
    /// The branch it is on now, or "detached".
    pub live: String,
}

/// How a workspace bound to `bound` and now on `live` has drifted; `None` when it hasn't,
/// isn't bound, or is detached (a rebase or bisect is no switch of branch).
pub fn drift(bound: Option<&str>, live: Option<&str>) -> Option<Drift> {
    let (bound, live) = (bound?, live?);
    (live != bound).then(|| Drift { bound: bound.to_string(), live: live.to_string() })
}

impl Workspaces {
    /// Binds the workspace at `folder` to `branch`.
    pub fn bind(&mut self, folder: &Path, branch: &str) {
        self.branches.insert(normalize(folder), branch.to_string());
    }

    /// The branch the workspace at `folder` is bound to.
    pub fn bound(&self, folder: &Path) -> Option<&str> {
        self.branches.get(&normalize(folder)).map(String::as_str)
    }

    /// Lets go of the binding (the workspace is gone).
    pub fn unbind(&mut self, folder: &Path) {
        self.branches.remove(&normalize(folder));
    }

    /// Drops the bindings of folders that aren't in `list` (removed outside Kynex).
    /// Returns whether anything changed.
    pub fn prune_bindings(&mut self, list: &[Workspace]) -> bool {
        let before = self.branches.len();
        self.branches.retain(|k, _| list.iter().any(|w| normalize(&w.path) == *k));
        self.branches.len() != before
    }

    /// First sight: binds each checkout that has a branch and no binding yet to that
    /// branch. Returns whether anything changed.
    pub fn bind_unbound(&mut self, list: &[Workspace]) -> bool {
        let mut changed = false;
        for w in list.iter().filter(|w| !w.folder) {
            let Some(branch) = &w.branch else { continue };
            if self.bound(&w.path).is_none() {
                self.bind(&w.path, branch);
                changed = true;
            }
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn ws(path: &str, branch: Option<&str>, folder: bool) -> Workspace {
        Workspace {
            path: PathBuf::from(path),
            branch: branch.map(str::to_string),
            main: false,
            label: path.to_string(),
            folder,
        }
    }

    #[test]
    fn drift_needs_a_binding_and_a_different_head() {
        assert_eq!(drift(None, Some("main")), None);
        assert_eq!(drift(Some("main"), Some("main")), None);
        assert_eq!(drift(Some("a"), Some("b")), Some(Drift { bound: "a".into(), live: "b".into() }));
        assert_eq!(drift(Some("a"), None), None);
    }

    #[test]
    fn bindings_of_gone_folders_are_pruned() {
        let mut w = Workspaces::default();
        w.bind(Path::new("/a"), "one");
        w.bind(Path::new("/gone"), "two");
        assert!(w.prune_bindings(&[ws("/a", Some("one"), false)]));
        assert_eq!(w.bound(Path::new("/gone")), None);
        assert_eq!(w.bound(Path::new("/a")), Some("one"));
        assert!(!w.prune_bindings(&[ws("/a", Some("one"), false)]));
    }

    #[test]
    fn bind_and_unbind() {
        let mut w = Workspaces::default();
        w.bind(Path::new("/p/x"), "feat");
        assert_eq!(w.bound(Path::new("/p/x")), Some("feat"));
        w.unbind(Path::new("/p/x"));
        assert_eq!(w.bound(Path::new("/p/x")), None);
    }

    #[test]
    fn first_sight_binds_once_and_skips_folders_and_detached() {
        let mut w = Workspaces::default();
        let list = [ws("/a", Some("one"), false), ws("/b", None, false), ws("/c", Some("x"), true)];
        assert!(w.bind_unbound(&list));
        assert_eq!(w.bound(Path::new("/a")), Some("one"));
        assert_eq!(w.bound(Path::new("/b")), None);
        assert_eq!(w.bound(Path::new("/c")), None);
        // A later switch isn't a new first sight.
        assert!(!w.bind_unbound(&[ws("/a", Some("two"), false)]));
        assert_eq!(w.bound(Path::new("/a")), Some("one"));
    }
}
