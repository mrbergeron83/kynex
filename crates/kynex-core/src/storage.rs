//! Disk use of Claude Code sessions, and freeing it. A session keeps:
//!
//! - its log: `~/.claude/projects/<slug>/<id>.jsonl`, plus a `<id>/` folder of
//!   subagent logs and tool output beside it;
//! - its scratchpad: `<temp>/claude/<slug>/<id>/`;
//! - its file checkpoints, which rewind restores from: `~/.claude/file-history/<id>/`.
//!
//! `<slug>` is the session's folder with every character but letters and
//! digits turned into `-`. Everything here walks folders and blocks; the app
//! calls it off the UI thread. Deleting goes through [`delete`] only, which
//! touches nothing but a session id's own paths under those three roots.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Where Claude Code keeps sessions on this machine.
#[derive(Clone, Debug)]
pub struct Roots {
    pub projects: PathBuf,
    pub scratch: PathBuf,
    pub history: PathBuf,
}

impl Roots {
    /// This user's: `~/.claude/projects`, `<temp>/claude`, `~/.claude/file-history`.
    pub fn local() -> Option<Self> {
        let home = PathBuf::from(std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?);
        let claude = home.join(".claude");
        Some(Self {
            projects: claude.join("projects"),
            scratch: std::env::temp_dir().join("claude"),
            history: claude.join("file-history"),
        })
    }
}

/// What a session (or a set of them) takes, by kind, in bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Sizes {
    pub log: u64,
    pub scratch: u64,
    pub checkpoints: u64,
}

impl Sizes {
    pub fn total(&self) -> u64 {
        self.log + self.scratch + self.checkpoints
    }

    fn add(&mut self, o: Sizes) {
        self.log += o.log;
        self.scratch += o.scratch;
        self.checkpoints += o.checkpoints;
    }
}

/// One session's disk use.
#[derive(Clone, Debug)]
pub struct SessionUse {
    pub id: String,
    /// Its project's folder name under the roots; empty for checkpoints
    /// whose session isn't found anywhere else.
    pub slug: String,
    pub sizes: Sizes,
    /// When its log (else its scratchpad) last changed.
    pub modified: SystemTime,
    /// It still has a log. Without one it's leftovers of a session that's gone.
    pub has_log: bool,
}

/// Every session found, biggest first.
#[derive(Clone, Debug, Default)]
pub struct Usage {
    pub sessions: Vec<SessionUse>,
}

impl Usage {
    pub fn total(&self) -> Sizes {
        let mut s = Sizes::default();
        self.sessions.iter().for_each(|u| s.add(u.sizes));
        s
    }

    /// Totals per project slug, biggest first.
    pub fn by_project(&self) -> Vec<(String, Sizes)> {
        let mut map: HashMap<&str, Sizes> = HashMap::new();
        for u in &self.sessions {
            map.entry(u.slug.as_str()).or_default().add(u.sizes);
        }
        let mut out: Vec<(String, Sizes)> = map.into_iter().map(|(k, v)| (k.to_string(), v)).collect();
        out.sort_by(|a, b| b.1.total().cmp(&a.1.total()).then_with(|| a.0.cmp(&b.0)));
        out
    }

    /// Sessions last used more than `days` ago, as of `now`, that `keep` doesn't
    /// keep (pinned, open, running).
    pub fn older_than(&self, days: u64, now: SystemTime, keep: impl Fn(&SessionUse) -> bool) -> Vec<&SessionUse> {
        let cutoff = now.checked_sub(Duration::from_secs(days * 86_400)).unwrap_or(SystemTime::UNIX_EPOCH);
        self.sessions.iter().filter(|u| u.modified < cutoff && !keep(u)).collect()
    }

    /// Scratchpads and checkpoints left from sessions whose log is gone.
    pub fn leftovers(&self) -> Vec<&SessionUse> {
        self.sessions.iter().filter(|u| !u.has_log && u.sizes.total() > 0).collect()
    }
}

/// How the sessions list is ordered.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sort {
    #[default]
    Size,
    /// Last used, newest first.
    Recent,
    /// By title, A to Z.
    Name,
}

/// The sessions (indices into `sessions`) whose label contains every word of
/// `query` (any case), in `sort` order. `label` is what a session is known by:
/// its title and project.
pub fn arrange(sessions: &[SessionUse], label: impl Fn(&SessionUse) -> String, query: &str, sort: Sort) -> Vec<usize> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    let mut found: Vec<(usize, String)> = sessions
        .iter()
        .enumerate()
        .map(|(i, u)| (i, label(u).to_lowercase()))
        .filter(|(_, l)| words.iter().all(|w| l.contains(w.as_str())))
        .collect();
    match sort {
        Sort::Size => found.sort_by(|a, b| sessions[b.0].sizes.total().cmp(&sessions[a.0].sizes.total())),
        Sort::Recent => found.sort_by(|a, b| sessions[b.0].modified.cmp(&sessions[a.0].modified)),
        Sort::Name => found.sort_by(|a, b| a.1.cmp(&b.1)),
    }
    found.into_iter().map(|(i, _)| i).collect()
}

/// Where each part of a session is on disk, whether or not it's there now.
pub fn locations(roots: &Roots, u: &SessionUse) -> Vec<(Part, PathBuf)> {
    Part::ALL.iter().filter_map(|&p| Some((p, paths(roots, &u.slug, &u.id, p)?.into_iter().next()?))).collect()
}

/// `cwd`'s slug as the folder under `root` is actually spelled: its letter
/// case follows whoever launched Claude (`C:` vs `c:`). Blocks on listing `root`.
fn slug_in(root: &Path, cwd: &Path) -> String {
    let slug = slug(cwd);
    subdirs(root).into_iter().map(|(name, _)| name).find(|name| name.eq_ignore_ascii_case(&slug)).unwrap_or(slug)
}

/// Where the scratchpads of the sessions run in `cwd` are: one folder per
/// session, named by its id. Blocks on listing the scratch root.
pub fn project_scratch(roots: &Roots, cwd: &Path) -> PathBuf {
    roots.scratch.join(slug_in(&roots.scratch, cwd))
}

/// Session `id`'s scratchpad folder, or `None` for something that isn't an id.
pub fn session_scratch(roots: &Roots, cwd: &Path, id: &str) -> Option<PathBuf> {
    is_id(id).then(|| project_scratch(roots, cwd).join(id))
}

/// One session's disk use, for session `id` run in folder `cwd`. Blocks: walks its folders.
pub fn session_sizes(roots: &Roots, cwd: &Path, id: &str) -> Sizes {
    let slug = slug_in(&roots.projects, cwd);
    let size = |part| paths(roots, &slug, id, part).map_or(0, |ps| ps.iter().map(|p| size_of(p)).sum());
    Sizes { log: size(Part::Log), scratch: size(Part::Scratch), checkpoints: size(Part::Checkpoints) }
}

/// The folder name Claude Code files `cwd`'s sessions under.
pub fn slug(cwd: &Path) -> String {
    cwd.to_string_lossy().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
}

/// A session id: a UUID, so it can't name anything but its own files.
pub fn is_id(s: &str) -> bool {
    s.len() == 36
        && s.char_indices().all(|(i, c)| if matches!(i, 8 | 13 | 18 | 23) { c == '-' } else { c.is_ascii_hexdigit() })
}

/// A project slug: letters, digits and `-` only, never a path.
fn is_slug(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// Bytes under `path`, a file or a folder (links not followed). Unreadable parts count as 0.
pub fn size_of(path: &Path) -> u64 {
    let Ok(meta) = std::fs::symlink_metadata(path) else { return 0 };
    if !meta.is_dir() {
        return meta.len();
    }
    std::fs::read_dir(path).map_or(0, |entries| entries.flatten().map(|e| size_of(&e.path())).sum())
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Every session's disk use under `roots`. Blocks: walks every session folder.
pub fn scan(roots: &Roots) -> Usage {
    let mut found: HashMap<String, SessionUse> = HashMap::new();
    fn entry<'a>(found: &'a mut HashMap<String, SessionUse>, id: &str, slug: &str) -> &'a mut SessionUse {
        let e = found.entry(id.to_string()).or_insert_with(|| SessionUse {
            id: id.to_string(),
            slug: String::new(),
            sizes: Sizes::default(),
            modified: SystemTime::UNIX_EPOCH,
            has_log: false,
        });
        if e.slug.is_empty() {
            e.slug = slug.to_string();
        }
        e
    }
    for (slug, dir) in subdirs(&roots.projects) {
        for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = e.path();
            let Some(id) = path.file_stem().map(|s| s.to_string_lossy().into_owned()).filter(|s| is_id(s)) else {
                continue;
            };
            if path.extension().is_some_and(|x| x == "jsonl") {
                let u = entry(&mut found, &id, &slug);
                u.slug = slug.clone();
                u.sizes.log += size_of(&path);
                u.has_log = true;
                u.modified = u.modified.max(modified(&path).unwrap_or(SystemTime::UNIX_EPOCH));
            } else if path.is_dir() && path.extension().is_none() {
                entry(&mut found, &id, &slug).sizes.log += size_of(&path);
            }
        }
    }
    for (slug, dir) in subdirs(&roots.scratch) {
        for (id, path) in subdirs(&dir).into_iter().filter(|(id, _)| is_id(id)) {
            let u = entry(&mut found, &id, &slug);
            u.sizes.scratch += size_of(&path);
            if !u.has_log {
                u.modified = u.modified.max(modified(&path).unwrap_or(SystemTime::UNIX_EPOCH));
            }
        }
    }
    for (id, path) in subdirs(&roots.history).into_iter().filter(|(id, _)| is_id(id)) {
        entry(&mut found, &id, "").sizes.checkpoints += size_of(&path);
    }
    let mut sessions: Vec<SessionUse> = found.into_values().collect();
    sessions.sort_by(|a, b| b.sizes.total().cmp(&a.sizes.total()).then_with(|| a.id.cmp(&b.id)));
    Usage { sessions }
}

fn subdirs(dir: &Path) -> Vec<(String, PathBuf)> {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
        .collect()
}

/// What to free of a session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    /// Its log, and the folder beside it.
    Log,
    Scratch,
    /// Rewind can't restore files once these are gone.
    Checkpoints,
}

impl Part {
    pub const ALL: [Part; 3] = [Part::Log, Part::Scratch, Part::Checkpoints];
}

/// The paths `part` of session `id` in project `slug` lives at, or `None` if
/// the id or slug could name anything else.
pub fn paths(roots: &Roots, slug: &str, id: &str, part: Part) -> Option<Vec<PathBuf>> {
    if !is_id(id) {
        return None;
    }
    let project = || Some(slug).filter(|s| is_slug(s));
    Some(match part {
        Part::Log => {
            let dir = roots.projects.join(project()?);
            vec![dir.join(format!("{id}.jsonl")), dir.join(id)]
        }
        Part::Scratch => vec![roots.scratch.join(project()?).join(id)],
        Part::Checkpoints => vec![roots.history.join(id)],
    })
}

/// Deletes `parts` of session `id` in project `slug`; returns the bytes freed.
/// Refuses ids and slugs that aren't one (see [`is_id`]). Blocks.
pub fn delete(roots: &Roots, slug: &str, id: &str, parts: &[Part]) -> Result<u64, String> {
    let mut freed = 0;
    for &part in parts {
        // A part with no project (checkpoints found alone) has nothing to delete there.
        let Some(paths) = paths(roots, slug, id, part) else {
            if !is_id(id) {
                return Err(format!("\"{id}\" isn't a session id"));
            }
            continue;
        };
        for p in paths {
            let size = size_of(&p);
            let gone = match std::fs::symlink_metadata(&p) {
                Err(_) => continue,
                Ok(m) if m.is_dir() => std::fs::remove_dir_all(&p),
                Ok(_) => std::fs::remove_file(&p),
            };
            gone.map_err(|e| format!("Couldn't delete {}: {e}", p.display()))?;
            freed += size;
        }
    }
    Ok(freed)
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "0137a8e3-4603-48d4-aa35-9d22086a1e3c";
    const B: &str = "036f425b-51c5-4546-9e69-7820ca1a58b7";
    const C: &str = "041d8416-85b4-436b-bab8-02dbbd98d3c9";

    fn roots(name: &str) -> Roots {
        let base = std::env::temp_dir().join(format!("kynex-storage-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        Roots { projects: base.join("projects"), scratch: base.join("scratch"), history: base.join("history") }
    }

    fn write(path: PathBuf, bytes: usize) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, vec![b'x'; bytes]).unwrap();
    }

    fn cleanup(r: &Roots) {
        let _ = std::fs::remove_dir_all(r.projects.parent().unwrap());
    }

    fn sample(name: &str) -> Roots {
        let r = roots(name);
        write(r.projects.join("C--work-app").join(format!("{A}.jsonl")), 100);
        write(r.projects.join("C--work-app").join(A).join("subagents").join("x.jsonl"), 50);
        write(r.projects.join("C--work-app").join(format!("{B}.jsonl")), 10);
        write(r.projects.join("C--work-app").join("memory").join("notes.md"), 999);
        write(r.scratch.join("C--work-app").join(A).join("tasks").join("out.txt"), 30);
        // A scratchpad whose session is gone.
        write(r.scratch.join("C--work-app").join(C).join("f"), 7);
        write(r.history.join(A).join("abc@v1"), 20);
        r
    }

    #[test]
    fn sizes_each_session_by_kind() {
        let r = sample("scan");
        let usage = scan(&r);
        let ids: Vec<&str> = usage.sessions.iter().map(|u| u.id.as_str()).collect();
        assert_eq!(ids, [A, B, C]);
        let a = &usage.sessions[0];
        assert_eq!(a.sizes, Sizes { log: 150, scratch: 30, checkpoints: 20 });
        assert_eq!(a.slug, "C--work-app");
        assert!(a.has_log);
        // Folders that aren't sessions (`memory`) don't count.
        assert_eq!(usage.total().total(), 200 + 10 + 7);
        assert_eq!(usage.by_project(), [("C--work-app".to_string(), Sizes { log: 160, scratch: 37, checkpoints: 20 })]);
        assert_eq!(usage.leftovers().iter().map(|u| u.id.as_str()).collect::<Vec<_>>(), [C]);
        cleanup(&r);
    }

    #[test]
    fn sizes_one_session() {
        let r = sample("one");
        let s = session_sizes(&r, Path::new("C:/work/app"), A);
        assert_eq!(s, Sizes { log: 150, scratch: 30, checkpoints: 20 });
        // The folder's case doesn't matter.
        assert_eq!(session_sizes(&r, Path::new("c:/work/app"), A).log, 150);
        assert_eq!(session_sizes(&r, Path::new("C:/work/app"), "nope"), Sizes::default());
        cleanup(&r);
    }

    #[test]
    fn finds_scratch_folders() {
        let r = sample("scratch-dirs");
        assert_eq!(project_scratch(&r, Path::new("c:/work/app")), r.scratch.join("C--work-app"));
        assert_eq!(session_scratch(&r, Path::new("C:/work/app"), A), Some(r.scratch.join("C--work-app").join(A)));
        assert_eq!(session_scratch(&r, Path::new("C:/work/app"), ".."), None);
        // A project never run yet: its folder as it will be named.
        assert_eq!(project_scratch(&r, Path::new("D:/new")), r.scratch.join("D--new"));
        cleanup(&r);
    }

    #[test]
    fn deletes_only_the_parts_asked_for() {
        let r = sample("delete");
        assert_eq!(delete(&r, "C--work-app", A, &[Part::Scratch]).unwrap(), 30);
        assert!(r.projects.join("C--work-app").join(format!("{A}.jsonl")).exists());
        assert!(r.history.join(A).exists());
        assert_eq!(delete(&r, "C--work-app", A, &Part::ALL).unwrap(), 170);
        assert!(!r.projects.join("C--work-app").join(A).exists());
        assert!(r.projects.join("C--work-app").join(format!("{B}.jsonl")).exists());
        assert!(r.projects.join("C--work-app").join("memory").exists());
        // Gone already: nothing freed, no error.
        assert_eq!(delete(&r, "C--work-app", A, &Part::ALL).unwrap(), 0);
        cleanup(&r);
    }

    #[test]
    fn refuses_anything_but_a_session() {
        let r = sample("guard");
        assert!(delete(&r, "C--work-app", "..", &Part::ALL).is_err());
        assert!(delete(&r, "C--work-app", "memory", &Part::ALL).is_err());
        assert!(delete(&r, "C--work-app", &format!("{A}/.."), &Part::ALL).is_err());
        assert!(paths(&r, "..", A, Part::Log).is_none());
        assert!(paths(&r, "C--work-app/../x", A, Part::Scratch).is_none());
        assert!(paths(&r, "", A, Part::Log).is_none());
        // A bad slug still lets checkpoints (keyed by id alone) go, and nothing else.
        assert_eq!(delete(&r, "..", A, &Part::ALL).unwrap(), 20);
        assert!(r.projects.join("C--work-app").join(format!("{A}.jsonl")).exists());
        assert!(r.projects.join("C--work-app").join("memory").exists());
        cleanup(&r);
    }

    #[test]
    fn ids_and_slugs() {
        assert!(is_id(A));
        assert!(!is_id("0137a8e3-4603-48d4-aa35-9d22086a1e3"));
        assert!(!is_id("0137a8e3x4603-48d4-aa35-9d22086a1e3c"));
        assert!(!is_id("../../../../../../../../../../../.."));
        assert_eq!(slug(Path::new(r"C:\Users\me\repos\kynex")), "C--Users-me-repos-kynex");
        assert_eq!(slug(Path::new("/home/me/my app")), "-home-me-my-app");
    }

    #[test]
    fn filters_and_sorts() {
        let day = Duration::from_secs(86_400);
        let now = SystemTime::UNIX_EPOCH + day * 1000;
        let at = |id: &str, size: u64, days_ago: u32| SessionUse {
            id: id.into(),
            slug: "p".into(),
            sizes: Sizes { log: size, ..Default::default() },
            modified: now - day * days_ago,
            has_log: true,
        };
        let sessions = vec![at(A, 10, 1), at(B, 300, 50), at(C, 20, 5)];
        let label = |u: &SessionUse| match u.id.as_str() {
            A => "Fix the parser · kynex".to_string(),
            B => "Deploy notes · otto".to_string(),
            _ => "Parser tests · kynex".to_string(),
        };
        assert_eq!(arrange(&sessions, label, "", Sort::Size), [1, 2, 0]);
        assert_eq!(arrange(&sessions, label, "", Sort::Recent), [0, 2, 1]);
        assert_eq!(arrange(&sessions, label, "", Sort::Name), [1, 0, 2]);
        assert_eq!(arrange(&sessions, label, "PARSER kynex", Sort::Size), [2, 0]);
        assert!(arrange(&sessions, label, "parser otto", Sort::Size).is_empty());
    }

    #[test]
    fn locates_each_part() {
        let r = roots("locate");
        let u = SessionUse {
            id: A.into(),
            slug: "C--work-app".into(),
            sizes: Sizes::default(),
            modified: SystemTime::UNIX_EPOCH,
            has_log: true,
        };
        let parts: Vec<Part> = locations(&r, &u).into_iter().map(|(p, _)| p).collect();
        assert_eq!(parts, Part::ALL);
        assert_eq!(locations(&r, &u)[1].1, r.scratch.join("C--work-app").join(A));
        // No project: only the checkpoints can be found.
        let lone = SessionUse { slug: String::new(), ..u };
        assert_eq!(locations(&r, &lone).len(), 1);
    }

    #[test]
    fn old_sessions_skip_what_is_kept() {
        let day = Duration::from_secs(86_400);
        let now = SystemTime::UNIX_EPOCH + day * 1000;
        let at = |id: &str, days_ago: u32| SessionUse {
            id: id.into(),
            slug: "p".into(),
            sizes: Sizes::default(),
            modified: now - day * days_ago,
            has_log: true,
        };
        let usage = Usage { sessions: vec![at(A, 100), at(B, 10), at(C, 400)] };
        let old: Vec<&str> = usage.older_than(90, now, |u| u.id == C).iter().map(|u| u.id.as_str()).collect();
        assert_eq!(old, [A]);
        assert_eq!(usage.older_than(365, now, |_| false).len(), 1);
    }
}
