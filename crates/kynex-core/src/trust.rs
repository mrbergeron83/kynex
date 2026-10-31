//! The CLI's workspace trust. Claude Code asks once per folder whether you trust
//! its files and records the answer as `projects["<folder>"].hasTrustDialogAccepted`
//! in `~/.claude.json`. Until then it ignores what a folder's own config asks for
//! (`permissions.allow` in `.claude/settings.json`, `.mcp.json` servers…). Kynex
//! never shows the CLI's dialog, so it asks itself and records the answer the same way.

use std::path::Path;

use serde_json::{Map, Value};

use crate::extensions::{Paths, norm, read_json};

/// The folder files the CLI only honours in a trusted folder.
const GATED: [&str; 3] = [".claude/settings.json", ".claude/settings.local.json", ".mcp.json"];

/// The trust-gated files in `folder` when the CLI doesn't trust it, so Kynex
/// should ask; empty when it's trusted or has nothing the CLI would ignore. Blocks.
pub fn needs_trust(folder: &Path) -> Vec<&'static str> {
    let gated: Vec<_> = GATED.into_iter().filter(|f| folder.join(f).is_file()).collect();
    if gated.is_empty() {
        return gated;
    }
    let Some(paths) = Paths::detect() else { return Vec::new() };
    // An unreadable config isn't ours to judge; the CLI will say what's wrong.
    match read_json(&paths.global) {
        Ok(global) if !is_trusted(&global, folder) => gated,
        _ => Vec::new(),
    }
}

/// Whether the CLI trusts `folder`: it or a folder above it was accepted.
pub fn is_trusted(global: &Value, folder: &Path) -> bool {
    let Some(projects) = global["projects"].as_object() else { return false };
    folder.ancestors().any(|dir| {
        let key = norm(&dir.to_string_lossy());
        projects.get(&key).is_some_and(|p| p["hasTrustDialogAccepted"] == Value::Bool(true))
    })
}

/// Records that you trust `folder`, as the CLI's dialog does. Blocks.
pub fn trust(folder: &Path) -> Result<(), String> {
    let paths = Paths::detect().ok_or("Couldn't find your home folder.")?;
    trust_in(&paths.global, folder)
}

/// [`trust`] with the CLI's config at `path`. The rest of the file stays as it was.
fn trust_in(path: &Path, folder: &Path) -> Result<(), String> {
    let mut global = read_json(path)?;
    mark_trusted(&mut global, folder);
    crate::project::write_json(path, &global)
}

/// Sets the flag on `folder`'s entry, adding the entry (and `projects`) if needed.
fn mark_trusted(global: &mut Value, folder: &Path) {
    if !global.is_object() {
        *global = Value::Object(Map::new());
    }
    let projects = global.as_object_mut().expect("an object").entry("projects").or_insert_with(|| Map::new().into());
    if !projects.is_object() {
        *projects = Map::new().into();
    }
    let key = norm(&folder.to_string_lossy());
    let entry = projects.as_object_mut().expect("an object").entry(key).or_insert_with(|| Map::new().into());
    if !entry.is_object() {
        *entry = Map::new().into();
    }
    entry["hasTrustDialogAccepted"] = Value::Bool(true);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A fresh folder for one test.
    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("kynex-trust-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn root() -> std::path::PathBuf {
        if cfg!(windows) { "C:\\work".into() } else { "/work".into() }
    }

    #[test]
    fn a_folder_or_one_above_it_counts() {
        let repo = root().join("repo");
        let key = |p: &Path| norm(&p.to_string_lossy());
        let global = json!({ "projects": { key(&repo): { "hasTrustDialogAccepted": true } } });
        assert!(is_trusted(&global, &repo));
        assert!(is_trusted(&global, &repo.join("sub")));
        assert!(!is_trusted(&global, &root()));
        assert!(!is_trusted(&global, &root().join("other")));

        let refused = json!({ "projects": { key(&repo): { "hasTrustDialogAccepted": false } } });
        assert!(!is_trusted(&refused, &repo));
        assert!(!is_trusted(&json!({}), &repo));
        assert!(!is_trusted(&Value::Null, &repo));
    }

    #[test]
    fn trusting_keeps_the_rest_of_the_file() {
        let dir = scratch("keep");
        let path = dir.join(".claude.json");
        let repo = root().join("repo");
        let key = norm(&repo.to_string_lossy());
        let before = json!({
            "numStartups": 3,
            "projects": { key.clone(): { "allowedTools": [], "lastCost": 1.5 }, "/elsewhere": {} },
            "userID": "x",
        });
        std::fs::write(&path, serde_json::to_string_pretty(&before).unwrap()).unwrap();

        trust_in(&path, &repo).unwrap();
        let after = read_json(&path).unwrap();
        assert!(is_trusted(&after, &repo));
        assert_eq!(after["projects"][&key]["lastCost"], 1.5);
        assert_eq!(after["numStartups"], 3);
        assert_eq!(after["projects"]["/elsewhere"], json!({}));
        // The CLI's own key order survives.
        let keys: Vec<_> = after.as_object().unwrap().keys().cloned().collect();
        assert_eq!(keys, ["numStartups", "projects", "userID"]);
    }

    #[test]
    fn trusting_starts_a_missing_file_or_entry() {
        let dir = scratch("start");
        let path = dir.join(".claude.json");
        let repo = root().join("repo");
        trust_in(&path, &repo).unwrap();
        assert!(is_trusted(&read_json(&path).unwrap(), &repo));

        let mut odd = json!({ "projects": [] });
        mark_trusted(&mut odd, &repo);
        assert!(is_trusted(&odd, &repo));
    }

    #[test]
    fn asks_only_when_there_is_something_to_ignore() {
        assert!(needs_trust(&scratch("empty")).is_empty());
    }
}
