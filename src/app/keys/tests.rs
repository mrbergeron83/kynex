use super::*;

/// The defaults are picked per OS when Kynex is built, so each OS checks its own (CI runs
/// this on Windows, macOS and Linux).
#[test]
fn no_two_shortcuts_take_the_same_keys() {
    let all: Vec<(&str, String)> =
        SHORTCUTS.iter().chain(PREFIXED).map(|(n, k)| (*n, k.replace("prefix", DEFAULT_PREFIX))).collect();
    let all: Vec<(&str, &str)> = all.iter().map(|(n, k)| (*n, k.as_str())).collect();
    assert_eq!(kynex_core::shortcuts::clashes(&all, cfg!(target_os = "macos")), []);
}
