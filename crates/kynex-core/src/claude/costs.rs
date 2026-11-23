//! Lightweight cumulative USD cost store per Claude Code session ID.
//!
//! Because `claude --resume` resets its per-process `total_cost_usd` counter to
//! `0.0` on each launch, Kynex persists the running total under
//! `<data_dir>/claude-costs/<session_id>.json` using atomic temp-file writes.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// Persisted cumulative spend for one Claude session.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionCost {
    pub total_usd: f64,
    #[serde(default)]
    pub turns: u32,
}

/// Default directory for Claude session cost files (`<data_dir>/claude-costs`).
pub fn dir() -> PathBuf {
    crate::paths::data_dir().join("claude-costs")
}

/// Validates that `session_id` is a safe single-segment identifier (letters,
/// digits, `-`, `_`, 1..=128 chars) so it can never escape the cost directory.
pub fn is_valid_id(session_id: &str) -> bool {
    !session_id.is_empty()
        && session_id.len() <= 128
        && session_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn cost_path(base: &Path, session_id: &str) -> Option<PathBuf> {
    is_valid_id(session_id).then(|| base.join(format!("{session_id}.json")))
}

/// Loads the stored cumulative cost for `session_id` from [`dir`], returning
/// `SessionCost::default()` when missing, invalid, or unreadable.
pub fn load(session_id: &str) -> SessionCost {
    load_in(&dir(), session_id)
}

pub fn load_in(base: &Path, session_id: &str) -> SessionCost {
    let Some(path) = cost_path(base, session_id) else {
        return SessionCost::default();
    };
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str::<SessionCost>(&s).ok())
        .filter(|c| c.total_usd.is_finite() && c.total_usd >= 0.0)
        .unwrap_or_default()
}

/// Atomically persists `cost` for `session_id` under [`dir`].
pub fn save(session_id: &str, cost: SessionCost) -> Result<(), String> {
    save_in(&dir(), session_id, cost)
}

pub fn save_in(base: &Path, session_id: &str, cost: SessionCost) -> Result<(), String> {
    let Some(path) = cost_path(base, session_id) else {
        return Err(format!("invalid session id: {session_id:?}"));
    };
    if !cost.total_usd.is_finite() || cost.total_usd < 0.0 {
        return Err("cost must be a finite non-negative number".into());
    }
    std::fs::create_dir_all(base).map_err(|e| format!("couldn't create {}: {e}", base.display()))?;

    let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp = base.join(format!("{session_id}.json.tmp.{}.{seq}", std::process::id()));
    let json = serde_json::to_string(&cost).map_err(|e| format!("couldn't serialize cost: {e}"))?;
    std::fs::write(&tmp, json).map_err(|e| format!("couldn't write {}: {e}", tmp.display()))?;

    if let Err(first_err) = std::fs::rename(&tmp, &path) {
        // Fallback for filesystems where rename doesn't replace an existing target.
        let _ = std::fs::remove_file(&path);
        if let Err(e) = std::fs::rename(&tmp, &path) {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!("couldn't replace {} ({first_err}): {e}", path.display()));
        }
    }
    Ok(())
}

/// Adds a non-negative `delta_usd` (and increments `turns`) for `session_id`
/// and atomically saves the updated total.
pub fn add_delta(session_id: &str, delta_usd: f64) -> Result<SessionCost, String> {
    add_delta_in(&dir(), session_id, delta_usd)
}

pub fn add_delta_in(base: &Path, session_id: &str, delta_usd: f64) -> Result<SessionCost, String> {
    let mut cur = load_in(base, session_id);
    if delta_usd.is_finite() && delta_usd > 0.0 {
        cur.total_usd += delta_usd;
    }
    cur.turns = cur.turns.saturating_add(1);
    save_in(base, session_id, cur)?;
    Ok(cur)
}

/// Removes any saved cost record for `session_id`.
pub fn delete(session_id: &str) {
    delete_in(&dir(), session_id);
}

pub fn delete_in(base: &Path, session_id: &str) {
    if let Some(path) = cost_path(base, session_id) {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("kynex-claude-costs-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        p
    }

    #[test]
    fn round_trips_and_accumulates_atomically() {
        let dir = temp_dir("accum");
        let sid = "0137a8e3-4603-48d4-aa35-9d22086a1e3c";
        assert_eq!(load_in(&dir, sid), SessionCost::default());

        let c1 = add_delta_in(&dir, sid, 0.125).unwrap();
        assert!((c1.total_usd - 0.125).abs() < 1e-9);
        assert_eq!(c1.turns, 1);

        let c2 = add_delta_in(&dir, sid, 0.375).unwrap();
        assert!((c2.total_usd - 0.5).abs() < 1e-9);
        assert_eq!(c2.turns, 2);

        let loaded = load_in(&dir, sid);
        assert!((loaded.total_usd - 0.5).abs() < 1e-9);
        assert_eq!(loaded.turns, 2);

        delete_in(&dir, sid);
        assert_eq!(load_in(&dir, sid), SessionCost::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_invalid_ids_and_non_finite_costs() {
        let dir = temp_dir("reject");
        for bad in ["", "..", "../escape", "a/b", "a\\b", "has space"] {
            assert!(!is_valid_id(bad), "should reject {bad:?}");
            assert!(save_in(&dir, bad, SessionCost { total_usd: 1.0, turns: 1 }).is_err());
            assert_eq!(load_in(&dir, bad), SessionCost::default());
        }
        assert!(save_in(&dir, "valid-id", SessionCost { total_usd: f64::NAN, turns: 1 }).is_err());
        assert!(save_in(&dir, "valid-id", SessionCost { total_usd: -0.5, turns: 1 }).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
