//! A session's files besides its scratch folder, for the session explorer:
//! the project files it edited, the images in its conversation and the
//! output of its background tasks, all from its items, newest first.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::Value;

use crate::git::Status;
use crate::links;
use crate::session::Item;

/// The tools that write project files, and where their input names the file.
const EDITS: &[&str] = &["Edit", "MultiEdit", "Write", "NotebookEdit"];

/// An image in the conversation: which item and which of its images.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageRef {
    pub item: usize,
    pub index: usize,
    /// Where it came from: "Your message", "Read", "Browser screenshot"…
    pub label: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionFiles {
    /// Project files Edit / Write / MultiEdit / NotebookEdit touched, resolved against the folder.
    pub edited: Vec<PathBuf>,
    pub images: Vec<ImageRef>,
    /// Files background commands and agents wrote their output to.
    pub outputs: Vec<PathBuf>,
}

impl SessionFiles {
    pub fn count(&self) -> usize {
        self.edited.len() + self.images.len() + self.outputs.len()
    }
}

/// What `items` (a session in `cwd`) left, newest first, each file once.
pub fn gather(items: &[Item], cwd: &Path) -> SessionFiles {
    let mut files = SessionFiles::default();
    let (mut edited, mut outputs) = (HashSet::new(), HashSet::new());
    let tool_of = |id: &str| {
        items.iter().find_map(|i| match i {
            Item::ToolUse { id: call, name, .. } if call == id => Some(name.clone()),
            _ => None,
        })
    };
    for (ix, item) in items.iter().enumerate().rev() {
        match item {
            Item::ToolUse { name, input, .. } if EDITS.contains(&name.as_str()) => {
                let v: Value = serde_json::from_str(input).unwrap_or_default();
                let Some(path) = v["file_path"].as_str().or(v["notebook_path"].as_str()).filter(|p| !p.is_empty())
                else {
                    continue;
                };
                let path = links::resolve(cwd, path);
                if edited.insert(path.clone()) {
                    files.edited.push(path);
                }
            }
            Item::User { images, .. } => {
                for index in (0..images.len()).rev() {
                    files.images.push(ImageRef { item: ix, index, label: "Your message".into() });
                }
            }
            Item::ToolResult { id, images, .. } => {
                let tool = tool_of(id);
                for index in (0..images.len()).rev() {
                    let label = match tool.as_deref() {
                        Some(t) if t.contains("screenshot") => "Browser screenshot".to_string(),
                        Some(t) => crate::mcp::describe(t, &Value::Null).map_or_else(|| t.to_string(), |(l, _)| l),
                        None => "A tool's result".to_string(),
                    };
                    files.images.push(ImageRef { item: ix, index, label });
                }
            }
            Item::Task(notice) => {
                if let Some(p) = &notice.output_file
                    && outputs.insert(p.clone())
                {
                    files.outputs.push(p.clone());
                }
            }
            _ => {}
        }
    }
    files
}

/// Whether `path` has changes git hasn't committed, by `status` (whose paths
/// are relative to the repository's top, which may be above the session's folder).
pub fn changed(status: &Status, path: &Path) -> bool {
    let full = path.to_string_lossy().replace('\\', "/").to_lowercase();
    status.files.iter().any(|f| full.ends_with(&format!("/{}", f.path.to_lowercase())))
}

/// Most files [`count_newer`] looks at.
const COUNT_CAP: usize = 10_000;

/// How many files under `dir` changed after `since`. Blocks: walks the folder.
pub fn count_newer(dir: &Path, since: SystemTime) -> usize {
    let (mut n, mut seen) = (0, 0);
    let mut dirs = vec![dir.to_path_buf()];
    while let Some(d) = dirs.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            seen += 1;
            if seen > COUNT_CAP {
                return n;
            }
            let Ok(meta) = e.metadata() else { continue };
            if meta.is_dir() {
                dirs.push(e.path());
            } else if meta.modified().is_ok_and(|m| m > since) {
                n += 1;
            }
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use super::*;
    use crate::session::Timing;
    use crate::tasks::Notice;

    fn call(id: &str, name: &str, input: &str) -> Item {
        Item::ToolUse {
            id: id.into(),
            name: name.into(),
            summary: String::new(),
            input: input.into(),
            timing: Timing::default(),
        }
    }

    fn image() -> crate::session::Preview {
        Arc::new(())
    }

    #[test]
    fn gathers_edits_images_and_outputs_newest_first() {
        let cwd = Path::new("/work/app");
        let notice = Notice {
            task_id: "b1".into(),
            tool_use_id: "x".into(),
            status: crate::tasks::Status::Completed,
            summary: String::new(),
            output_file: Some(PathBuf::from("/tmp/b1.output")),
            duration_ms: None,
        };
        let items = vec![
            Item::User { text: "look".into(), images: vec![image()] },
            call("a", "Edit", r#"{"file_path":"src/main.rs"}"#),
            call("b", "mcp__playwright__browser_take_screenshot", "{}"),
            Item::ToolResult { id: "b".into(), content: String::new(), images: vec![image()], is_error: false },
            call("c", "Write", r#"{"file_path":"/work/app/README.md"}"#),
            call("d", "Edit", r#"{"file_path":"src/main.rs"}"#),
            call("e", "Read", r#"{"file_path":"src/lib.rs"}"#),
            Item::Task(notice.clone()),
            Item::Task(notice),
        ];
        let f = gather(&items, cwd);
        assert_eq!(f.edited, [cwd.join("src/main.rs"), PathBuf::from("/work/app/README.md")]);
        assert_eq!(
            f.images,
            [
                ImageRef { item: 3, index: 0, label: "Browser screenshot".into() },
                ImageRef { item: 0, index: 0, label: "Your message".into() },
            ]
        );
        assert_eq!(f.outputs, [PathBuf::from("/tmp/b1.output")]);
        assert_eq!(f.count(), 5);
    }

    #[test]
    fn marks_files_git_has_changes_for() {
        let status = crate::git::Status {
            files: vec![crate::git::FileStatus {
                path: "src/main.rs".into(),
                orig_path: None,
                staged: None,
                unstaged: None,
            }],
            ..Default::default()
        };
        assert!(changed(&status, Path::new(r"C:\work\app\src\main.rs")));
        assert!(!changed(&status, Path::new("/work/app/src/lib.rs")));
        assert!(!changed(&status, Path::new("/work/app/xsrc/main.rs")));
    }

    #[test]
    fn counts_files_changed_since() {
        let dir = std::env::temp_dir().join(format!("kynex-newer-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a"), "").unwrap();
        std::fs::write(dir.join("sub").join("b"), "").unwrap();
        let long_ago = SystemTime::now() - Duration::from_secs(3600);
        assert_eq!(count_newer(&dir, long_ago), 2);
        assert_eq!(count_newer(&dir, SystemTime::now() + Duration::from_secs(3600)), 0);
        assert_eq!(count_newer(&dir.join("nope"), long_ago), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
