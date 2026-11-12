use super::*;

#[test]
fn no_two_shortcuts_take_the_same_keys() {
    let all: Vec<(&str, String)> =
        SHORTCUTS.iter().chain(PREFIXED).map(|(n, k)| (*n, k.replace("prefix", DEFAULT_PREFIX))).collect();
    let all: Vec<(&str, &str)> = all.iter().map(|(n, k)| (*n, k.as_str())).collect();
    for mac in [false, true] {
        assert_eq!(kynex_core::shortcuts::clashes(&all, mac), [], "mac: {mac}");
    }
}
