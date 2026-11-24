use super::*;

#[test]
fn long_paste_threshold_matches_lines_and_chars() {
    assert!(!is_long_paste("hello\nworld"));
    assert!(!is_long_paste(&"a\n".repeat(9)));
    assert!(is_long_paste(&"a\n".repeat(10)));
    assert!(!is_long_paste(&"x".repeat(999)));
    assert!(is_long_paste(&"x".repeat(1_000)));
    assert!(is_long_paste(&"é".repeat(1_000)));
    assert!(!is_long_paste(&"é".repeat(999)));
}
