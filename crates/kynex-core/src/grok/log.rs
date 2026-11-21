//! Grok Build's own record of sessions (`~/.grok/sessions/<cwd, percent-encoded>/<id>/summary.json`),
//! read back for the sessions list and session deletion.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::Value;

use crate::agent_cli;
use crate::sessions::{SessionInfo, parse_iso_time};

/// `~/.grok/sessions`.
fn root() -> Option<PathBuf> {
    Some(dirs::home_dir()?.join(".grok").join("sessions"))
}

/// Whether Grok's records can be read, so a session missing from them is really gone.
pub fn readable() -> bool {
    root().is_some_and(|r| std::fs::read_dir(r).is_ok())
}

/// `cwd` as Grok names its folder: percent-encoded, all but `A–Z a–z 0–9 - . _ ~`.
fn folder_name(cwd: &Path) -> String {
    let mut out = String::new();
    for b in cwd.to_string_lossy().bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn safe_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Lists all Grok sessions stored directly under `cwd`'s folder in `~/.grok/sessions/`.
pub fn list(cwd: &Path) -> Vec<SessionInfo> {
    let Some(dir) = root().map(|r| r.join(folder_name(cwd))).filter(|p| p.is_dir()) else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&dir) else { return Vec::new() };
    entries
        .filter_map(Result::ok)
        .filter_map(|e| {
            let path = e.path();
            if !path.is_dir() {
                return None;
            }
            let id = path.file_name()?.to_str()?.to_string();
            if !safe_id(&id) {
                return None;
            }
            let summary =
                std::fs::read_to_string(path.join("summary.json")).ok().and_then(|s| serde_json::from_str(&s).ok());
            let modified = || std::fs::metadata(&path).and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
            Some(info_from(&id, &summary.unwrap_or(Value::Null), modified))
        })
        .collect()
}

/// Where Grok keeps session `id` of `cwd`. Blocks on file system.
pub fn session_dir(cwd: &Path, id: &str) -> Option<PathBuf> {
    if !safe_id(id) {
        return None;
    }
    let root = root()?;
    let dir = root.join(folder_name(cwd)).join(id);
    if dir.is_dir() {
        return Some(dir);
    }
    std::fs::read_dir(root).ok()?.filter_map(Result::ok).map(|e| e.path().join(id)).find(|p| p.is_dir())
}

/// Session `id` of `cwd` for the sessions list, from Grok's `summary.json`.
pub fn info(cwd: &Path, id: &str) -> Option<SessionInfo> {
    let dir = session_dir(cwd, id)?;
    let summary = std::fs::read_to_string(dir.join("summary.json")).ok().and_then(|s| serde_json::from_str(&s).ok());
    let modified = || std::fs::metadata(&dir).and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
    Some(info_from(id, &summary.unwrap_or(Value::Null), modified))
}

/// [`info`] from a parsed `summary.json`; `modified` dates it when it has no time of its own.
pub(crate) fn info_from(id: &str, v: &Value, modified: impl FnOnce() -> SystemTime) -> SessionInfo {
    let text = |k: &str| v[k].as_str().map(str::trim).filter(|t| !t.is_empty()).map(str::to_string);
    let title = text("generated_title").or_else(|| text("session_summary"));
    let at =
        ["last_active_at", "updated_at", "created_at"].iter().find_map(|k| v[*k].as_str().and_then(parse_iso_time));
    SessionInfo {
        title: title.clone().unwrap_or_else(|| "Grok session".into()),
        first: title.clone().unwrap_or_default(),
        named: title.is_some(),
        id: id.to_string(),
        modified: at.unwrap_or_else(modified),
        branch: None,
        ran_in: text("cwd").or_else(|| v["info"]["cwd"].as_str().map(str::to_string)).map(PathBuf::from),
        provider: agent_cli::GROK.into(),
    }
}

/// Deletes Grok's record of session `id`; false if it had none. Blocks.
pub(crate) fn delete(cwd: &Path, id: &str) -> Result<bool, String> {
    let Some(dir) = session_dir(cwd, id) else { return Ok(false) };
    std::fs::remove_dir_all(&dir).map_err(|e| format!("couldn't delete {}: {e}", dir.display()))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn folders_are_named_as_grok_names_them() {
        assert_eq!(folder_name(Path::new(r"C:\Users\me\repos\kynex")), "C%3A%5CUsers%5Cme%5Crepos%5Ckynex");
        assert_eq!(folder_name(Path::new(r"C:\Programs\Gaea 2.0")), "C%3A%5CPrograms%5CGaea%202.0");
        assert_eq!(folder_name(Path::new("/home/me/a_b~c")), "%2Fhome%2Fme%2Fa_b~c");
    }

    #[test]
    fn only_plain_ids_name_a_folder() {
        assert!(safe_id("01a1997d-0dc6-7d11-b22d-3a5e9997eeb5"));
        for bad in ["", "..", "../x", r"a\b", "a/b"] {
            assert!(!safe_id(bad), "{bad}");
        }
    }

    #[test]
    fn a_summary_names_and_dates_the_session() {
        let v = json!({ "info": { "id": "x", "cwd": "C:\\work" }, "generated_title": "Add a smoke line",
                        "session_summary": "older", "last_active_at": "2026-11-02T00:19:30.424996700Z" });
        let info = info_from("x", &v, || SystemTime::UNIX_EPOCH);
        assert_eq!(info.title, "Add a smoke line");
        assert!(info.named);
        assert_eq!(info.provider, agent_cli::GROK);
        assert_eq!(info.ran_in, Some(PathBuf::from("C:\\work")));
        assert!(info.modified > SystemTime::UNIX_EPOCH);
        let bare = info_from("y", &Value::Null, || SystemTime::UNIX_EPOCH);
        assert_eq!(bare.title, "Grok session");
        assert!(!bare.named);
    }
}
