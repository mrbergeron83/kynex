//! What you've sent in a project, for the Claude chat composer's keys: ↑ / ↓
//! walk through earlier messages, Ctrl+R searches them, and Ctrl+S stashes the
//! draft. Unsent drafts are kept per session, so they survive a restart. Saved
//! with the project (`history.json`).

use std::path::Path;

use serde::{Deserialize, Serialize};

/// How many messages a project keeps.
const MAX: usize = 200;

/// How many sessions' unsent drafts a project keeps.
const MAX_DRAFTS: usize = 50;

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct History {
    /// Oldest first; a message sent again moves to the end.
    entries: Vec<String>,
    /// The draft Ctrl+S put aside.
    stash: Option<String>,
    /// Unsent drafts by CLI session id, oldest first.
    drafts: Vec<(String, String)>,
    /// While walking with ↑ / ↓: which entry is shown, and the draft to come
    /// back to past the newest.
    #[serde(skip)]
    at: Option<usize>,
    #[serde(skip)]
    draft: String,
}

/// What Ctrl+S did.
#[derive(Debug, PartialEq, Eq)]
pub enum Stash {
    /// The draft was put aside; clear the composer.
    Stashed,
    /// The composer was empty: here's the stashed draft back.
    Restored(String),
    /// Nothing to stash or restore.
    Nothing,
}

impl History {
    const FILE: &str = "history.json";

    /// Blocks on a small file read.
    pub fn load(project: &Path) -> Self {
        crate::project::load(project, Self::FILE)
    }

    /// Blocks on a small file write.
    pub fn save(&self, project: &Path) -> Result<(), String> {
        crate::project::save(project, Self::FILE, self)
    }

    /// Remembers a sent message, and ends any walk through history.
    pub fn push(&mut self, text: &str) {
        self.at = None;
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        self.entries.retain(|e| e != text);
        self.entries.push(text.to_string());
        let over = self.entries.len().saturating_sub(MAX);
        self.entries.drain(..over);
    }

    /// ↑: the message before the one shown. The first step keeps `current` to
    /// come back to. `None` at the oldest (or with no history).
    pub fn older(&mut self, current: &str) -> Option<&str> {
        let at = match self.at {
            None if self.entries.is_empty() => return None,
            None => {
                self.draft = current.to_string();
                self.entries.len() - 1
            }
            Some(0) => return None,
            Some(i) => i - 1,
        };
        self.at = Some(at);
        Some(&self.entries[at])
    }

    /// ↓: the message after the one shown, then the draft from before the
    /// walk. `None` when not walking.
    pub fn newer(&mut self) -> Option<String> {
        let at = self.at?;
        if at + 1 < self.entries.len() {
            self.at = Some(at + 1);
            Some(self.entries[at + 1].clone())
        } else {
            self.at = None;
            Some(std::mem::take(&mut self.draft))
        }
    }

    /// Ctrl+R: messages containing `query` (any case), newest first.
    pub fn search(&self, query: &str) -> Vec<&str> {
        let q = query.to_lowercase();
        self.entries.iter().rev().filter(|e| e.to_lowercase().contains(&q)).map(String::as_str).collect()
    }

    /// Ctrl+S: puts a draft aside, or brings the stashed one back into an empty
    /// composer. Stashing over an older stash keeps that one in history.
    pub fn toggle_stash(&mut self, current: &str) -> Stash {
        if !current.trim().is_empty() {
            if let Some(old) = self.stash.replace(current.to_string()) {
                self.push(&old);
            }
            return Stash::Stashed;
        }
        match self.stash.take() {
            Some(s) => Stash::Restored(s),
            None => Stash::Nothing,
        }
    }

    pub fn has_stash(&self) -> bool {
        self.stash.is_some()
    }

    /// The unsent draft left in session `session`.
    pub fn draft(&self, session: &str) -> Option<&str> {
        self.drafts.iter().find(|(s, _)| s == session).map(|(_, d)| d.as_str())
    }

    /// Keeps `text` as session `session`'s draft (an empty one drops it).
    /// Returns whether anything changed, i.e. whether to save.
    pub fn set_draft(&mut self, session: &str, text: &str) -> bool {
        let before = self.draft(session).map(str::to_string);
        self.drafts.retain(|(s, _)| s != session);
        if !text.trim().is_empty() {
            self.drafts.push((session.to_string(), text.to_string()));
            let over = self.drafts.len().saturating_sub(MAX_DRAFTS);
            self.drafts.drain(..over);
        }
        before.as_deref() != Some(text).filter(|t| !t.trim().is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn history(entries: &[&str]) -> History {
        let mut h = History::default();
        for e in entries {
            h.push(e);
        }
        h
    }

    #[test]
    fn walks_back_and_returns_to_the_draft() {
        let mut h = history(&["one", "two", "three"]);
        assert_eq!(h.newer(), None, "not walking yet");
        assert_eq!(h.older("half-typed"), Some("three"));
        assert_eq!(h.older("three"), Some("two"));
        assert_eq!(h.older("two"), Some("one"));
        assert_eq!(h.older("one"), None, "the oldest");
        assert_eq!(h.newer().as_deref(), Some("two"));
        assert_eq!(h.newer().as_deref(), Some("three"));
        assert_eq!(h.newer().as_deref(), Some("half-typed"));
        assert_eq!(h.newer(), None);
        assert_eq!(History::default().older("x"), None);
    }

    #[test]
    fn sending_again_moves_to_the_end_and_ends_the_walk() {
        let mut h = history(&["one", "two", "  ", "one"]);
        assert_eq!(h.entries, ["two", "one"]);
        h.older("");
        h.push("three");
        assert_eq!(h.older("").unwrap(), "three", "the walk starts over");
        let mut big = History::default();
        for i in 0..MAX + 5 {
            big.push(&i.to_string());
        }
        assert_eq!(big.entries.len(), MAX);
        assert_eq!(big.entries[0], "5");
    }

    #[test]
    fn searches_newest_first_in_any_case() {
        let h = history(&["Fix the build", "add a test", "fix lints"]);
        assert_eq!(h.search("FIX"), ["fix lints", "Fix the build"]);
        assert_eq!(h.search("").len(), 3);
        assert!(h.search("zzz").is_empty());
    }

    #[test]
    fn stash_puts_aside_and_brings_back() {
        let mut h = History::default();
        assert_eq!(h.toggle_stash(""), Stash::Nothing);
        assert_eq!(h.toggle_stash("draft one"), Stash::Stashed);
        assert!(h.has_stash());
        assert_eq!(h.toggle_stash("draft two"), Stash::Stashed);
        assert_eq!(h.search("draft"), ["draft one"], "the replaced stash went to history");
        assert_eq!(h.toggle_stash("  "), Stash::Restored("draft two".into()));
        assert!(!h.has_stash());
    }

    #[test]
    fn drafts_are_kept_per_session() {
        let mut h = History::default();
        assert!(h.set_draft("s1", "half a thought"));
        assert!(!h.set_draft("s1", "half a thought"), "unchanged");
        assert!(h.set_draft("s2", "other"));
        assert_eq!(h.draft("s1"), Some("half a thought"));
        assert!(h.set_draft("s1", "  "), "emptied");
        assert_eq!(h.draft("s1"), None);
        assert!(!h.set_draft("s1", ""));
        for i in 0..MAX_DRAFTS + 3 {
            h.set_draft(&i.to_string(), "x");
        }
        assert_eq!(h.drafts.len(), MAX_DRAFTS);
        assert_eq!(h.draft("s2"), None, "the oldest went first");
    }

    #[test]
    fn round_trips_without_the_walk() {
        let mut h = history(&["one"]);
        h.toggle_stash("later");
        h.older("typing");
        let back: History = serde_json::from_str(&serde_json::to_string(&h).unwrap()).unwrap();
        assert_eq!(back.entries, ["one"]);
        assert_eq!(back.stash.as_deref(), Some("later"));
        assert_eq!(back.at, None);
    }
}
