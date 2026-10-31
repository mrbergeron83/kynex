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
    /// Models pinned to a session, by session id, over the project's own;
    /// empty is the CLI's default.
    pub models: BTreeMap<String, String>,
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
    /// Working, waiting for you, or finished and unseen: above everything,
    /// instead of where they'd be otherwise.
    pub active: Vec<String>,
    /// Above the rest, whatever group they're in, in the order they were pinned
    /// (or dragged into).
    pub pinned: Vec<String>,
    pub top: Vec<String>,
    /// One list per group, in [`SessionGroups::groups`] order.
    pub groups: Vec<Vec<String>>,
    pub archived: Vec<String>,
}

impl Arranged {
    /// How many sections have rows: Active, Pinned, each group, the top level
    /// and Archived. With one, the panel draws no headers.
    pub fn sections(&self) -> usize {
        [&self.active, &self.pinned, &self.top, &self.archived].iter().filter(|l| !l.is_empty()).count()
            + self.groups.iter().filter(|g| !g.is_empty()).count()
    }
}

/// Sessions picked in the panel to archive or delete together: Ctrl+click
/// picks one, Shift+click a run of rows. Not saved.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    /// Picking: a plain click picks or drops a row instead of opening it.
    pub on: bool,
    /// In the order they were picked.
    pub ids: Vec<String>,
    /// The last row picked by itself, where Shift+click runs start.
    anchor: Option<String>,
}

impl Selection {
    pub fn contains(&self, id: &str) -> bool {
        self.ids.iter().any(|s| s == id)
    }

    /// Picks `id`, or drops it if it's picked.
    pub fn toggle(&mut self, id: &str) {
        self.on = true;
        match self.ids.iter().position(|s| s == id) {
            Some(i) => {
                self.ids.remove(i);
            }
            None => self.ids.push(id.to_string()),
        }
        self.anchor = Some(id.to_string());
    }

    /// Picks every row of `shown` (the rows in panel order) from the last one
    /// picked to `id`; with none picked yet, just `id`.
    pub fn extend(&mut self, shown: &[String], id: &str) {
        self.on = true;
        let at = |id: &str| shown.iter().position(|s| s == id);
        let (Some(from), Some(to)) = (self.anchor.as_deref().and_then(at), at(id)) else {
            if !self.contains(id) {
                self.toggle(id);
            }
            return;
        };
        for s in &shown[from.min(to)..=from.max(to)] {
            if !self.contains(s) {
                self.ids.push(s.clone());
            }
        }
    }

    /// Picks every row of `shown`, or none if they all are.
    pub fn toggle_all(&mut self, shown: &[String]) {
        self.on = true;
        if shown.iter().all(|s| self.contains(s)) {
            self.ids.clear();
        } else {
            self.ids = shown.to_vec();
        }
        self.anchor = None;
    }

    /// Drops picks that are no longer listed (deleted, or filtered out).
    pub fn keep_only(&mut self, listed: &[String]) {
        self.ids.retain(|s| listed.contains(s));
        if self.anchor.as_ref().is_some_and(|a| !listed.contains(a)) {
            self.anchor = None;
        }
    }

    /// Stops picking and drops every pick.
    pub fn clear(&mut self) {
        *self = Self::default();
    }
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

    /// The model pinned to a session, if any.
    pub fn model_of(&self, id: &str) -> Option<&str> {
        self.models.get(id).map(String::as_str)
    }

    /// Pins a model to a session (`Some`), or has it follow the project's (`None`).
    pub fn set_model(&mut self, id: &str, model: Option<&str>) {
        match model {
            Some(m) => {
                self.models.insert(id.to_string(), m.to_string());
            }
            None => {
                self.models.remove(id);
            }
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

    /// Deletes a group and archives its sessions.
    pub fn delete_group_archiving(&mut self, i: usize) {
        let Some(g) = self.groups.get(i) else { return };
        for id in g.sessions.clone() {
            self.archive(&id);
        }
        self.delete_group(i);
    }

    /// Moves group `from` to place `to` among the groups.
    pub fn move_group(&mut self, from: usize, to: usize) {
        crate::tab_order::move_item(&mut self.groups, from, to);
    }

    /// Moves pinned session `id` to just before pinned session `before`, or to
    /// the end with `None`. Pins it if it isn't.
    pub fn move_pin(&mut self, id: &str, before: Option<&str>) {
        if !self.is_pinned(id) {
            self.unarchive(id);
        }
        self.pinned.retain(|s| s != id);
        let at = before.and_then(|b| self.pinned.iter().position(|s| s == b)).unwrap_or(self.pinned.len());
        self.pinned.insert(at, id.to_string());
    }

    /// Archives the sessions of `sessions` (id, last used, seconds since the
    /// epoch) not used since `cutoff`, except pinned and grouped ones and those
    /// already archived. Returns how many it archived.
    pub fn archive_older_than(&mut self, sessions: &[(String, i64)], cutoff: i64) -> usize {
        let old: Vec<String> = sessions
            .iter()
            .filter(|(id, used)| {
                *used < cutoff && !self.is_pinned(id) && self.group_of(id).is_none() && !self.is_archived(id)
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in &old {
            self.archive(id);
        }
        old.len()
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
        self.models.remove(id);
    }

    /// Sorts session ids (newest first) into the pinned ones, the top level,
    /// groups and archive. Ids saved here that aren't in `ids` are left out.
    pub fn arrange<'a>(&self, ids: impl IntoIterator<Item = &'a str>) -> Arranged {
        self.arrange_with(ids, &[])
    }

    /// [`Self::arrange`], with the `active` sessions (working, waiting, or
    /// finished and unseen) taken out of their places into [`Arranged::active`].
    pub fn arrange_with<'a>(&self, ids: impl IntoIterator<Item = &'a str>, active: &[String]) -> Arranged {
        let mut out = Arranged { groups: vec![Vec::new(); self.groups.len()], ..Arranged::default() };
        for id in ids {
            let list = if active.iter().any(|a| a == id) {
                &mut out.active
            } else if self.is_archived(id) {
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
        // Pinned ones keep the order they were pinned (or dragged) in.
        out.pinned.sort_by_key(|id| self.pinned.iter().position(|p| p == id));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn active_sessions_leave_their_places_and_pins_keep_their_order() {
        let mut g = SessionGroups::default();
        let i = g.add_group("Release");
        g.move_to("grouped", Some(i));
        g.toggle_pin("p2");
        g.toggle_pin("p1");
        g.archive("old");
        // Newest first: p1 is newer, but p2 was pinned first.
        let all = ["busy", "p1", "p2", "grouped", "loose", "old"];
        let a = g.arrange_with(all, &ids(&["busy", "grouped"]));
        assert_eq!(a.active, ["busy", "grouped"]);
        assert_eq!(a.pinned, ["p2", "p1"]);
        assert_eq!(a.groups, [Vec::<String>::new()]);
        assert_eq!((a.top.clone(), a.archived.clone()), (ids(&["loose"]), ids(&["old"])));
        assert_eq!(a.sections(), 4);
        // Dragging a pin before another reorders them.
        g.move_pin("p1", Some("p2"));
        assert_eq!(g.arrange(all).pinned, ["p1", "p2"]);
        assert_eq!(g.arrange(["loose"]).sections(), 1);
    }

    #[test]
    fn groups_reorder_and_delete_either_way() {
        let mut g = SessionGroups::default();
        let (a, b) = (g.add_group("A"), g.add_group("B"));
        g.move_to("s1", Some(a));
        g.move_to("s2", Some(b));
        g.move_group(1, 0);
        assert_eq!(g.groups.iter().map(|g| g.name.as_str()).collect::<Vec<_>>(), ["B", "A"]);
        g.delete_group_archiving(0);
        assert!(g.is_archived("s2") && g.groups.len() == 1);
        g.delete_group(0);
        assert!(!g.is_archived("s1") && g.group_of("s1").is_none());
    }

    #[test]
    fn tidying_archives_old_loose_sessions_only() {
        let mut g = SessionGroups::default();
        let i = g.add_group("Keep");
        g.move_to("old-grouped", Some(i));
        g.toggle_pin("old-pinned");
        let sessions: Vec<(String, i64)> =
            [("new", 900), ("old", 10), ("old-grouped", 10), ("old-pinned", 10)].map(|(s, t)| (s.into(), t)).into();
        assert_eq!(g.archive_older_than(&sessions, 500), 1);
        assert!(g.is_archived("old") && !g.is_archived("new"));
        assert!(!g.is_archived("old-grouped") && !g.is_archived("old-pinned"));
        // Already archived: nothing more to do.
        assert_eq!(g.archive_older_than(&sessions, 500), 0);
    }

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
        g.set_model("t", Some("opus"));
        g.forget("t");
        assert_eq!(g, SessionGroups::default());
    }

    #[test]
    fn models_pinned_to_a_session() {
        let mut g = SessionGroups::default();
        assert_eq!(g.model_of("s"), None);
        g.set_model("s", Some("haiku"));
        assert_eq!(g.model_of("s"), Some("haiku"));
        // Pinned to the CLI's default, whatever the project picks.
        g.set_model("s", Some(""));
        assert_eq!(g.model_of("s"), Some(""));
        g.set_model("s", None);
        assert_eq!(g.model_of("s"), None);
    }

    #[test]
    fn round_trips_through_json() {
        let mut g = SessionGroups::default();
        let i = g.add_group("G");
        g.move_to("s", Some(i));
        g.archive("t");
        g.toggle_pin("u");
        g.rename("u", "Named");
        g.set_model("u", Some("sonnet"));
        let back: SessionGroups = serde_json::from_str(&serde_json::to_string(&g).unwrap()).unwrap();
        assert_eq!(back, g);
        // Older or partial files still load.
        let partial: SessionGroups = serde_json::from_str(r#"{"groups":[{"name":"G"}]}"#).unwrap();
        assert_eq!(partial.groups[0].name, "G");
    }

    #[test]
    fn selection_picks_rows_runs_and_all() {
        let shown: Vec<String> = ["a", "b", "c", "d", "e"].map(String::from).to_vec();
        let mut s = Selection::default();
        // Shift+click with nothing picked picks just that row.
        s.extend(&shown, "b");
        assert!(s.on);
        assert_eq!(s.ids, ["b"]);
        // A run from it, either way, without doubling what's picked.
        s.extend(&shown, "d");
        assert_eq!(s.ids, ["b", "c", "d"]);
        s.toggle("e");
        s.extend(&shown, "c");
        assert_eq!(s.ids, ["b", "c", "d", "e"]);
        s.toggle("c");
        assert!(!s.contains("c"));
        // Gone from the list, gone from the picks.
        s.keep_only(&["b".into(), "d".into()]);
        assert_eq!(s.ids, ["b", "d"]);
        s.toggle_all(&shown);
        assert_eq!(s.ids.len(), 5);
        s.toggle_all(&shown);
        assert!(s.ids.is_empty() && s.on);
        s.clear();
        assert_eq!(s, Selection::default());
    }
}
