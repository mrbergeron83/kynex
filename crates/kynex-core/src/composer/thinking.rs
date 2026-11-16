//! Thinking: the on / off switch, and the `ultrathink` keyword.
//!
//! Off is `set_max_thinking_tokens` with 0, on is `null` (the session's default,
//! thinking on for the models that have it). The `alwaysThinkingEnabled` setting isn't
//! read live, and some models (Opus 5.5) think whatever they're told.
//!
//! `ultrathink` is the one word the CLI still reacts to (`think`, `think hard`… no
//! longer do): typed in a prompt it runs that turn at extra-high effort. The CLI says
//! in each `system/init` whether it does on the session's model (`ultrathink_available`,
//! when to colour the word) and whether a host's statement that the user typed it
//! raises the turn (`ultrathink_typed_raises`, when to send `ultrathink_typed`); a
//! `system/per_turn_effort_changed` turns both off until the next init.

use std::ops::Range;

use serde_json::{Value, json};

/// The keyword.
pub const KEYWORD: &str = "ultrathink";

/// The `set_max_thinking_tokens` request that turns thinking `on` or off.
pub fn request(on: bool) -> Value {
    json!({ "subtype": "set_max_thinking_tokens", "max_thinking_tokens": if on { Value::Null } else { json!(0) } })
}

/// What the CLI said about `ultrathink` on the session's model.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Ultrathink {
    /// Colour the word as it's typed.
    pub available: bool,
    /// Messages where the user typed it carry `ultrathink_typed`.
    pub typed_raises: bool,
}

impl Ultrathink {
    /// Follows a `system` message: an init says, a per-turn effort change takes it back.
    pub fn follow(&mut self, v: &Value) {
        match v["subtype"].as_str() {
            Some("init") => {
                self.available = v["ultrathink_available"] == true;
                self.typed_raises = v["ultrathink_typed_raises"] == true;
            }
            Some("per_turn_effort_changed") => *self = Self::default(),
            _ => {}
        }
    }

    /// Whether a message of `text` carries `ultrathink_typed`: the user typed the
    /// word (nothing was `pasted` into it) and the CLI acts on the statement.
    pub fn typed(self, text: &str, pasted: bool) -> bool {
        self.typed_raises && !pasted && !keyword_ranges(text).is_empty()
    }
}

/// Where `ultrathink` stands in `text` as a word of its own, in any case.
pub fn keyword_ranges(text: &str) -> Vec<Range<usize>> {
    let lower = text.to_ascii_lowercase();
    let word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
    lower
        .match_indices(KEYWORD)
        .map(|(i, _)| i..i + KEYWORD.len())
        .filter(|r| !word(text[..r.start].chars().next_back()) && !word(text[r.end..].chars().next()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_word_alone() {
        assert_eq!(keyword_ranges("please ultrathink about it"), vec![7..17]);
        assert_eq!(keyword_ranges("UltraThink, then ultrathink."), vec![0..10, 17..27]);
        assert!(keyword_ranges("ultrathinking ultrathink_x preultrathink").is_empty());
        assert!(keyword_ranges("think hard").is_empty());
    }

    #[test]
    fn init_says_and_an_effort_change_takes_it_back() {
        let mut u = Ultrathink::default();
        u.follow(&json!({ "subtype": "init", "ultrathink_available": true, "ultrathink_typed_raises": true }));
        assert_eq!(u, Ultrathink { available: true, typed_raises: true });
        assert!(u.typed("ultrathink this", false));
        assert!(!u.typed("ultrathink this", true), "pasted text isn't typed");
        assert!(!u.typed("think about this", false));
        u.follow(&json!({ "subtype": "per_turn_effort_changed" }));
        assert_eq!(u, Ultrathink::default());
        u.follow(&json!({ "subtype": "init" }));
        assert!(!u.available, "absent means unknown: nothing is coloured");
    }

    #[test]
    fn thinking_requests() {
        assert_eq!(request(false)["max_thinking_tokens"], 0);
        assert!(request(true)["max_thinking_tokens"].is_null());
    }
}
