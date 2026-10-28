//! The account's usage limits: the rolling 5-hour window and the weekly cap.
//! The CLI reports both in a stream-json `rate_limit_event` on every turn, for
//! every session, so the newest report wins; it's kept in `usage.json` so the
//! title bar has something to show before the first turn after a start.
//! API-key accounts have no such limits and get no reports, so nothing shows.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// From this much use on, a window shows as close to its limit.
pub const NEAR: f64 = 0.8;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Limits {
    pub five_hour: Option<Window>,
    pub seven_day: Option<Window>,
    /// Paying for use past the limits.
    #[serde(default)]
    pub overage: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Window {
    /// How much of it is used, 0 to 1.
    pub used: f64,
    /// When it starts over, in Unix seconds.
    pub resets_at: u64,
    /// The limit was hit: no more turns until it resets.
    pub limited: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Ok,
    Near,
    Limited,
}

impl Window {
    /// The window as of `now`: once its reset time passes, it's empty again.
    pub fn at(self, now: u64) -> Self {
        if now >= self.resets_at { Window { used: 0.0, resets_at: self.resets_at, limited: false } } else { self }
    }

    pub fn level(self) -> Level {
        if self.limited || self.used >= 1.0 {
            Level::Limited
        } else if self.used >= NEAR {
            Level::Near
        } else {
            Level::Ok
        }
    }

    /// "42%".
    pub fn percent(self) -> String {
        format!("{:.0}%", (self.used * 100.0).clamp(0.0, 100.0))
    }
}

/// The limits in a `rate_limit_event`, or `None` if it's some other event or
/// carries neither window.
pub fn parse(v: &Value) -> Option<Limits> {
    if v["type"] != "rate_limit_event" {
        return None;
    }
    let info = &v["rate_limit_info"];
    let windows = &info["unifiedWindows"];
    let window = |key: &str| {
        let w = &windows[key];
        Some(Window {
            used: w["utilization"].as_f64()?.max(0.0),
            resets_at: w["resetsAt"].as_u64()?,
            limited: w["status"] == "rejected",
        })
    };
    let limits = Limits {
        five_hour: window("five_hour"),
        seven_day: window("seven_day"),
        overage: info["isUsingOverage"].as_bool().unwrap_or(false),
    };
    (limits.five_hour.is_some() || limits.seven_day.is_some()).then_some(limits)
}

impl Limits {
    /// Both windows as of `now`.
    pub fn at(&self, now: u64) -> Self {
        Limits {
            five_hour: self.five_hour.map(|w| w.at(now)),
            seven_day: self.seven_day.map(|w| w.at(now)),
            overage: self.overage,
        }
    }

    /// One line per window, for the tooltip: "5-hour: 42% used, resets in 2h 10m".
    pub fn describe(&self, now: u64) -> String {
        let line = |name: &str, w: Window| {
            let state =
                if w.level() == Level::Limited { "limit reached".to_string() } else { format!("{} used", w.percent()) };
            format!("{name}: {state}, resets in {}", until(w.resets_at.saturating_sub(now)))
        };
        let mut lines: Vec<String> = Vec::new();
        lines.extend(self.five_hour.map(|w| line("5-hour", w)));
        lines.extend(self.seven_day.map(|w| line("Weekly", w)));
        if self.overage {
            lines.push("Using extra usage".into());
        }
        lines.join("\n")
    }

    /// The last limits seen, from a previous run.
    pub fn load() -> Option<Self> {
        serde_json::from_str(&std::fs::read_to_string(file()).ok()?).ok()
    }

    /// Keeps them for the next start. Blocks on the file write.
    pub fn save(&self) {
        if let Ok(json) = serde_json::to_string(self) {
            let _ = std::fs::create_dir_all(crate::paths::data_dir());
            let _ = std::fs::write(file(), json);
        }
    }
}

fn file() -> PathBuf {
    crate::paths::data_dir().join("usage.json")
}

/// Unix seconds now.
pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// A wait in its two largest units: "45s", "12m", "2h 10m", "3d 4h".
pub fn until(secs: u64) -> String {
    let (d, h, m) = (secs / 86_400, secs % 86_400 / 3600, secs % 3600 / 60);
    match (d, h, m) {
        (0, 0, 0) => format!("{secs}s"),
        (0, 0, m) => format!("{m}m"),
        (0, h, 0) => format!("{h}h"),
        (0, h, m) => format!("{h}h {m}m"),
        (d, 0, _) => format!("{d}d"),
        (d, h, _) => format!("{d}d {h}h"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn event(five: f64, status: &str) -> Value {
        json!({ "type": "rate_limit_event", "rate_limit_info": {
            "status": "allowed", "resetsAt": 1000, "rateLimitType": "five_hour", "isUsingOverage": false,
            "unifiedWindows": {
                "five_hour": { "utilization": five, "resetsAt": 1000, "status": status },
                "seven_day": { "utilization": 0.53, "resetsAt": 90_000, "status": "allowed" }
            } } })
    }

    #[test]
    fn parses_both_windows() {
        let l = parse(&event(0.12, "allowed")).unwrap();
        assert_eq!(l.five_hour, Some(Window { used: 0.12, resets_at: 1000, limited: false }));
        assert_eq!(l.seven_day.unwrap().percent(), "53%");
        assert!(!l.overage);
        assert!(parse(&json!({ "type": "result" })).is_none());
        assert!(parse(&json!({ "type": "rate_limit_event", "rate_limit_info": {} })).is_none());
    }

    #[test]
    fn levels() {
        let l = parse(&event(0.85, "allowed_warning")).unwrap();
        assert_eq!(l.five_hour.unwrap().level(), Level::Near);
        assert_eq!(l.seven_day.unwrap().level(), Level::Ok);
        let l = parse(&event(0.2, "rejected")).unwrap();
        assert_eq!(l.five_hour.unwrap().level(), Level::Limited, "rejected wins over the number");
    }

    #[test]
    fn windows_empty_once_they_reset() {
        let l = parse(&event(0.9, "rejected")).unwrap().at(1000);
        assert_eq!(l.five_hour.unwrap().level(), Level::Ok);
        assert_eq!(l.five_hour.unwrap().used, 0.0);
        assert_eq!(l.seven_day.unwrap().used, 0.53);
    }

    #[test]
    fn describes_each_window() {
        let l = parse(&event(0.42, "allowed")).unwrap();
        assert_eq!(l.describe(0), "5-hour: 42% used, resets in 16m\nWeekly: 53% used, resets in 1d 1h");
        let l = Limits { overage: true, ..parse(&event(1.0, "rejected")).unwrap() };
        assert!(l.describe(0).starts_with("5-hour: limit reached"));
        assert!(l.describe(0).ends_with("Using extra usage"));
    }

    #[test]
    fn waits() {
        assert_eq!(until(45), "45s");
        assert_eq!(until(12 * 60 + 5), "12m");
        assert_eq!(until(7200), "2h");
        assert_eq!(until(3 * 86_400 + 4 * 3600 + 59), "3d 4h");
        assert_eq!(until(86_400), "1d");
    }

    #[test]
    fn round_trips_through_json() {
        let l = parse(&event(0.5, "allowed")).unwrap();
        let back: Limits = serde_json::from_str(&serde_json::to_string(&l).unwrap()).unwrap();
        assert_eq!(back, l);
    }
}
