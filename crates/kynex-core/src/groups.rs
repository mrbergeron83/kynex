//! How a folder's sessions are organised in the sessions panel: named groups
//! that collapse, an archive that hides sessions without deleting them,
//! sessions pinned above the rest, and names given by hand.
//! Kept per project as `sessions.json` in its settings folder (see [`crate::project`]).

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionGroups {
    /// In the order they're shown.
    pub groups: Vec<Group>,
    /// Session ids hidden from the list until unarchived.
    pub archived: Vec<String>,
    /// Date headers ([`crate::dates::Bucket::key`]) opened or folded away
    /// from how they start.
    pub dates_toggled: Vec<String>,
    /// Session ids shown above everything else.
    pub pinned: Vec<String>,
    /// Names given by hand, by session id; they win over generated ones.
    pub names: BTreeMap<String, String>,
}

/// The longest name a session can be given, in characters.
const MAX_NAME: usize = 80;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Group {
    pub name: String,
    /// Session ids; the panel shows them newest first whatever their order here.
    pub sessions: Vec<String>,
    pub collapsed: bool,
}

/// A folder's sessions sorted into where the panel shows them, each list in
/// the order it was given (newest first).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Arranged {
    /// Above everything, whatever group they're in.
    pub pinned: Vec<String>,
    pub top: Vec<String>,
    /// One list per group, in [`SessionGroups::groups`] order.
    pub groups: Vec<Vec<String>>,
    pub archived: Vec<String>,
}

impl SessionGroups {
    const FILE: &str = "sessions.json";

    pub fn load(project: &Path) -> Self {
        crate::project::load(project, Self::FILE)
    }

    pub fn save(&self, project: &Path) -> Result<(), String> {
        crate::project::save(project, Self::FILE, self)
    }

    /// The group a session is in.
    pub fn group_of(&self, id: &str) -> Option<usize> {
        self.groups.iter().position(|g| g.sessions.iter().any(|s| s == id))
    }

    pub fn is_archived(&self, id: &str) -> bool {
        self.archived.iter().any(|s| s == id)
    }

    /// Whether a date header is open: recent ones start open, older ones folded.
    pub fn date_open(&self, bucket: crate::dates::Bucket) -> bool {
        bucket.open_by_default() != self.dates_toggled.contains(&bucket.key())
    }

    /// Folds an open date header, or opens a folded one.
    pub fn toggle_date(&mut self, bucket: crate::dates::Bucket) {
        let key = bucket.key();
        match self.dates_toggled.iter().position(|k| *k == key) {
            Some(i) => {
                self.dates_toggled.remove(i);
            }
            None => self.dates_toggled.push(key),
        }
    }

    pub fn is_pinned(&self, id: &str) -> bool {
        self.pinned.iter().any(|s| s == id)
    }

    /// Pins a session above the rest, or unpins it. A pinned session isn't archived.
    pub fn toggle_pin(&mut self, id: &str) {
        if self.is_pinned(id) {
            self.pinned.retain(|s| s != id);
        } else {
            self.unarchive(id);
            self.pinned.push(id.to_string());
        }
    }

    /// The name given to a session by hand, if any.
    pub fn name_of(&self, id: &str) -> Option<&str> {
        self.names.get(id).map(String::as_str)
    }

    /// Names a session by hand: one line, trimmed, at most [`MAX_NAME`]
    /// characters. A blank name drops it, back to the generated one.
    pub fn rename(&mut self, id: &str, name: &str) {
        let name: String = name.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(MAX_NAME).collect();
        if name.is_empty() {
            self.names.remove(id);
        } else {
            self.names.insert(id.to_string(), name);
        }
    }

    /// Hides a session; it leaves its group (and the pinned ones), and comes back at the top level.
    pub fn archive(&mut self, id: &str) {
        self.leave_group(id);
        self.pinned.retain(|s| s != id);
        if !self.is_archived(id) {
            self.archived.push(id.to_string());
        }
    }

    /// Shows an archived session again. Returns whether it was archived.
    pub fn unarchive(&mut self, id: &str) -> bool {
        let before = self.archived.len();
        self.archived.retain(|s| s != id);
        self.archived.len() != before
    }

    /// Puts a session in group `to`, or at the top level with `None`.
    /// A session moved anywhere is no longer archived, or pinned.
    pub fn move_to(&mut self, id: &str, to: Option<usize>) {
        self.unarchive(id);
        self.pinned.retain(|s| s != id);
        self.leave_group(id);
        if let Some(g) = to.and_then(|i| self.groups.get_mut(i)) {
            g.sessions.push(id.to_string());
        }
    }

    fn leave_group(&mut self, id: &str) {
        for g in &mut self.groups {
            g.sessions.retain(|s| s != id);
        }
    }

    /// Adds an empty group at the end, named `name` or "New group" when blank,
    /// numbered if that's taken. Returns its index.
    pub fn add_group(&mut self, name: &str) -> usize {
        let base = if name.trim().is_empty() { "New group" } else { name.trim() };
        let mut name = base.to_string();
        let mut n = 2;
        while self.groups.iter().any(|g| g.name == name) {
            name = format!("{base} {n}");
            n += 1;
        }
        self.groups.push(Group { name, ..Group::default() });
        self.groups.len() - 1
    }

    /// Renames a group; a blank name keeps the old one.
    pub fn rename_group(&mut self, i: usize, name: &str) {
        if let Some(g) = self.groups.get_mut(i).filter(|_| !name.trim().is_empty()) {
            g.name = name.trim().to_string();
        }
    }

    /// Deletes a group; its sessions go back to the top level.
    pub fn delete_group(&mut self, i: usize) {
        if i < self.groups.len() {
            self.groups.remove(i);
        }
    }

    pub fn toggle_group(&mut self, i: usize) {
        if let Some(g) = self.groups.get_mut(i) {
            g.collapsed = !g.collapsed;
        }
    }

    /// Drops a session that's gone (its transcript deleted).
    pub fn forget(&mut self, id: &str) {
        self.leave_group(id);
        self.unarchive(id);
        self.pinned.retain(|s| s != id);
        self.names.remove(id);
    }

    /// Sorts session ids (newest first) into the pinned ones, the top level,
    /// groups and archive. Ids saved here that aren't in `ids` are left out.
    pub fn arrange<'a>(&self, ids: impl IntoIterator<Item = &'a str>) -> Arranged {
        let mut out = Arranged { groups: vec![Vec::new(); self.groups.len()], ..Arranged::default() };
        for id in ids {
            let list = if self.is_archived(id) {
                &mut out.archived
            } else if self.is_pinned(id) {
                &mut out.pinned
            } else if let Some(g) = self.group_of(id) {
                &mut out.groups[g]
            } else {
                &mut out.top
            };
            list.push(id.to_string());
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn date_headers_fold_and_remember() {
        use crate::dates::Bucket;
        let mut g = SessionGroups::default();
        assert!(g.date_open(Bucket::Today) && !g.date_open(Bucket::MonthOf(2026, 3)));
        g.toggle_date(Bucket::Today);
        g.toggle_date(Bucket::MonthOf(2026, 3));
        assert!(!g.date_open(Bucket::Today) && g.date_open(Bucket::MonthOf(2026, 3)));
        g.toggle_date(Bucket::Today);
        assert!(g.date_open(Bucket::Today));
        // Older files without the field still load.
        let old: SessionGroups = serde_json::from_str(r#"{"groups":[],"archived":[]}"#).unwrap();
        assert!(old.dates_toggled.is_empty());
    }

    #[test]
    fn arranges_newest_first_into_top_groups_and_archive() {
        let mut g = SessionGroups::default();
        let work = g.add_group("Work");
        g.move_to("b", Some(work));
        g.move_to("d", Some(work));
        g.archive("c");
        let a = g.arrange(["a", "b", "c", "d", "e"]);
        assert_eq!(a.top, ["a", "e"]);
        assert_eq!(a.groups, [vec!["b", "d"]]);
        assert_eq!(a.archived, ["c"]);
        // Ids that no longer exist are left out.
        assert_eq!(g.arrange(["a"]).groups, [Vec::<String>::new()]);
    }

    #[test]
    fn a_session_is_in_one_place() {
        let mut g = SessionGroups::default();
        let (x, y) = (g.add_group("X"), g.add_group("Y"));
        g.move_to("s", Some(x));
        g.move_to("s", Some(y));
        assert_eq!(g.group_of("s"), Some(y));
        assert!(g.groups[x].sessions.is_empty());
        // Archiving takes it out of its group; moving it brings it back.
        g.archive("s");
        assert_eq!(g.group_of("s"), None);
        assert!(g.is_archived("s"));
        g.move_to("s", Some(x));
        assert!(!g.is_archived("s"));
        assert_eq!(g.group_of("s"), Some(x));
        g.move_to("s", None);
        assert_eq!(g.group_of("s"), None);
    }

    #[test]
    fn deleting_a_group_keeps_its_sessions() {
        let mut g = SessionGroups::default();
        let i = g.add_group("Old");
        g.move_to("s", Some(i));
        g.delete_group(i);
        assert!(g.groups.is_empty());
        assert_eq!(g.arrange(["s"]).top, ["s"]);
    }

    #[test]
    fn group_names() {
        let mut g = SessionGroups::default();
        g.add_group("");
        g.add_group(" ");
        g.add_group("  Bugs ");
        let names: Vec<_> = g.groups.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(names, ["New group", "New group 2", "Bugs"]);
        g.rename_group(2, "  ");
        assert_eq!(g.groups[2].name, "Bugs");
        g.rename_group(2, "Fixes");
        assert_eq!(g.groups[2].name, "Fixes");
        g.toggle_group(2);
        assert!(g.groups[2].collapsed);
    }

    #[test]
    fn forgets_deleted_sessions_and_unarchives() {
        let mut g = SessionGroups::default();
        g.archive("a");
        assert!(g.unarchive("a"));
        assert!(!g.unarchive("a"));
        let i = g.add_group("G");
        g.move_to("b", Some(i));
        g.archive("c");
        g.forget("b");
        g.forget("c");
        assert_eq!(
            g,
            SessionGroups {
                groups: vec![Group { name: "G".into(), ..Group::default() }],
                archived: vec![],
                ..SessionGroups::default()
            }
        );
    }

    #[test]
    fn pinned_sessions_sit_above_groups_and_leave_the_archive() {
        let mut g = SessionGroups::default();
        let work = g.add_group("Work");
        g.move_to("b", Some(work));
        g.archive("c");
        g.toggle_pin("b");
        g.toggle_pin("c");
        assert!(g.is_pinned("b") && g.is_pinned("c") && !g.is_archived("c"));
        let a = g.arrange(["a", "b", "c", "d"]);
        assert_eq!(a.pinned, ["b", "c"]);
        assert_eq!(a.top, ["a", "d"]);
        // Still in its group underneath: unpinned, it goes back there.
        assert_eq!(a.groups, [Vec::<String>::new()]);
        g.toggle_pin("b");
        assert_eq!(g.arrange(["b"]).groups, [vec!["b"]]);
        // Archiving or moving a session unpins it.
        g.archive("c");
        assert!(!g.is_pinned("c"));
        g.toggle_pin("a");
        g.move_to("a", Some(work));
        assert!(!g.is_pinned("a"));
    }

    #[test]
    fn names_given_by_hand() {
        let mut g = SessionGroups::default();
        assert_eq!(g.name_of("s"), None);
        g.rename("s", "  Fix the\nlogin   redirect ");
        assert_eq!(g.name_of("s"), Some("Fix the login redirect"));
        g.rename("s", &"x".repeat(200));
        assert_eq!(g.name_of("s").unwrap().chars().count(), MAX_NAME);
        // Blank goes back to the generated name.
        g.rename("s", "   ");
        assert_eq!(g.name_of("s"), None);
        // A deleted session takes its name and pin with it.
        g.rename("t", "Keep");
        g.toggle_pin("t");
        g.forget("t");
        assert_eq!(g, SessionGroups::default());
    }

    #[test]
    fn round_trips_through_json() {
        let mut g = SessionGroups::default();
        let i = g.add_group("G");
        g.move_to("s", Some(i));
        g.archive("t");
        g.toggle_pin("u");
        g.rename("u", "Named");
        let back: SessionGroups = serde_json::from_str(&serde_json::to_string(&g).unwrap()).unwrap();
        assert_eq!(back, g);
        // Older or partial files still load.
        let partial: SessionGroups = serde_json::from_str(r#"{"groups":[{"name":"G"}]}"#).unwrap();
        assert_eq!(partial.groups[0].name, "G");
    }
}
