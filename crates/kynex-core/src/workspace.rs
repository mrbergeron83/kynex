//! Workspaces: the checkouts of one project. A workspace is the project's main tree or one
//! of its linked git worktrees; the sidebar lists them and everything in the window
//! (sessions, terminal, Git view) follows the selected one. A folder that isn't a git
//! repository has exactly one workspace.
//!
//! What Kynex remembers per project lives in `workspaces.json` ([`Workspaces`]): which
//! workspace is selected, the session each one last had open, and their order.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::git::{self, Worktree};
use crate::paths::data_dir;
use crate::project::{self, normalize};
use crate::snapshots::History;
use crate::tiling::SavedLayout;

/// One checkout of a project.
#[derive(Clone, Debug, PartialEq)]
pub struct Workspace {
    pub path: PathBuf,
    /// `None` when the HEAD is detached, or for a folder that isn't a repository.
    pub branch: Option<String>,
    /// The project's own working tree, as opposed to a linked worktree.
    pub main: bool,
    /// What the sidebar calls it: the folder's name.
    pub label: String,
    /// A plain folder added by hand (scripts, notes), not a checkout of the project.
    pub folder: bool,
}

impl Workspace {
    fn main_tree(path: &Path) -> Self {
        Self { path: path.to_path_buf(), branch: None, main: true, label: folder_label(path), folder: false }
    }

    /// Whether this workspace's folder is `path` (slashes and, on Windows, case aside).
    pub fn at(&self, path: &Path) -> bool {
        same_folder(&self.path, path)
    }
}

/// Whether two paths name the same folder (slashes and, on Windows, case aside).
pub fn same_folder(a: &Path, b: &Path) -> bool {
    normalize(a) == normalize(b)
}

fn folder_label(path: &Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

/// The workspaces for `git worktree list` output: worktrees whose folder is gone
/// (`prunable`) drop out, and the project's own tree is always there, first.
pub fn from_worktrees(root: &Path, worktrees: &[Worktree]) -> Vec<Workspace> {
    let mut out: Vec<Workspace> = worktrees
        .iter()
        .filter(|w| !w.prunable)
        .map(|w| {
            let label = if w.path.file_name().is_some() { folder_label(&w.path) } else { w.label().to_string() };
            Workspace { path: w.path.clone(), branch: w.branch.clone(), main: w.main, label, folder: false }
        })
        .collect();
    if !out.iter().any(|c| c.main) {
        out.insert(0, Workspace::main_tree(root));
    }
    out
}

/// The workspaces of the project at `root`. A folder that isn't a repository (or whose
/// worktrees can't be read) has just its main workspace. Blocking: runs several `git`
/// commands per worktree (status and diff stats included).
pub fn list(root: &Path) -> Vec<Workspace> {
    match git::list_worktrees(root) {
        Ok(worktrees) => {
            let mut out = from_worktrees(root, &worktrees);
            out.retain(|c| c.main || c.path.is_dir());
            out
        }
        Err(_) => vec![Workspace::main_tree(root)],
    }
}

/// Where Kynex puts the worktrees it creates for a project:
/// `<data dir>/worktrees/<project>-<hash>`.
pub fn worktrees_dir(project: &Path) -> PathBuf {
    data_dir().join("worktrees").join(project::folder_name(project))
}

/// The folder for a new worktree on `branch`: `<worktrees dir>/<branch slug>`.
/// Worktrees made elsewhere are still listed; this is only where new ones go. Different
/// branches can share a slug (`feat/x`, `feat-x`), so the caller checks the folder is free.
pub fn new_worktree_path(project: &Path, branch: &str) -> PathBuf {
    worktrees_dir(project).join(branch_slug(branch))
}

/// Makes a workspace: a worktree on `branch` (the existing branch of that name, or a new
/// one from `base`) under [`worktrees_dir`]. Returns its folder. Blocking: runs `git`.
pub fn create(project: &Path, branch: &str, base: Option<&str>) -> Result<PathBuf, String> {
    let path = new_worktree_path(project, branch);
    create_at(project, &path, branch, base)?;
    Ok(path)
}

fn create_at(project: &Path, path: &Path, branch: &str, base: Option<&str>) -> Result<(), String> {
    let branch = branch.trim();
    if branch.is_empty() {
        return Err("Name the branch for the new workspace".into());
    }
    if path.exists() {
        return Err(format!("{} already exists; pick another branch name", path.display()));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("Couldn't create {}: {e}", parent.display()))?;
    }
    git::add_worktree_from(project, path, branch, base)
}

/// Removes a workspace's worktree; its branch stays. Git refuses while it has
/// changes, unless `force`. Blocking: runs `git`.
pub fn remove(project: &Path, path: &Path, force: bool) -> Result<(), String> {
    git::remove_worktree(project, path, force)
}

/// A branch name as a folder name: lower case (one folder on case-insensitive disks),
/// letters, digits, `-`, `_` and `.` kept, everything else (`/` included) a single `-`.
/// Never empty, never a name Windows reserves.
pub fn branch_slug(branch: &str) -> String {
    let mut slug = String::new();
    for c in branch.chars() {
        if c.is_alphanumeric() || c == '_' || c == '.' {
            slug.extend(c.to_lowercase());
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    // Capped so a long branch name stays under Windows' path limit.
    let slug: String = slug.chars().take(40).collect();
    let slug = slug.trim_matches(['-', '.']);
    if slug.is_empty() {
        return "branch".into();
    }
    let stem = slug.split('.').next().unwrap_or(slug);
    let reserved = matches!(stem, "con" | "prn" | "aux" | "nul")
        || (stem.len() == 4
            && (stem.starts_with("com") || stem.starts_with("lpt"))
            && stem.ends_with(|c: char| c.is_ascii_digit()));
    if reserved { format!("{slug}-wt") } else { slug.to_string() }
}

/// What Kynex remembers about a project's workspaces (`workspaces.json`).
#[derive(Clone, Default, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Workspaces {
    /// The selected workspace's folder.
    pub selected: Option<PathBuf>,
    /// The session each workspace last had open, keyed by its (normalized) folder.
    pub sessions: BTreeMap<String, String>,
    /// The sidebar order. Workspaces not listed come after, in git's order.
    pub order: Vec<PathBuf>,
    /// Plain folders added as workspaces by hand, beside the project's checkouts.
    pub folders: Vec<PathBuf>,
    /// Each workspace's panes and the sessions in them, keyed like `sessions`. An entry
    /// that can't be read is skipped, so one bad layout doesn't cost the rest of the file.
    #[serde(deserialize_with = "lenient_layouts")]
    pub layouts: BTreeMap<String, SavedLayout>,
    /// Each workspace's older layouts, keyed like `layouts` (see `snapshots`).
    #[serde(deserialize_with = "lenient_history")]
    pub history: BTreeMap<String, History>,
    /// Workspaces removed through Kynex, whose sessions stay in sight (see `removed`).
    pub removed: Vec<crate::removed::Removed>,
}

/// The `history` map, less the entries that don't parse.
fn lenient_history<'de, D: serde::Deserializer<'de>>(d: D) -> Result<BTreeMap<String, History>, D::Error> {
    let raw: BTreeMap<String, serde_json::Value> = BTreeMap::deserialize(d)?;
    Ok(raw.into_iter().filter_map(|(k, v)| Some((k, serde_json::from_value::<History>(v).ok()?))).collect())
}

/// The `layouts` map, less the entries that don't parse or don't hold together.
fn lenient_layouts<'de, D: serde::Deserializer<'de>>(d: D) -> Result<BTreeMap<String, SavedLayout>, D::Error> {
    let raw: BTreeMap<String, serde_json::Value> = BTreeMap::deserialize(d)?;
    Ok(raw
        .into_iter()
        .filter_map(|(k, v)| Some((k, serde_json::from_value::<SavedLayout>(v).ok()?)))
        .filter(|(_, l)| l.is_valid())
        .collect())
}

impl Workspaces {
    const FILE: &str = "workspaces.json";

    /// What the file was called when workspaces were "contexts".
    const LEGACY_FILE: &str = "contexts.json";

    /// Reads the project's file; missing or unreadable gives the defaults. A project that
    /// still has the file from when workspaces were "contexts" has it renamed first.
    /// Blocking.
    pub fn load(project: &Path) -> Self {
        migrate_legacy(&project::dir(project));
        project::load(project, Self::FILE)
    }

    /// Writes it. Blocking.
    pub fn save(&self, project: &Path) -> Result<(), String> {
        project::save(project, Self::FILE, self)
    }

    /// The selected workspace among `list`: the saved one if it still exists, otherwise
    /// the main tree. `list` must not be empty (see [`list`]).
    pub fn selected_in<'a>(&self, list: &'a [Workspace]) -> &'a Workspace {
        self.selected
            .as_deref()
            .and_then(|s| list.iter().find(|c| c.at(s)))
            .or_else(|| list.iter().find(|c| c.main))
            .unwrap_or(&list[0])
    }

    /// Selects the workspace at `path`. Returns whether that changed anything.
    pub fn select(&mut self, path: &Path) -> bool {
        if self.selected.as_deref().is_some_and(|s| normalize(s) == normalize(path)) {
            return false;
        }
        self.selected = Some(path.to_path_buf());
        true
    }

    /// Adds a plain folder as a workspace of `project`. Returns whether it was new: the
    /// project's own folder and one already added aren't.
    pub fn add_folder(&mut self, project: &Path, path: &Path) -> bool {
        if same_folder(project, path) || self.is_folder(path) {
            return false;
        }
        self.folders.push(path.to_path_buf());
        self.forget_removed(path);
        true
    }

    /// Takes an added folder out of the list, keeping the session it had so adding
    /// it again resumes. (Unlike [`Self::forget`], which drops that too.)
    pub fn remove_folder(&mut self, path: &Path) {
        self.folders.retain(|f| !same_folder(f, path));
        self.order.retain(|p| !same_folder(p, path));
        if self.selected.as_deref().is_some_and(|s| same_folder(s, path)) {
            self.selected = None;
        }
    }

    /// Whether `path` is one of the added folders.
    pub fn is_folder(&self, path: &Path) -> bool {
        self.folders.iter().any(|f| same_folder(f, path))
    }

    /// `list` with the added folders after it (one that is already a checkout stays that).
    pub fn with_folders(&self, mut list: Vec<Workspace>) -> Vec<Workspace> {
        for f in &self.folders {
            if !list.iter().any(|c| c.at(f)) {
                list.push(Workspace {
                    path: f.clone(),
                    branch: None,
                    main: false,
                    label: folder_label(f),
                    folder: true,
                });
            }
        }
        list
    }

    /// `list` in the saved order.
    pub fn ordered(&self, mut list: Vec<Workspace>) -> Vec<Workspace> {
        let mut out = Vec::with_capacity(list.len());
        for p in &self.order {
            if let Some(i) = list.iter().position(|c| c.at(p)) {
                out.push(list.remove(i));
            }
        }
        out.append(&mut list);
        out
    }

    /// The session id the workspace at `path` last had open.
    pub fn session(&self, path: &Path) -> Option<&str> {
        self.sessions.get(&normalize(path)).map(String::as_str)
    }

    /// Remembers (or, with `None`, forgets) the workspace's session. Returns whether
    /// that changed anything.
    pub fn remember(&mut self, path: &Path, session: Option<&str>) -> bool {
        let key = normalize(path);
        match session {
            Some(id) if self.sessions.get(&key).map(String::as_str) != Some(id) => {
                self.sessions.insert(key, id.to_string());
                true
            }
            Some(_) => false,
            None => self.sessions.remove(&key).is_some(),
        }
    }

    /// The panes the workspace at `path` last had.
    pub fn layout(&self, path: &Path) -> Option<&SavedLayout> {
        self.layouts.get(&normalize(path))
    }

    /// Remembers (or, with `None`, forgets) the workspace's panes. Returns whether that
    /// changed anything.
    pub fn set_layout(&mut self, path: &Path, layout: Option<SavedLayout>) -> bool {
        let key = normalize(path);
        match layout {
            Some(l) if self.layouts.get(&key) != Some(&l) => {
                self.layouts.insert(key, l);
                true
            }
            Some(_) => false,
            None => self.layouts.remove(&key).is_some(),
        }
    }

    /// The workspace's layout history, started if it has none.
    pub fn history(&mut self, path: &Path) -> &mut History {
        self.history.entry(normalize(path)).or_default()
    }

    /// Drops everything saved about the workspace at `path`, once it was removed on
    /// purpose. (Not done on its own when a worktree isn't listed: a failed `git`
    /// call must not cost the sessions.)
    pub fn forget(&mut self, path: &Path) {
        self.sessions.remove(&normalize(path));
        self.layouts.remove(&normalize(path));
        self.history.remove(&normalize(path));
        self.order.retain(|p| normalize(p) != normalize(path));
        self.folders.retain(|p| normalize(p) != normalize(path));
        if self.selected.as_deref().is_some_and(|s| normalize(s) == normalize(path)) {
            self.selected = None;
        }
    }

    /// Migration: the project's one remembered session (`ProjectState.session_id`
    /// before workspaces) becomes the main workspace's, unless it already has one.
    /// Returns whether anything changed.
    pub fn adopt_legacy_session(&mut self, main: &Path, legacy: Option<&str>) -> bool {
        match legacy {
            Some(id) if self.session(main).is_none() => self.remember(main, Some(id)),
            _ => false,
        }
    }
}

/// Renames a project's `contexts.json` to `workspaces.json` unless that already exists
/// (it has the same shape). Blocking; failures are left for the next start.
fn migrate_legacy(dir: &Path) {
    let (old, new) = (dir.join(Workspaces::LEGACY_FILE), dir.join(Workspaces::FILE));
    if old.is_file() && !new.exists() {
        let _ = std::fs::rename(&old, &new);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wt(path: &str, branch: Option<&str>, main: bool) -> Worktree {
        Worktree {
            path: path.into(),
            branch: branch.map(Into::into),
            head: "abc1234".into(),
            main,
            ..Default::default()
        }
    }

    fn sample() -> Vec<Workspace> {
        from_worktrees(
            Path::new("/work/app"),
            &[
                wt("/work/app", Some("main"), true),
                wt("/data/wt/feat-x", Some("feat/x"), false),
                wt("/data/wt/fix", None, false),
            ],
        )
    }

    #[test]
    fn lists_worktrees_main_first_labelled_by_folder() {
        let list = sample();
        assert_eq!(list.len(), 3);
        assert!(list[0].main);
        assert_eq!(list[0].label, "app");
        assert_eq!(list[1].label, "feat-x");
        assert_eq!(list[1].branch.as_deref(), Some("feat/x"));
        assert_eq!(list[2].branch, None);
    }

    #[test]
    fn a_vanished_worktree_drops_out() {
        let mut gone = wt("/data/wt/gone", Some("gone"), false);
        gone.prunable = true;
        let list = from_worktrees(Path::new("/work/app"), &[wt("/work/app", Some("main"), true), gone]);
        assert_eq!(list.len(), 1);
        assert!(list[0].main);
    }

    #[test]
    fn the_main_tree_is_always_there() {
        let list = from_worktrees(Path::new("/work/app"), &[]);
        assert_eq!(list.len(), 1);
        assert!(list[0].main);
        assert_eq!(list[0].path, Path::new("/work/app"));
    }

    #[test]
    fn a_folder_that_is_not_a_repo_has_one_workspace() {
        let dir = std::env::temp_dir().join(format!("kynex-workspace-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let list = list(&dir);
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(list.len(), 1);
        assert!(list[0].main);
    }

    #[test]
    fn selection_falls_back_to_the_main_tree() {
        let list = sample();
        let mut c = Workspaces::default();
        assert!(c.selected_in(&list).main);
        assert!(c.select(Path::new("/data/wt/feat-x")));
        assert!(!c.select(Path::new("/data/wt/feat-x")));
        assert_eq!(c.selected_in(&list).label, "feat-x");
        // The selected worktree vanishes.
        let without = &list[..1];
        assert!(c.selected_in(without).main);
        // ...and the choice is kept, so it comes back if the worktree does.
        assert_eq!(c.selected_in(&list).label, "feat-x");
    }

    #[test]
    fn order_puts_listed_first_and_keeps_the_rest() {
        let c = Workspaces {
            order: vec!["/data/wt/fix".into(), "/gone".into(), "/data/wt/feat-x".into()],
            ..Default::default()
        };
        let labels: Vec<_> = c.ordered(sample()).into_iter().map(|c| c.label).collect();
        assert_eq!(labels, ["fix", "feat-x", "app"]);
    }

    #[test]
    fn remembers_a_session_per_workspace() {
        let mut c = Workspaces::default();
        let (a, b) = (Path::new("/work/app"), Path::new("/data/wt/feat-x"));
        assert!(c.remember(a, Some("s1")));
        assert!(!c.remember(a, Some("s1")));
        assert!(c.remember(b, Some("s2")));
        assert_eq!((c.session(a), c.session(b)), (Some("s1"), Some("s2")));
        assert!(c.remember(a, None));
        assert!(!c.remember(a, None));
        assert_eq!(c.session(a), None);
    }

    #[test]
    fn forget_drops_everything_about_a_workspace() {
        let b = Path::new("/data/wt/feat-x");
        let mut c = Workspaces::default();
        c.remember(b, Some("s2"));
        c.select(b);
        c.order.push(b.to_path_buf());
        c.forget(b);
        assert_eq!(c, Workspaces::default());
    }

    #[test]
    fn legacy_session_becomes_the_main_workspaces() {
        let main = Path::new("/work/app");
        let mut c = Workspaces::default();
        assert!(c.adopt_legacy_session(main, Some("old")));
        assert_eq!(c.session(main), Some("old"));
        // A session the workspace already has wins, and nothing is invented.
        assert!(!c.adopt_legacy_session(main, Some("other")));
        assert_eq!(c.session(main), Some("old"));
        assert!(!Workspaces::default().adopt_legacy_session(main, None));
    }

    fn git_in(dir: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }

    #[test]
    fn creates_and_removes_a_workspace() {
        let dir = std::env::temp_dir().join(format!("kynex-workspace-wt-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        let repo = dir.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git_in(&repo, &["init", "-q", "-b", "main"]);
        git_in(&repo, &["commit", "-q", "--allow-empty", "-m", "first"]);

        let wt = dir.join("worktrees").join("feat-x");
        create_at(&repo, &wt, "feat/x", Some("main")).unwrap();
        assert!(wt.is_dir());
        let list = list(&repo);
        assert_eq!(list.len(), 2);
        assert_eq!(list[1].branch.as_deref(), Some("feat/x"));
        // The folder is taken, and a name is needed.
        assert!(create_at(&repo, &wt, "feat/y", None).is_err());
        assert!(create_at(&repo, &dir.join("other"), "  ", None).is_err());

        remove(&repo, &wt, false).unwrap();
        let after = super::list(&repo);
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(after.len(), 1);
        assert!(after[0].main);
    }

    #[test]
    fn plain_folders_are_workspaces_too() {
        let project = Path::new("/work/app");
        let mut c = Workspaces::default();
        assert!(c.add_folder(project, Path::new("/work/scripts")));
        // The project itself and a repeat aren't new.
        assert!(!c.add_folder(project, Path::new("/work/app/")));
        assert!(!c.add_folder(project, Path::new("/work/scripts/")));
        assert!(c.is_folder(Path::new("/work/scripts")));

        let list = c.with_folders(sample());
        assert_eq!(list.len(), 4);
        let folder = &list[3];
        assert!(folder.folder && !folder.main && folder.branch.is_none());
        assert_eq!(folder.label, "scripts");
        assert!(sample().iter().all(|c| !c.folder));
        // A folder that's already a checkout stays a checkout.
        let mut c2 = Workspaces::default();
        assert!(c2.add_folder(project, Path::new("/data/wt/feat-x")));
        assert_eq!(c2.with_folders(sample()).len(), 3);

        c.select(Path::new("/work/scripts"));
        c.remember(Path::new("/work/scripts"), Some("s9"));
        // Removing the folder keeps its session, for adding it again...
        c.remove_folder(Path::new("/work/scripts"));
        assert!(!c.is_folder(Path::new("/work/scripts")) && c.selected.is_none());
        assert_eq!(c.session(Path::new("/work/scripts")), Some("s9"));
        // ...and forgetting drops everything.
        c.forget(Path::new("/work/scripts"));
        assert_eq!(c, Workspaces::default());
    }

    #[test]
    fn same_folder_ignores_trailing_slashes() {
        assert!(same_folder(Path::new("/work/app/"), Path::new("/work/app")));
        assert!(!same_folder(Path::new("/work/app"), Path::new("/work/api")));
        assert!(sample()[1].at(Path::new("/data/wt/feat-x/")));
    }

    #[test]
    fn the_file_round_trips_and_tolerates_gaps() {
        let mut c = Workspaces::default();
        c.select(Path::new("/data/wt/feat-x"));
        c.remember(Path::new("/work/app"), Some("s1"));
        c.order.push("/work/app".into());
        let json = serde_json::to_string(&c).unwrap();
        assert_eq!(serde_json::from_str::<Workspaces>(&json).unwrap(), c);
        assert_eq!(serde_json::from_str::<Workspaces>("{}").unwrap(), Workspaces::default());
    }

    fn saved_two_panes() -> SavedLayout {
        let mut layout = crate::layout::Layout::new("1", 1);
        layout.split(1, crate::layout::Axis::Row, 2, crate::layout::Rect::new(0.0, 0.0, 2000.0, 1000.0));
        SavedLayout {
            layout,
            sessions: BTreeMap::from([(1, "a".to_string())]),
            tools: None,
            seeded: false,
            git_spot: None,
            git_tab: Default::default(),
        }
    }

    #[test]
    fn a_layout_is_remembered_per_workspace_and_round_trips() {
        let mut c = Workspaces::default();
        let (a, b) = (Path::new("/work/app"), Path::new("/data/wt/x"));
        let l = saved_two_panes();
        assert!(c.set_layout(a, Some(l.clone())));
        assert!(!c.set_layout(a, Some(l.clone())));
        assert_eq!(c.layout(a), Some(&l));
        assert_eq!(c.layout(b), None);
        let back: Workspaces = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert_eq!(back, c);
        assert!(c.set_layout(a, None));
        assert!(!c.set_layout(a, None));
        c.set_layout(b, Some(l));
        c.forget(b);
        assert_eq!(c, Workspaces::default());
    }

    #[test]
    fn an_unreadable_layout_does_not_cost_the_file() {
        let good = serde_json::to_value(saved_two_panes()).unwrap();
        let broken_shape = serde_json::json!({"layout": 5});
        let mut twice = good.clone();
        twice["layout"]["tabs"][0]["focus"] = 99.into();
        let json = serde_json::json!({
            "selected": "/work/app",
            "sessions": { "k": "s1" },
            "layouts": { "good": good, "shape": broken_shape, "focus": twice, "null": null },
        });
        let c: Workspaces = serde_json::from_value(json).unwrap();
        assert_eq!(c.selected, Some(PathBuf::from("/work/app")));
        assert_eq!(c.sessions.len(), 1);
        assert_eq!(c.layouts.keys().collect::<Vec<_>>(), ["good"]);
    }

    #[test]
    fn slugs_are_safe_folder_names() {
        assert_eq!(branch_slug("feat/Add Login"), "feat-add-login");
        assert_eq!(branch_slug("fix//a__b.c"), "fix-a__b.c");
        assert_eq!(branch_slug("/.hidden-"), "hidden");
        assert_eq!(branch_slug("///"), "branch");
        assert_eq!(branch_slug(""), "branch");
        assert_eq!(branch_slug("CON"), "con-wt");
        assert_eq!(branch_slug("com1.txt"), "com1.txt-wt");
        assert_eq!(branch_slug("console"), "console");
        assert_eq!(branch_slug(&"a".repeat(80)).len(), 40);
    }

    #[test]
    fn new_worktrees_go_under_the_data_dir() {
        let p = new_worktree_path(Path::new("/work/app"), "feat/x");
        assert!(p.ends_with(Path::new("worktrees").join(project::folder_name(Path::new("/work/app"))).join("feat-x")));
    }

    #[test]
    fn contexts_json_becomes_workspaces_json() {
        let dir = std::env::temp_dir().join(format!("kynex-workspace-migrate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("contexts.json"), r#"{"selected":"/a","order":["/a","/b"]}"#).unwrap();
        migrate_legacy(&dir);
        assert!(!dir.join("contexts.json").exists());
        let moved: Workspaces =
            serde_json::from_str(&std::fs::read_to_string(dir.join("workspaces.json")).unwrap()).unwrap();
        assert_eq!(moved.selected, Some(PathBuf::from("/a")));
        assert_eq!(moved.order.len(), 2);
        // One that already has a workspaces.json keeps it, and the old file stays out of the way.
        std::fs::write(dir.join("contexts.json"), r#"{"selected":"/old"}"#).unwrap();
        migrate_legacy(&dir);
        let kept: Workspaces =
            serde_json::from_str(&std::fs::read_to_string(dir.join("workspaces.json")).unwrap()).unwrap();
        assert_eq!(kept.selected, Some(PathBuf::from("/a")));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
