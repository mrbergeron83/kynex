//! The shortcuts list in Settings → Keyboard: names from `keymap.json` in words,
//! and the search over them.

/// A shortcut's name (`next_pane_tab`) in words ("Next pane tab"). `pane_tab_3`
/// reads "Show pane tab 3"; the `_arrow` and `_too` variants say so.
pub fn label(name: &str) -> String {
    let (base, suffix) = if let Some(b) = name.strip_suffix("_arrow") {
        (b, " (arrow)")
    } else if let Some(b) = name.strip_suffix("_too") {
        (b, " (alternate)")
    } else {
        (name, "")
    };
    let words = match base.strip_prefix("pane_tab_") {
        Some(n) if n.parse::<usize>().is_ok() => format!("show pane tab {n}"),
        _ => base.replace('_', " "),
    };
    let mut chars = words.chars();
    let first = chars.next().map(|c| c.to_uppercase().collect::<String>()).unwrap_or_default();
    format!("{first}{}{suffix}", chars.as_str())
}

/// Whether a shortcut matches what was typed: every word of `query` is in its
/// name, its label or its keys, ignoring case. An empty query matches all.
pub fn matches(name: &str, keys: &str, query: &str) -> bool {
    let hay = format!("{} {} {}", name.replace('_', " "), label(name), keys).to_lowercase();
    query.split_whitespace().all(|w| hay.contains(&w.to_lowercase()))
}

/// `ctrl-shift-]` as "Ctrl+Shift+]" ("⌃⇧]" on macOS); a chord's keystrokes are
/// joined by a space. `secondary` counts as the platform key.
pub fn display(keys: &str, mac: bool) -> String {
    use crate::fmt::{self, Mods};
    keys.split_whitespace()
        .map(|stroke| {
            // Modifiers come first, joined to the key by `-`; the key itself may be `-`.
            let mut mods = Mods { control: false, alt: false, shift: false, platform: false };
            let mut rest = stroke;
            while let Some((m, tail)) = rest.split_once('-').filter(|(_, tail)| !tail.is_empty()) {
                match m {
                    "ctrl" | "control" => mods.control = true,
                    "alt" => mods.alt = true,
                    "shift" => mods.shift = true,
                    "cmd" | "super" | "win" => mods.platform = true,
                    _ => break,
                }
                rest = tail;
            }
            fmt::keystroke(mods, rest, mac)
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// One keystroke the same however it was written: modifiers sorted, `secondary` as the
/// OS's (`cmd` on macOS, `ctrl` elsewhere), lower case.
fn normal(stroke: &str, mac: bool) -> String {
    let mut mods = Vec::new();
    let mut rest = stroke;
    while let Some((m, tail)) = rest.split_once('-').filter(|(_, tail)| !tail.is_empty()) {
        mods.push(match m.to_lowercase().as_str() {
            "secondary" if mac => "cmd".to_string(),
            "secondary" | "control" => "ctrl".to_string(),
            "super" | "win" => "cmd".to_string(),
            other => other.to_string(),
        });
        rest = tail;
    }
    mods.sort();
    mods.push(rest.to_lowercase());
    mods.join("-")
}

/// The pairs of shortcuts in `bindings` (name, keys) that would take the same keys on
/// macOS (`mac`) or elsewhere. Unbound ones (empty keys) clash with nothing.
pub fn clashes<'a>(bindings: &[(&'a str, &str)], mac: bool) -> Vec<(&'a str, &'a str)> {
    let keys: Vec<(&str, String)> = bindings
        .iter()
        .filter(|(_, k)| !k.trim().is_empty())
        .map(|(n, k)| (*n, k.split_whitespace().map(|s| normal(s, mac)).collect::<Vec<_>>().join(" ")))
        .collect();
    let mut out = Vec::new();
    for (i, (a, ka)) in keys.iter().enumerate() {
        out.extend(keys[i + 1..].iter().filter(|(_, kb)| kb == ka).map(|(b, _)| (*a, *b)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clashes_are_the_same_keys_however_written() {
        let b = [
            ("a", "secondary-shift-g"),
            ("b", "shift-ctrl-G"),
            ("c", "cmd-shift-g"),
            ("d", ""),
            ("e", ""),
            ("f", "ctrl-b g"),
        ];
        assert_eq!(clashes(&b, false), [("a", "b")]);
        assert_eq!(clashes(&b, true), [("a", "c")]);
        assert!(clashes(&[("x", "ctrl--"), ("y", "ctrl-=")], false).is_empty());
    }

    #[test]
    fn labels_read_as_words() {
        assert_eq!(label("next_pane_tab"), "Next pane tab");
        assert_eq!(label("pane_tab_3"), "Show pane tab 3");
        assert_eq!(label("focus_left_arrow"), "Focus left (arrow)");
        assert_eq!(label("next_session_too"), "Next session (alternate)");
        assert_eq!(label(""), "");
    }

    #[test]
    fn keys_read_as_a_hint() {
        assert_eq!(display("ctrl-shift-]", false), "Ctrl+Shift+]");
        assert_eq!(display("ctrl-b v", false), "Ctrl+B V");
        assert_eq!(display("ctrl--", false), "Ctrl+-");
        assert_eq!(display("escape escape", false), "Esc Esc");
        assert_eq!(display("", false), "");
    }

    #[test]
    fn search_needs_every_word() {
        assert!(matches("split_right", "Ctrl+B V", ""));
        assert!(matches("split_right", "Ctrl+B V", "split"));
        assert!(matches("split_right", "Ctrl+B V", "SPLIT right"));
        assert!(matches("split_right", "Ctrl+B V", "ctrl v"));
        assert!(!matches("split_right", "Ctrl+B V", "split down"));
    }
}
