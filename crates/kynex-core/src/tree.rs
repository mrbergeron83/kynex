//! The sidebar's tree: each workspace of a project a row that folds, and under it its
//! sessions in sections. This module has the rules for what shows where; the app draws it.
//!
//! Under a workspace, in order:
//!
//! 1. **Pinned**, in the order they were pinned (or dragged into).
//! 2. **Recent**, newest first: open sessions, and those used in the last
//!    [`RECENT_FOR`]; never fewer than the newest [`RECENT_AT_LEAST`]. New sessions
//!    with no transcript listed yet come first.
//! 3. The **groups**, in their order, each folded or open.
//! 4. **Older**, folded until opened by hand: everything else, newest first.
//! 5. **Archived**, folded.
//!
//! Which workspaces are open: the selected one, and those opened by hand
//! ([`SessionGroups::tree_open`], kept with the workspace's sessions). While the title
//! filter has a query, whatever holds a match is open, whatever its folded state, and
//! goes back to how it was when the filter closes.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use crate::groups::SessionGroups;

/// How long a session used last stays in Recent.
pub const RECENT_FOR: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Recent has at least this many sessions when the workspace has them.
pub const RECENT_AT_LEAST: usize = 5;

/// One workspace's sessions sorted into the sections they show in, each in the order drawn.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sections {
    pub pinned: Vec<String>,
    pub recent: Vec<String>,
    /// One list per group, in [`SessionGroups::groups`] order.
    pub groups: Vec<Vec<String>>,
    pub older: Vec<String>,
    pub archived: Vec<String>,
}

/// A section under a workspace.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    Pinned,
    Recent,
    /// Group `i` of [`SessionGroups::groups`].
    Group(usize),
    Older,
    Archived,
}

impl Sections {
    /// Every row of every section, folded or not.
    pub fn len(&self) -> usize {
        self.pinned.len()
            + self.recent.len()
            + self.older.len()
            + self.archived.len()
            + self.groups.iter().map(Vec::len).sum::<usize>()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The sections that have rows, in the order drawn.
    pub fn non_empty(&self) -> Vec<Section> {
        let mut out = Vec::new();
        if !self.pinned.is_empty() {
            out.push(Section::Pinned);
        }
        if !self.recent.is_empty() {
            out.push(Section::Recent);
        }
        out.extend(self.groups.iter().enumerate().filter(|(_, g)| !g.is_empty()).map(|(i, _)| Section::Group(i)));
        if !self.older.is_empty() {
            out.push(Section::Older);
        }
        if !self.archived.is_empty() {
            out.push(Section::Archived);
        }
        out
    }

    /// Whether to draw section headers: only with more than one section to tell apart.
    /// Groups, Older and Archived always have theirs, since they fold.
    pub fn headers(&self) -> bool {
        self.non_empty().len() > 1
    }

    /// A section's rows.
    pub fn of(&self, s: Section) -> &[String] {
        match s {
            Section::Pinned => &self.pinned,
            Section::Recent => &self.recent,
            Section::Group(i) => self.groups.get(i).map_or(&[], Vec::as_slice),
            Section::Older => &self.older,
            Section::Archived => &self.archived,
        }
    }

    /// Keeps only the rows `keep` says yes to, each where it was (the title filter).
    /// Sections are worked out before filtering, so a match stays in the section it's in.
    pub fn retain(&mut self, mut keep: impl FnMut(&str) -> bool) {
        for list in [&mut self.pinned, &mut self.recent, &mut self.older, &mut self.archived] {
            list.retain(|id| keep(id));
        }
        for g in &mut self.groups {
            g.retain(|id| keep(id));
        }
    }

    /// Whether `id` is a row here.
    pub fn contains(&self, id: &str) -> bool {
        let has = |l: &Vec<String>| l.iter().any(|s| s == id);
        has(&self.pinned) || has(&self.recent) || has(&self.older) || has(&self.archived) || self.groups.iter().any(has)
    }
}

/// Sorts one workspace's sessions into their [`Sections`].
///
/// - `listed`: its sessions with when each was last used, as read from its transcripts;
///   any order (they're sorted newest first here).
/// - `open`: ids of the sessions open in it (in a pane, or shown): they stay in Recent
///   however old.
/// - `fresh`: open sessions with no transcript listed yet, in the order given: the top
///   of Recent (where they stay once listed, being newest). Ids also in `listed` are
///   dropped from `fresh`.
/// - `now`: the time Recent counts back from.
///
/// Ids saved in `groups` that aren't listed are left out.
pub fn sections(
    groups: &SessionGroups,
    listed: &[(String, SystemTime)],
    open: &[String],
    fresh: &[String],
    now: SystemTime,
) -> Sections {
    let mut by_time: Vec<&(String, SystemTime)> = listed.iter().collect();
    // Newest first; the same time keeps the given order.
    by_time.sort_by(|a, b| b.1.cmp(&a.1));
    let cutoff = now.checked_sub(RECENT_FOR).unwrap_or(SystemTime::UNIX_EPOCH);
    let mut out = Sections { groups: vec![Vec::new(); groups.groups.len()], ..Sections::default() };
    out.recent.extend(fresh.iter().filter(|f| !listed.iter().any(|(id, _)| id == *f)).cloned());
    // Loose sessions (not pinned, grouped or archived), newest first.
    let mut loose = 0;
    for (id, used) in by_time {
        if groups.is_archived(id) {
            out.archived.push(id.clone());
        } else if groups.is_pinned(id) {
            out.pinned.push(id.clone());
        } else if let Some(g) = groups.group_of(id) {
            out.groups[g].push(id.clone());
        } else {
            let recent = loose < RECENT_AT_LEAST || *used >= cutoff || open.contains(id);
            loose += 1;
            if recent { &mut out.recent } else { &mut out.older }.push(id.clone());
        }
    }
    out.pinned.sort_by_key(|id| groups.pinned.iter().position(|p| p == id));
    out
}

/// How a workspace's sections are folded right now.
#[derive(Clone, Copy, Debug)]
pub struct View<'a> {
    pub groups: &'a SessionGroups,
    /// Its Archived section is open (not saved: it starts folded).
    pub archived_open: bool,
    /// The title filter has a query.
    pub filtering: bool,
}

impl View<'_> {
    /// Whether section `s`, holding `rows` rows, draws them. While filtering, every section
    /// with a match does.
    pub fn is_open(&self, s: Section, rows: usize) -> bool {
        if self.filtering {
            return rows > 0;
        }
        match s {
            Section::Pinned | Section::Recent => true,
            Section::Group(i) => !self.groups.groups.get(i).is_some_and(|g| g.collapsed),
            Section::Older => self.groups.older_open,
            Section::Archived => self.archived_open,
        }
    }

    /// The rows drawn under the workspace, in order: the sections that are open.
    pub fn drawn(&self, sections: &Sections) -> Vec<String> {
        let mut out = Vec::new();
        for s in sections.non_empty() {
            let rows = sections.of(s);
            if self.is_open(s, rows.len()) {
                out.extend(rows.iter().cloned());
            }
        }
        out
    }
}

/// Whether a workspace is open in the tree. Not filtering: the selected one, and those
/// opened by hand. Filtering: those with a match.
pub fn workspace_open(selected: bool, opened_by_hand: bool, filtering: bool, matches: usize) -> bool {
    if filtering { matches > 0 } else { selected || opened_by_hand }
}

/// Whether `title` matches the filter's `query` (trimmed, any case); everything matches an
/// empty one.
pub fn matches(query: &str, title: &str) -> bool {
    let q = query.trim().to_lowercase();
    q.is_empty() || title.to_lowercase().contains(&q)
}

/// A workspace as the tree draws it: open or folded, and the rows drawn under it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Branch {
    pub path: PathBuf,
    pub open: bool,
    /// [`View::drawn`] for it.
    pub rows: Vec<String>,
}

/// Every session row of the tree in the order drawn, with the workspace each is under:
/// what the list keys (↑ / ↓) and Shift+click runs walk. Folded workspaces have none.
pub fn rows_in_order(branches: &[Branch]) -> Vec<(PathBuf, String)> {
    branches.iter().filter(|b| b.open).flat_map(|b| b.rows.iter().map(move |id| (b.path.clone(), id.clone()))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: u64 = 24 * 60 * 60;

    /// A fixed "now", so no test depends on today's date.
    fn now() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_000 * DAY)
    }

    fn days_ago(d: u64) -> SystemTime {
        now() - Duration::from_secs(d * DAY)
    }

    /// Sessions named by how many days ago each was used.
    fn listed(list: &[(&str, u64)]) -> Vec<(String, SystemTime)> {
        list.iter().map(|(id, d)| (id.to_string(), days_ago(*d))).collect()
    }

    fn ids(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn recent_is_the_last_week_and_older_the_rest() {
        let g = SessionGroups::default();
        let l = listed(&[("a", 0), ("b", 1), ("c", 3), ("d", 6), ("e", 7), ("f", 9), ("g", 30), ("h", 400)]);
        let s = sections(&g, &l, &[], &[], now());
        // "e", used exactly a week ago, is still in.
        assert_eq!(s.recent, ids(&["a", "b", "c", "d", "e"]));
        assert_eq!(s.older, ids(&["f", "g", "h"]));
        assert_eq!(s.non_empty(), [Section::Recent, Section::Older]);
    }

    #[test]
    fn recent_has_at_least_the_newest_five() {
        let g = SessionGroups::default();
        // Nothing used this week: the newest five are recent anyway.
        let l = listed(&[("old1", 50), ("old2", 60), ("old3", 70), ("old4", 80), ("old5", 90), ("old6", 100)]);
        assert_eq!(sections(&g, &l, &[], &[], now()).recent, ids(&["old1", "old2", "old3", "old4", "old5"]));
        // Fewer than five in all: every one.
        let few = listed(&[("x", 300), ("y", 200)]);
        let s = sections(&g, &few, &[], &[], now());
        assert_eq!((s.recent, s.older), (ids(&["y", "x"]), vec![]));
    }

    #[test]
    fn listed_in_any_order_come_out_newest_first() {
        let g = SessionGroups::default();
        let l = listed(&[("mid", 2), ("new", 0), ("old", 5)]);
        assert_eq!(sections(&g, &l, &[], &[], now()).recent, ids(&["new", "mid", "old"]));
    }

    #[test]
    fn an_open_session_stays_recent_however_old() {
        let g = SessionGroups::default();
        let l = listed(&[("a", 0), ("b", 1), ("c", 2), ("d", 3), ("e", 4), ("f", 20), ("opened", 90)]);
        let s = sections(&g, &l, &ids(&["opened"]), &[], now());
        assert_eq!(s.recent, ids(&["a", "b", "c", "d", "e", "opened"]));
        assert_eq!(s.older, ids(&["f"]));
    }

    #[test]
    fn new_sessions_come_first_in_recent() {
        let g = SessionGroups::default();
        let l = listed(&[("a", 0), ("b", 1)]);
        // "a" is fresh in the caller's eyes but already listed: it stays where its time puts it.
        let s = sections(&g, &l, &[], &ids(&["new:1", "a"]), now());
        assert_eq!(s.recent, ids(&["new:1", "a", "b"]));
        assert!(s.pinned.is_empty());
    }

    #[test]
    fn pinned_grouped_and_archived_leave_recent_and_older() {
        let mut g = SessionGroups::default();
        let work = g.add_group("Work");
        g.move_to("grouped", Some(work));
        g.toggle_pin("old-pin");
        g.toggle_pin("new-pin");
        g.archive("gone");
        let l = listed(&[("new-pin", 0), ("grouped", 0), ("gone", 0), ("loose", 1), ("old-pin", 300), ("stale", 30)]);
        let s = sections(&g, &l, &[], &[], now());
        // A pinned session is pinned however old, in pin order.
        assert_eq!(s.pinned, ids(&["old-pin", "new-pin"]));
        assert_eq!(s.groups, [ids(&["grouped"])]);
        assert_eq!(s.archived, ids(&["gone"]));
        // The newest five count loose sessions only: "stale" is the second.
        assert_eq!((s.recent.clone(), s.older.clone()), (ids(&["loose", "stale"]), vec![]));
        assert_eq!(s.len(), 6);
        assert!(s.contains("grouped") && !s.contains("nope"));
        assert_eq!(s.non_empty(), [Section::Pinned, Section::Recent, Section::Group(0), Section::Archived]);
    }

    #[test]
    fn an_empty_workspace_has_no_sections() {
        let mut g = SessionGroups::default();
        g.add_group("Empty");
        // Saved ids of sessions that aren't listed are left out.
        g.toggle_pin("deleted");
        let s = sections(&g, &[], &[], &[], now());
        assert!(s.is_empty() && s.non_empty().is_empty() && !s.headers());
        assert_eq!(s.groups, [Vec::<String>::new()]);
        assert!(View { groups: &g, archived_open: true, filtering: false }.drawn(&s).is_empty());
    }

    #[test]
    fn folded_sections_draw_no_rows() {
        let mut g = SessionGroups::default();
        let (a, b) = (g.add_group("A"), g.add_group("B"));
        g.move_to("ga", Some(a));
        g.move_to("gb", Some(b));
        g.toggle_group(b);
        g.archive("arch");
        let mut l = listed(&[("ga", 0), ("gb", 0), ("arch", 0), ("r1", 0), ("r2", 1), ("r3", 1), ("r4", 2), ("r5", 3)]);
        l.extend(listed(&[("old", 60)]));
        let s = sections(&g, &l, &[], &[], now());
        assert!(s.headers());
        let view = View { groups: &g, archived_open: false, filtering: false };
        // Group B folded, Older and Archived folded.
        assert_eq!(view.drawn(&s), ids(&["r1", "r2", "r3", "r4", "r5", "ga"]));
        g.older_open = true;
        let view = View { groups: &g, archived_open: true, filtering: false };
        assert_eq!(view.drawn(&s), ids(&["r1", "r2", "r3", "r4", "r5", "ga", "old", "arch"]));
    }

    #[test]
    fn filtering_opens_what_holds_a_match() {
        let mut g = SessionGroups::default();
        let i = g.add_group("Folded");
        g.move_to("fix grouped", Some(i));
        g.toggle_group(i);
        g.archive("fix archived");
        let mut l =
            listed(&[("fix grouped", 0), ("fix archived", 0), ("a", 0), ("b", 0), ("c", 0), ("d", 0), ("e", 0)]);
        l.extend(listed(&[("fix old", 90), ("other old", 91)]));
        let mut s = sections(&g, &l, &[], &[], now());
        // Recent is worked out before the filter: "fix old" stays in Older.
        s.retain(|id| matches("FIX", id));
        assert_eq!(s.recent, Vec::<String>::new());
        assert_eq!(s.older, ids(&["fix old"]));
        let view = View { groups: &g, archived_open: false, filtering: true };
        assert!(view.is_open(Section::Group(i), 1) && !view.is_open(Section::Recent, 0));
        assert_eq!(view.drawn(&s), ids(&["fix grouped", "fix old", "fix archived"]));
        // The folded state is untouched: closing the filter puts it back.
        let view = View { filtering: false, ..view };
        assert!(view.drawn(&s).is_empty());
    }

    #[test]
    fn sessions_files_of_the_last_release_load_and_lose_nothing() {
        // As 0.0.28 writes it, date headers folded by hand included.
        let old = r#"{"groups":[{"name":"Work","sessions":["s1"],"collapsed":true}],"archived":["s2"],"dates_toggled":["today","2026-03"],"pinned":["s3"],"names":{"s3":"Named"},"models":{"s3":"opus"},"providers":{"s4":"grok"}}"#;
        let g: SessionGroups = serde_json::from_str(old).unwrap();
        assert!(!g.tree_open && !g.older_open);
        let back: serde_json::Value = serde_json::to_value(&g).unwrap();
        assert_eq!(back, serde_json::from_str::<serde_json::Value>(old).unwrap());
        // The new fields round-trip.
        let g = SessionGroups { tree_open: true, older_open: true, ..g };
        let again: SessionGroups = serde_json::from_str(&serde_json::to_string(&g).unwrap()).unwrap();
        assert_eq!(again, g);
    }

    #[test]
    fn titles_match_in_any_case() {
        assert!(matches("", "anything") && matches("  ", "x"));
        assert!(matches(" Login ", "Fix the login redirect"));
        assert!(!matches("logout", "Fix the login redirect"));
    }

    #[test]
    fn which_workspaces_are_open() {
        // Selected, or opened by hand.
        assert!(workspace_open(true, false, false, 0));
        assert!(workspace_open(false, true, false, 0));
        assert!(!workspace_open(false, false, false, 3));
        // Filtering: only those with a match, folded by hand or not.
        assert!(workspace_open(false, false, true, 1));
        assert!(!workspace_open(true, true, true, 0));
    }

    #[test]
    fn rows_in_order_skip_folded_workspaces() {
        let (a, b, c) = (PathBuf::from("/a"), PathBuf::from("/b"), PathBuf::from("/c"));
        let branches = [
            Branch { path: a.clone(), open: true, rows: ids(&["a1", "a2"]) },
            Branch { path: b, open: false, rows: ids(&["b1"]) },
            Branch { path: c.clone(), open: true, rows: ids(&["c1"]) },
        ];
        assert_eq!(
            rows_in_order(&branches),
            [(a.clone(), "a1".to_string()), (a, "a2".to_string()), (c, "c1".to_string())]
        );
    }
}
