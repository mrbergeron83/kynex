//! How a folder's sessions are organised in the sessions panel: named groups
//! that collapse, and an archive that hides sessions without deleting them.
//! Kept per project as `sessions.json` in its settings folder (see [`crate::project`]).

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
}

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

    /// Hides a session; it leaves its group, and comes back at the top level.
    pub fn archive(&mut self, id: &str) {
        self.leave_group(id);
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
    /// A session moved anywhere is no longer archived.
    pub fn move_to(&mut self, id: &str, to: Option<usize>) {
        self.unarchive(id);
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
    }

    /// Sorts session ids (newest first) into the top level, groups and archive.
    /// Ids saved here that aren't in `ids` are left out.
    pub fn arrange<'a>(&self, ids: impl IntoIterator<Item = &'a str>) -> Arranged {
        let mut out = Arranged { groups: vec![Vec::new(); self.groups.len()], ..Arranged::default() };
        for id in ids {
            let list = if self.is_archived(id) {
                &mut out.archived
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
    fn round_trips_through_json() {
        let mut g = SessionGroups::default();
        let i = g.add_group("G");
        g.move_to("s", Some(i));
        g.archive("t");
        let back: SessionGroups = serde_json::from_str(&serde_json::to_string(&g).unwrap()).unwrap();
        assert_eq!(back, g);
        // Older or partial files still load.
        let partial: SessionGroups = serde_json::from_str(r#"{"groups":[{"name":"G"}]}"#).unwrap();
        assert_eq!(partial.groups[0].name, "G");
    }
}
