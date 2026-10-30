//! Short human-readable times, turn recaps and key shortcuts.

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

/// Time running so far, in whole seconds, for a count that ticks once a
/// second: "8s", "1m 12s", "1h 3m".
pub fn elapsed(d: std::time::Duration) -> String {
    let s = d.as_secs();
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m {}s", s / 60, s % 60),
        _ => format!("{}h {}m", s / 3600, s % 3600 / 60),
    }
}

/// A time limit, in whole units: "45s", "2m", "1m 30s", "1h", "1h 30m".
pub fn limit(ms: u64) -> String {
    let s = ms.div_ceil(1000);
    let (big, small) = match s {
        0..60 => return format!("{s}s"),
        60..3600 => ((s / 60, "m"), (s % 60, "s")),
        _ => ((s / 3600, "h"), (s % 3600 / 60, "m")),
    };
    match small.0 {
        0 => format!("{}{}", big.0, big.1),
        n => format!("{}{} {n}{}", big.0, big.1, small.1),
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
/// `files` are paths; the recap names them.
pub fn recap(duration_ms: u64, tools: usize, files: &[String], cost: f64) -> String {
    let mut parts = vec![duration(duration_ms)];
    if tools > 0 {
        parts.push(format!("{tools} tool call{}", if tools == 1 { "" } else { "s" }));
    }
    let names: Vec<&str> = files.iter().map(|f| file_name(f)).collect();
    match names.len() {
        0 => {}
        1..=3 => parts.push(format!("edited {}", names.join(", "))),
        n => parts.push(format!("edited {}, {} and {} more", names[0], names[1], n - 2)),
    }
    parts.push(format!("${cost:.2}"));
    parts.join(" · ")
}

/// The last part of a path, with either separator: `C:\x\main.rs` → `main.rs`.
pub fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).find(|n| !n.is_empty()).unwrap_or(path)
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

/// A size on disk, short: "512 B", "95 MB", "2.3 GB" (1 KB = 1024 bytes).
pub fn bytes(n: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if n < 1024 {
        return format!("{n} B");
    }
    let mut v = n as f64 / 1024.;
    let mut unit = 0;
    while v >= 1024. && unit + 1 < UNITS.len() {
        v /= 1024.;
        unit += 1;
    }
    let s = if v < 10. { format!("{v:.1}") } else { format!("{v:.0}") };
    format!("{} {}", s.trim_end_matches(".0"), UNITS[unit])
}

/// The first `n` words of a message, on one line, with "…" when there's more:
/// a stand-in title until a session has its own name.
pub fn first_words(text: &str, n: usize) -> String {
    let mut words = text.split_whitespace();
    let mut s = words.by_ref().take(n).collect::<Vec<_>>().join(" ");
    if words.next().is_some() {
        s.push('…');
    }
    s
}

/// Modifier keys held in a shortcut; `platform` is Cmd on macOS.
#[derive(Clone, Copy, Default)]
pub struct Mods {
    pub control: bool,
    pub alt: bool,
    pub shift: bool,
    pub platform: bool,
}

/// One keystroke as a shortcut hint: "⇧⌘T" on macOS (Apple's order), else
/// "Ctrl+Shift+T". `key` is gpui's name for it (`t`, `escape`, `` ` ``).
pub fn keystroke(mods: Mods, key: &str, mac: bool) -> String {
    let key = match key {
        "escape" => "Esc".to_string(),
        "enter" => if mac { "↩" } else { "Enter" }.to_string(),
        "tab" => if mac { "⇥" } else { "Tab" }.to_string(),
        "backspace" => if mac { "⌫" } else { "Backspace" }.to_string(),
        "up" => "↑".to_string(),
        "down" => "↓".to_string(),
        "left" => "←".to_string(),
        "right" => "→".to_string(),
        "pageup" => "Page Up".to_string(),
        "pagedown" => "Page Down".to_string(),
        "space" => "Space".to_string(),
        k => {
            let mut c = k.chars();
            c.next().map_or(String::new(), |f| f.to_uppercase().chain(c).collect())
        }
    };
    if mac {
        let held = [(mods.control, "⌃"), (mods.alt, "⌥"), (mods.shift, "⇧"), (mods.platform, "⌘")];
        let mut s: String = held.iter().filter(|h| h.0).map(|h| h.1).collect();
        s.push_str(&key);
        s
    } else {
        let held = [(mods.control, "Ctrl"), (mods.alt, "Alt"), (mods.shift, "Shift"), (mods.platform, "Super")];
        let mut parts: Vec<&str> = held.iter().filter(|h| h.0).map(|h| h.1).collect();
        parts.push(&key);
        parts.join("+")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_words_make_a_short_title() {
        assert_eq!(first_words("Fix the\nscrolling  bug", 8), "Fix the scrolling bug");
        assert_eq!(first_words("one two three four five", 3), "one two three…");
        assert_eq!(first_words("   ", 8), "");
    }

    #[test]
    fn shortcuts_read_per_platform() {
        let ctrl_shift = Mods { control: true, shift: true, ..Mods::default() };
        assert_eq!(keystroke(ctrl_shift, "t", false), "Ctrl+Shift+T");
        let cmd_shift = Mods { platform: true, shift: true, ..Mods::default() };
        assert_eq!(keystroke(cmd_shift, "t", true), "⇧⌘T");
        assert_eq!(keystroke(Mods { control: true, ..Mods::default() }, "`", false), "Ctrl+`");
        assert_eq!(keystroke(Mods::default(), "escape", true), "Esc");
        assert_eq!(keystroke(Mods { shift: true, ..Mods::default() }, "tab", false), "Shift+Tab");
        assert_eq!(keystroke(Mods::default(), "f5", false), "F5");
    }

    #[test]
    fn durations() {
        assert_eq!(duration(8_200), "8.2s");
        assert_eq!(duration(252_000), "4m 12s");
        assert_eq!(duration(3_780_000), "1h 3m");
        let secs = std::time::Duration::from_secs;
        assert_eq!(elapsed(std::time::Duration::from_millis(8_900)), "8s");
        assert_eq!(elapsed(secs(72)), "1m 12s");
        assert_eq!(elapsed(secs(3_780)), "1h 3m");
    }

    #[test]
    fn limits() {
        assert_eq!(limit(45_000), "45s");
        assert_eq!(limit(500), "1s");
        assert_eq!(limit(120_000), "2m");
        assert_eq!(limit(90_000), "1m 30s");
        assert_eq!(limit(600_000), "10m");
        assert_eq!(limit(3_600_000), "1h");
        assert_eq!(limit(5_400_000), "1h 30m");
    }

    #[test]
    fn byte_sizes() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(1023), "1023 B");
        assert_eq!(bytes(1024), "1 KB");
        assert_eq!(bytes(95 * 1024 * 1024), "95 MB");
        assert_eq!(bytes(2_469_606_195), "2.3 GB");
        assert_eq!(bytes(u64::MAX), "16777216 TB");
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
        assert_eq!(recap(1_000, 1, &["C:\\w\\a.rs".into()], 0.5), "1.0s · 1 tool call · edited a.rs · $0.50");
        assert_eq!(file_name("/w/src/"), "src");
        let files: Vec<String> = ["a", "b", "c", "d", "e"].map(String::from).into();
        assert_eq!(recap(1_000, 5, &files, 0.0), "1.0s · 5 tool calls · edited a, b and 3 more · $0.00");
    }
}
