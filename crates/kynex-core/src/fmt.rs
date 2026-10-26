//! Short human-readable times and turn recaps.

use std::time::SystemTime;

/// "8.2s", "4m 12s", "1h 3m".
pub fn duration(ms: u64) -> String {
    let s = ms / 1000;
    match s {
        0..60 => format!("{:.1}s", ms as f64 / 1000.0),
        60..3600 => format!("{}m {}s", s / 60, s % 60),
        _ => format!("{}h {}m", s / 3600, s % 3600 / 60),
    }
}

/// "just now", "5m ago", "3h ago", "2d ago".
pub fn ago(t: SystemTime) -> String {
    let secs = t.elapsed().map_or(0, |d| d.as_secs());
    match secs {
        0..60 => "just now".into(),
        60..3600 => format!("{}m ago", secs / 60),
        3600..86400 => format!("{}h ago", secs / 3600),
        _ => format!("{}d ago", secs / 86400),
    }
}

/// [`ago`] for a Unix timestamp in seconds, as git reports them.
pub fn ago_unix(secs: i64) -> String {
    ago(std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs.max(0) as u64))
}

/// The one-line recap after a turn: "12.3s · 4 tool calls · edited a.rs, b.rs · $0.05".
pub fn recap(duration_ms: u64, tools: usize, files: &[String], cost: f64) -> String {
    let mut parts = vec![duration(duration_ms)];
    if tools > 0 {
        parts.push(format!("{tools} tool call{}", if tools == 1 { "" } else { "s" }));
    }
    match files.len() {
        0 => {}
        1..=3 => parts.push(format!("edited {}", files.join(", "))),
        n => parts.push(format!("edited {}, {} and {} more", files[0], files[1], n - 2)),
    }
    parts.push(format!("${cost:.2}"));
    parts.join(" · ")
}

/// A token count, short: "950", "36k", "1.2M".
pub fn tokens(n: u64) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..1_000_000 => format!("{}k", (n + 500) / 1_000),
        _ => {
            let m = format!("{:.1}", n as f64 / 1_000_000.0);
            format!("{}M", m.trim_end_matches(".0"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(duration(8_200), "8.2s");
        assert_eq!(duration(252_000), "4m 12s");
        assert_eq!(duration(3_780_000), "1h 3m");
    }

    #[test]
    fn token_counts() {
        assert_eq!(tokens(950), "950");
        assert_eq!(tokens(36_400), "36k");
        assert_eq!(tokens(200_000), "200k");
        assert_eq!(tokens(1_000_000), "1M");
        assert_eq!(tokens(1_240_000), "1.2M");
    }

    #[test]
    fn recaps() {
        assert_eq!(recap(1_000, 0, &[], 0.0), "1.0s · $0.00");
        assert_eq!(recap(1_000, 1, &["a.rs".into()], 0.5), "1.0s · 1 tool call · edited a.rs · $0.50");
        let files: Vec<String> = ["a", "b", "c", "d", "e"].map(String::from).into();
        assert_eq!(recap(1_000, 5, &files, 0.0), "1.0s · 5 tool calls · edited a, b and 3 more · $0.00");
    }
}
