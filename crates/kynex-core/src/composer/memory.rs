//! `#` notes: a line added to one of the memory files Claude Code reads at the start
//! of every session.

use std::io::Write;
use std::path::{Path, PathBuf};

/// Which memory file a note goes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// `CLAUDE.md` in the project: shared with everyone who works on it.
    Project,
    /// `CLAUDE.local.md` in the project: yours only (keep it out of git).
    Local,
    /// `~/.claude/CLAUDE.md`: yours, in every project.
    User,
}

impl Target {
    pub const ALL: [Target; 3] = [Target::Project, Target::Local, Target::User];

    pub fn label(self) -> &'static str {
        match self {
            Target::Project => "Project memory",
            Target::Local => "Project memory, just mine",
            Target::User => "My memory, every project",
        }
    }

    /// The file it names for a session in `folder`; `None` if the home folder is unknown.
    pub fn file(self, folder: &Path) -> Option<PathBuf> {
        Some(self.path(folder, &dirs::home_dir()?))
    }

    /// The file it names for a session in `folder`, with `home` the user's home folder.
    pub fn path(self, folder: &Path, home: &Path) -> PathBuf {
        match self {
            Target::Project => folder.join("CLAUDE.md"),
            Target::Local => folder.join("CLAUDE.local.md"),
            Target::User => home.join(".claude").join("CLAUDE.md"),
        }
    }
}

/// Adds `note` to `target`'s file for a session in `folder`; returns the file. Blocks.
pub fn add(target: Target, folder: &Path, note: &str) -> Result<PathBuf, String> {
    let path = target.file(folder).ok_or("Couldn't find your home folder")?;
    append(&path, note)?;
    Ok(path)
}

/// Adds `note` to the memory file at `path` as a list item on a line of its own,
/// creating the file (and its folder) if need be. Blocks.
pub fn append(path: &Path, note: &str) -> Result<(), String> {
    let note = note.trim();
    if note.is_empty() {
        return Err("Nothing to remember".into());
    }
    let fail = |e: std::io::Error| format!("Couldn't add to {}: {e}", path.display());
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(fail)?;
    }
    let before = std::fs::read(path).unwrap_or_default();
    // Keep the file's line endings.
    let nl = if before.windows(2).any(|w| w == b"\r\n") { "\r\n" } else { "\n" };
    let mut text = String::new();
    if !before.is_empty() && !before.ends_with(b"\n") {
        text.push_str(nl);
    }
    let line = note.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" ");
    text.push_str(&format!("- {line}{nl}"));
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path).map_err(fail)?;
    file.write_all(text.as_bytes()).map_err(fail)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kynex-memory-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn targets_name_the_cli_files() {
        let (folder, home) = (Path::new("/p"), Path::new("/h"));
        assert_eq!(Target::Project.path(folder, home), Path::new("/p/CLAUDE.md"));
        assert_eq!(Target::Local.path(folder, home), Path::new("/p/CLAUDE.local.md"));
        assert_eq!(Target::User.path(folder, home), Path::new("/h/.claude/CLAUDE.md"));
    }

    #[test]
    fn appends_a_line_creating_the_file() {
        let dir = scratch("new");
        let path = dir.join(".claude").join("CLAUDE.md");
        append(&path, "  use tabs  ").unwrap();
        append(&path, "run tests\nbefore commits").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "- use tabs\n- run tests before commits\n");
        assert!(append(&path, "  ").is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn keeps_the_files_line_endings_and_last_line() {
        let dir = scratch("crlf");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("CLAUDE.md");
        std::fs::write(&path, "# Notes\r\n- one").unwrap();
        append(&path, "two").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "# Notes\r\n- one\r\n- two\r\n");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
