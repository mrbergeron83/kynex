//! Per-project data kept in Kynex's own data dir rather than inside the repo:
//!
//! ```text
//! <data dir>/projects/<name>-<hash>/
//!     project.json        which folder this is
//!     sessions.json       session groups, archive, pins, custom names, providers
//!     workspaces.json     workspaces, saved pane layouts, layout history
//!     appearance.json     the project's colour
//!     worktrees.json      where new worktrees go, if not the app-wide folder
//! ```

use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::paths::data_dir;

/// The settings folder for a project, e.g. `projects/kynex-3f9a1c0d`.
pub fn dir(project: &Path) -> PathBuf {
    data_dir().join("projects").join(folder_name(project))
}

/// Readable but unique: the folder's name plus a hash of its full path, so two repos both
/// called `api` don't share settings.
pub(crate) fn folder_name(project: &Path) -> String {
    let name = project.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut slug: String =
        name.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' { c.to_ascii_lowercase() } else { '-' }
            })
            .take(40)
            .collect();
    if slug.trim_matches(['-', '.']).is_empty() {
        slug = "project".into();
    }
    format!("{slug}-{:08x}", fnv1a(normalize(project).as_bytes()) as u32)
}

/// One spelling per folder: no trailing separator, and on Windows one case and one kind of slash.
pub(crate) fn normalize(p: &Path) -> String {
    let s = p.to_string_lossy();
    let s = s.trim_end_matches(['/', '\\']);
    if cfg!(windows) { s.replace('/', "\\").to_lowercase() } else { s.to_string() }
}

fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ *b as u64).wrapping_mul(0x0000_0100_0000_01b3))
}

#[derive(Serialize, Deserialize)]
struct ProjectInfo {
    path: PathBuf,
}

pub(crate) fn load<T: DeserializeOwned + Default>(project: &Path, file: &str) -> T {
    crate::migrate::read(&dir(project).join(file)).0
}

/// Writes one section, creating the project's folder (and `project.json`) on first use.
pub(crate) fn save<T: Serialize>(project: &Path, file: &str, value: &T) -> Result<(), String> {
    let dir = dir(project);
    std::fs::create_dir_all(&dir).map_err(|e| format!("Couldn't create {}: {e}", dir.display()))?;
    let info = dir.join("project.json");
    if !info.exists() {
        write_json(&info, &ProjectInfo { path: project.to_path_buf() })?;
    }
    let path = dir.join(file);
    if let Ok(text) = std::fs::read_to_string(&path)
        && let crate::migrate::Loaded::Newer { found } = crate::migrate::migrate(&text)
    {
        return Err(format!("{} was saved by a newer Kynex (format v{found}); not overwriting it", path.display()));
    }
    write_json(&path, value)
}

/// Atomic JSON write via temp file + rename.
pub(crate) fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let s = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, s)
        .and_then(|_| std::fs::rename(&tmp, path))
        .map_err(|e| format!("Couldn't save {}: {e}", path.display()))
}

/// The project colour names supported by the theme palette.
pub const COLOURS: [&str; 8] = ["red", "orange", "yellow", "green", "teal", "blue", "violet", "pink"];

/// A deterministic colour from [`COLOURS`] derived from `project`'s path.
pub fn auto_colour(project: &Path) -> &'static str {
    let h = fnv1a(normalize(project).as_bytes()) as usize;
    COLOURS[h % COLOURS.len()]
}

/// How a project's colour is chosen.
#[derive(Clone, Default, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Colour {
    #[default]
    Auto,
    Off,
    #[serde(untagged)]
    Named(String),
}

/// The project's colour (title bar, tab, focused tile border).
#[derive(Clone, Default, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    pub colour: Colour,
}

impl Appearance {
    const FILE: &str = "appearance.json";

    pub fn load(project: &Path) -> Self {
        load(project, Self::FILE)
    }

    pub fn save(&self, project: &Path) -> Result<(), String> {
        save(project, Self::FILE, self)
    }

    pub fn colour(&self, project: &Path) -> Option<&str> {
        match &self.colour {
            Colour::Auto => Some(auto_colour(project)),
            Colour::Off => None,
            Colour::Named(s) if s.is_empty() => None,
            Colour::Named(s) => Some(s.as_str()),
        }
    }
}

/// Where new git worktrees for this project go, if not the app-wide folder.
#[derive(Clone, Default, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WorktreeSettings {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root: Option<PathBuf>,
}

impl WorktreeSettings {
    const FILE: &str = "worktrees.json";

    pub fn load(project: &Path) -> Self {
        load(project, Self::FILE)
    }

    pub fn save(&self, project: &Path) -> Result<(), String> {
        save(project, Self::FILE, self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_names_are_readable_and_unique() {
        let a = folder_name(Path::new("/work/api"));
        let b = folder_name(Path::new("/other/api"));
        assert!(a.starts_with("api-") && b.starts_with("api-"));
        assert_ne!(a, b);
    }

    #[test]
    fn appearance_and_worktree_settings_round_trip() {
        let a = Appearance { colour: Colour::Named("blue".into()) };
        let back: Appearance = serde_json::from_str(&serde_json::to_string(&a).unwrap()).unwrap();
        assert_eq!(back, a);

        let w = WorktreeSettings { root: Some(PathBuf::from("../trees")) };
        let back: WorktreeSettings = serde_json::from_str(&serde_json::to_string(&w).unwrap()).unwrap();
        assert_eq!(back, w);
    }
}
