//! Versioned saved files (`state.json`, per-project JSON): old files are brought up to
//! `CURRENT`, and a file from a newer Kynex is kept aside rather than overwritten.

use std::path::Path;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

/// The format this build writes.
pub const CURRENT: u32 = 4;

/// `MIGRATIONS[i]` turns a file at version `i` into version `i + 1`.
const MIGRATIONS: &[fn(&mut Map<String, Value>)] = &[v0_to_v1, v1_to_v2, v2_to_v3, v3_to_v4];

const _: () = assert!(MIGRATIONS.len() == CURRENT as usize);

/// Version 0 is every file written before versions existed; nothing to change yet.
fn v0_to_v1(_: &mut Map<String, Value>) {}

/// KNX Ops is gone (#353): a state saved with its view showing on the Chat tab opens on the
/// first project instead, and its saved fields and the project settings it added are dropped.
fn v1_to_v2(map: &mut Map<String, Value>) {
    let on_chat = match map.get("active") {
        Some(Value::String(s)) => s.eq_ignore_ascii_case("chat"),
        _ => false,
    };
    let has_projects = map.get("projects").and_then(Value::as_array).is_some_and(|p| !p.is_empty());
    if map.get("chief_view") == Some(&Value::Bool(true)) && on_chat && has_projects {
        map.insert("active".into(), serde_json::json!({ "Project": 0 }));
    }
    for key in ["chief_view", "chief_consented", "chief_session_id", "chief_workers", "chief_spent", "chief_working"] {
        map.remove(key);
    }
    // Project settings: `claude.json` holds the fields at the top level; accept a nested object too.
    for key in ["worker_model", "careful"] {
        map.remove(key);
    }
    if let Some(Value::Object(claude)) = map.get_mut("claude") {
        for key in ["worker_model", "careful"] {
            claude.remove(key);
        }
    }
}

/// `notify` and `sounds` become per-event rules (#448, #447): on meant a note for both events
/// (an OS notification in the background, a toast in front: `Both`), and `sounds` the default sound.
fn v2_to_v3(map: &mut Map<String, Value>) {
    let notify = map.remove("notify");
    let sounds = map.remove("sounds");
    if (notify.is_none() && sounds.is_none()) || map.contains_key("notifications") {
        return;
    }
    let on = notify == Some(Value::Bool(true));
    let rule = serde_json::json!({
        "delivery": if on { "Both" } else { "Off" },
        "sound": if sounds == Some(Value::Bool(true)) { "Default" } else { "None" },
    });
    map.insert("notifications".into(), serde_json::json!({ "finished": rule, "needs_you": rule }));
}

/// Pure-CLI architecture: Chat tab removed (maps old `active` to `active_project`), and
/// the complex `notifications` rules matrix becomes a simple `background_notifications` bool.
fn v3_to_v4(map: &mut Map<String, Value>) {
    if let Some(active) = map.remove("active") {
        let idx = match active {
            Value::Object(obj) => obj.get("Project").and_then(Value::as_u64),
            _ => map.get("projects").and_then(Value::as_array).filter(|p| !p.is_empty()).map(|_| 0),
        };
        if let Some(i) = idx {
            map.insert("active_project".into(), Value::from(i));
        }
    }
    if let Some(Value::Object(rules)) = map.remove("notifications") {
        let on = ["finished", "needs_you"]
            .iter()
            .any(|k| rules.get(*k).and_then(|r| r.get("delivery")).and_then(Value::as_str).is_some_and(|d| d != "Off"));
        map.entry("background_notifications").or_insert(Value::Bool(on));
    }
}

pub enum Loaded {
    /// Migrated to `CURRENT`, with `"version"` set.
    Ok(Value),
    /// Written by a newer Kynex; this build can't read it safely.
    Newer { found: u32 },
    /// Not a JSON object, or a `"version"` that isn't a whole number.
    Invalid,
}

pub fn migrate(json: &str) -> Loaded {
    let Ok(Value::Object(mut map)) = serde_json::from_str::<Value>(json) else {
        return Loaded::Invalid;
    };
    let version = match map.get("version") {
        None => 0,
        Some(v) => match v.as_u64().and_then(|n| u32::try_from(n).ok()) {
            Some(n) => n,
            None => return Loaded::Invalid,
        },
    };
    if version > CURRENT {
        return Loaded::Newer { found: version };
    }
    for step in &MIGRATIONS[version as usize..CURRENT as usize] {
        step(&mut map);
    }
    map.insert("version".into(), Value::from(CURRENT));
    Loaded::Ok(Value::Object(map))
}

/// Reads and migrates the file at `path`. Returns the value (the default when the
/// file is missing, invalid, or from a newer Kynex) and the newer version found, if
/// any. A newer file is copied to `<path>.json.bak` first.
pub fn read<T: DeserializeOwned + Default>(path: &Path) -> (T, Option<u32>) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return (T::default(), None);
    };
    match migrate(&text) {
        Loaded::Ok(value) => (serde_json::from_value(value).unwrap_or_default(), None),
        Loaded::Newer { found } => {
            let _ = std::fs::copy(path, path.with_extension("json.bak"));
            (T::default(), Some(found))
        }
        Loaded::Invalid => (T::default(), None),
    }
}

/// `value` as pretty JSON with `"version": CURRENT` at the top level.
pub fn stamp<T: Serialize>(value: &T) -> Result<String, String> {
    let mut v = serde_json::to_value(value).map_err(|e| e.to_string())?;
    let Value::Object(map) = &mut v else {
        return Err("only objects can be stamped with a version".into());
    };
    map.insert("version".into(), Value::from(CURRENT));
    serde_json::to_string_pretty(&v).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    /// Shaped like a 0.0.25 `state.json`: no version, real keys.
    const OLD: &str = r#"{
        "projects": [{"path": "C:\\repos\\kynex", "view": "code"}],
        "active": "chat",
        "theme": "dark",
        "zoom": 1.1,
        "sessions_collapsed": false
    }"#;

    #[test]
    fn unversioned_file_migrates_to_current_and_keeps_every_key() {
        let Loaded::Ok(v) = migrate(OLD) else { panic!("expected Ok") };
        assert_eq!(v["version"], CURRENT);
        assert_eq!(v["theme"], "dark");
        assert_eq!(v["zoom"], 1.1);
        assert_eq!(v["sessions_collapsed"], false);
        assert_eq!(v["active_project"], 0);
        assert!(v.get("active").is_none());
    }

    #[test]
    fn current_file_is_left_as_it_is() {
        let text = r#"{"version": 4, "zoom": 1.0}"#;
        let Loaded::Ok(v) = migrate(text) else { panic!("expected Ok") };
        assert_eq!(v, serde_json::json!({"version": 4, "zoom": 1.0}));
    }

    fn migrated(text: &str) -> Value {
        let Loaded::Ok(v) = migrate(text) else { panic!("expected Ok") };
        v
    }

    #[test]
    fn a_state_saved_on_knx_ops_opens_on_the_first_project() {
        let v = migrated(
            r#"{"version": 1, "active": "Chat", "chief_view": true, "projects": [{"path": "a"}, {"path": "b"}]}"#,
        );
        assert_eq!(v["active_project"], 0);
        assert!(v.get("active").is_none());
        assert_eq!(v["version"], CURRENT);
    }

    #[test]
    fn active_project_migration_preserves_index_and_handles_empty() {
        let v = migrated(r#"{"version": 1, "active": "Chat", "chief_view": true, "projects": []}"#);
        assert!(v.get("active_project").is_none());
        let v = migrated(r#"{"version": 1, "active": {"Project": 1}, "chief_view": true, "projects": [1, 2]}"#);
        assert_eq!(v["active_project"], 1);
        let v = migrated(r#"{"version": 1, "active": "Chat", "chief_view": false, "projects": [1]}"#);
        assert_eq!(v["active_project"], 0);
    }

    #[test]
    fn knx_ops_keys_are_dropped() {
        let v = migrated(
            r#"{"version": 1, "zoom": 1.0, "chief_view": false, "chief_consented": true, "chief_session_id": "x",
                "chief_workers": {}, "chief_spent": {}, "chief_working": [],
                "claude": {"model": "m", "worker_model": "haiku", "careful": true}}"#,
        );
        assert_eq!(v, serde_json::json!({"version": 4, "zoom": 1.0, "claude": {"model": "m"}}));
        // A project's settings file keeps them at the top level.
        let v = migrated(r#"{"version": 1, "model": "m", "worker_model": "haiku", "careful": true}"#);
        assert_eq!(v, serde_json::json!({"version": 4, "model": "m"}));
    }

    #[test]
    fn notify_and_rules_become_background_notifications() {
        let v = migrated(r#"{"version": 2, "notify": true, "sounds": true, "zoom": 1.0}"#);
        assert_eq!(v["background_notifications"], true);
        assert!(v.get("notify").is_none() && v.get("sounds").is_none() && v.get("notifications").is_none());
        let v = migrated(r#"{"version": 2, "notify": false, "sounds": true}"#);
        assert_eq!(v["background_notifications"], false);
        let v = migrated(
            r#"{"version": 3, "notifications": {"finished": {"delivery": "Background"}, "needs_you": {"delivery": "Off"}}}"#,
        );
        assert_eq!(v["background_notifications"], true);
        let v = migrated(
            r#"{"version": 3, "notifications": {"finished": {"delivery": "Off"}, "needs_you": {"delivery": "Off"}}}"#,
        );
        assert_eq!(v["background_notifications"], false);
        // Nothing saved, nothing added.
        assert!(migrated(r#"{"version": 3}"#).get("background_notifications").is_none());
    }

    #[test]
    fn newer_version_is_reported_not_read() {
        assert!(matches!(migrate(r#"{"version": 99, "zoom": 1.0}"#), Loaded::Newer { found: 99 }));
    }

    #[test]
    fn non_objects_and_bad_versions_are_invalid() {
        assert!(matches!(migrate("[1, 2]"), Loaded::Invalid));
        assert!(matches!(migrate("\"text\""), Loaded::Invalid));
        assert!(matches!(migrate("not json"), Loaded::Invalid));
        assert!(matches!(migrate(r#"{"version": "one"}"#), Loaded::Invalid));
    }

    #[derive(Deserialize, Serialize, Default, Debug, PartialEq)]
    #[serde(default)]
    struct Sample {
        zoom: f32,
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("kynex-migrate-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn read_keeps_a_newer_file_aside_and_leaves_it_alone() {
        let dir = scratch("newer");
        let path = dir.join("state.json");
        let newer = r#"{"version": 7, "zoom": 1.5}"#;
        std::fs::write(&path, newer).unwrap();

        let (s, found): (Sample, _) = read(&path);
        assert_eq!(s, Sample::default());
        assert_eq!(found, Some(7));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), newer);
        assert_eq!(std::fs::read_to_string(dir.join("state.json.bak")).unwrap(), newer);

        // A later newer file replaces the copy.
        let newer2 = r#"{"version": 8}"#;
        std::fs::write(&path, newer2).unwrap();
        let _ = read::<Sample>(&path);
        assert_eq!(std::fs::read_to_string(dir.join("state.json.bak")).unwrap(), newer2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_migrates_an_old_file_and_reads_missing_as_default() {
        let dir = scratch("old");
        let path = dir.join("state.json");
        std::fs::write(&path, r#"{"zoom": 1.25}"#).unwrap();
        let (s, found): (Sample, _) = read(&path);
        assert_eq!((s.zoom, found), (1.25, None));
        assert!(!dir.join("state.json.bak").exists());

        let (s, found): (Sample, _) = read(&dir.join("missing.json"));
        assert_eq!((s, found), (Sample::default(), None));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stamp_adds_the_version_to_an_object() {
        let text = stamp(&Sample { zoom: 2.0 }).unwrap();
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["version"], CURRENT);
        assert_eq!(v["zoom"], 2.0);
        assert!(stamp(&5u32).is_err());
    }
}
