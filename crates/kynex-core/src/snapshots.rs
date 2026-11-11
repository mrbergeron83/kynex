//! A workspace's recent layouts: undo / redo of changes to its panes, reopening a pane
//! that was closed, and a list of older layouts to bring back.
//!
//! The app calls [`History::record`] with the workspace's current layout every poll. A
//! change to the arrangement (panes, splits, tabs, their sessions or tools; not the
//! focus, the active tab or zoom) is kept for undo, up to [`KEEP`] of them. A layout is
//! also kept every [`EVERY`] seconds when it changed since the last one; those, [`KEEP`]
//! of them, are saved with the workspace so yesterday's layout is still there after a
//! restart. Undo, redo and the lists stay in this run.

use serde::{Deserialize, Serialize};

use crate::tiling::SavedLayout;

/// Layouts kept of each kind.
pub const KEEP: usize = 48;
/// Seconds between the layouts kept on a timer (herdr's 15 minutes).
pub const EVERY: u64 = 15 * 60;
/// Closed panes that can be reopened.
const CLOSED: usize = 10;
/// Changes this close together (seconds) count as one undo step, unless a pane closed.
const COALESCE: u64 = 3;

/// A layout and when it was current (seconds since the Unix epoch).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub at: u64,
    pub layout: SavedLayout,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct History {
    /// The layouts kept on a timer, oldest first. The only part that is saved.
    pub timed: Vec<Snapshot>,
    #[serde(skip)]
    undo: Vec<Snapshot>,
    #[serde(skip)]
    redo: Vec<Snapshot>,
    /// The layout from just before each pane was closed, newest last.
    #[serde(skip)]
    closed: Vec<Snapshot>,
    #[serde(skip)]
    current: Option<Snapshot>,
    /// A layout was handed out to be applied: the next one recorded is that, not a change.
    #[serde(skip)]
    applying: bool,
    /// The current layout was just applied: the next change is a step of its own, however soon.
    #[serde(skip)]
    after_apply: bool,
}

impl Snapshot {
    /// What it holds, in words: "3 panes", "4 panes in 2 tabs".
    pub fn summary(&self) -> String {
        let panes = self.layout.layout.panes().len();
        let tabs = self.layout.layout.tabs().len();
        let p = if panes == 1 { "1 pane".to_string() } else { format!("{panes} panes") };
        if tabs > 1 { format!("{p} in {tabs} tabs") } else { p }
    }
}

/// Whether two layouts arrange the same panes the same way: the focus, the active tab and
/// zoom aren't part of it.
pub fn same_shape(a: &SavedLayout, b: &SavedLayout) -> bool {
    let (x, y) = (a.layout.tabs(), b.layout.tabs());
    x.len() == y.len()
        && x.iter().zip(y).all(|(p, q)| p.name == q.name && p.root() == q.root())
        && a.sessions == b.sessions
        && a.tools == b.tools
}

impl History {
    /// Notes the workspace's layout as of `now`. Returns whether the saved part
    /// (`timed`) changed.
    pub fn record(&mut self, now: u64, layout: &SavedLayout) -> bool {
        if std::mem::take(&mut self.applying) {
            self.after_apply = true;
            self.current = Some(Snapshot { at: now, layout: layout.clone() });
        } else if let Some(cur) = self.current.take() {
            let since = if same_shape(&cur.layout, layout) {
                cur.at
            } else {
                let closing = layout.layout.panes().len() < cur.layout.layout.panes().len();
                let quick = now.saturating_sub(cur.at) < COALESCE
                    && !closing
                    && !self.undo.is_empty()
                    && !std::mem::take(&mut self.after_apply);
                if closing {
                    push(&mut self.closed, cur.clone(), CLOSED);
                }
                if !quick {
                    push(&mut self.undo, cur, KEEP);
                }
                self.redo.clear();
                now
            };
            self.current = Some(Snapshot { at: since, layout: layout.clone() });
        } else {
            self.current = Some(Snapshot { at: now, layout: layout.clone() });
        }
        let due = self.timed.last().is_none_or(|t| now.saturating_sub(t.at) >= EVERY && !same_shape(&t.layout, layout));
        if due {
            push(&mut self.timed, Snapshot { at: now, layout: layout.clone() }, KEEP);
        }
        due
    }

    /// The workspace has no layout worth keeping for now: what comes next isn't a change to
    /// the one before.
    pub fn pause(&mut self) {
        self.current = None;
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn can_reopen(&self) -> bool {
        !self.closed.is_empty()
    }

    /// The layout before the last change, to apply. It becomes the current one.
    pub fn undo(&mut self) -> Option<SavedLayout> {
        let back = self.undo.pop()?;
        if let Some(cur) = self.current.take() {
            self.redo.push(cur);
        }
        self.apply(back)
    }

    /// The layout an undo came from, to apply.
    pub fn redo(&mut self) -> Option<SavedLayout> {
        let forward = self.redo.pop()?;
        if let Some(cur) = self.current.take() {
            self.undo.push(cur);
        }
        self.apply(forward)
    }

    /// The layout from just before the pane closed last, to apply (a change like any other:
    /// it can be undone).
    pub fn reopen_closed(&mut self) -> Option<SavedLayout> {
        let before = self.closed.pop()?;
        self.change_to(before)
    }

    /// The older layouts to pick from, newest first, less those that arrange the panes as
    /// the current one does or as the one before.
    pub fn entries(&self) -> Vec<Snapshot> {
        let mut all: Vec<&Snapshot> = self.timed.iter().chain(&self.undo).collect();
        all.sort_by_key(|s| std::cmp::Reverse(s.at));
        let mut out: Vec<Snapshot> = Vec::new();
        for s in all {
            let current = self.current.as_ref().is_some_and(|c| same_shape(&c.layout, &s.layout));
            if !current && !out.last().is_some_and(|p| same_shape(&p.layout, &s.layout)) {
                out.push(s.clone());
            }
        }
        out
    }

    /// Brings back `snapshot` (one of [`Self::entries`]) as a change that can be undone.
    pub fn restore(&mut self, snapshot: &Snapshot) -> Option<SavedLayout> {
        self.change_to(snapshot.clone())
    }

    fn change_to(&mut self, to: Snapshot) -> Option<SavedLayout> {
        if let Some(cur) = self.current.take() {
            push(&mut self.undo, cur, KEEP);
        }
        self.redo.clear();
        self.apply(to)
    }

    fn apply(&mut self, s: Snapshot) -> Option<SavedLayout> {
        self.applying = true;
        self.current = Some(s.clone());
        Some(s.layout)
    }
}

fn push(list: &mut Vec<Snapshot>, s: Snapshot, keep: usize) {
    list.push(s);
    if list.len() > keep {
        list.remove(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{Axis, Layout, Rect};
    use std::collections::BTreeMap;

    fn one(session: &str) -> SavedLayout {
        SavedLayout {
            layout: Layout::new("1", 1),
            sessions: BTreeMap::from([(1, session.to_string())]),
            tools: None,
            seeded: false,
        }
    }

    fn two() -> SavedLayout {
        let mut layout = Layout::new("1", 1);
        layout.split(1, Axis::Row, 2, Rect::new(0.0, 0.0, 2000.0, 1000.0));
        SavedLayout {
            layout,
            sessions: BTreeMap::from([(1, "a".to_string()), (2, "b".to_string())]),
            tools: None,
            seeded: false,
        }
    }

    #[test]
    fn the_first_layout_is_kept_and_a_repeat_adds_nothing() {
        let mut h = History::default();
        assert!(h.record(0, &one("a")));
        assert!(!h.record(10, &one("a")));
        assert!(!h.can_undo());
        assert_eq!(h.timed.len(), 1);
    }

    #[test]
    fn a_change_can_be_undone_and_redone() {
        let mut h = History::default();
        h.record(0, &one("a"));
        h.record(100, &two());
        assert!(h.can_undo() && !h.can_redo());
        assert_eq!(h.undo(), Some(one("a")));
        // The app applies it and the next poll reports it.
        h.record(101, &one("a"));
        assert!(h.can_redo() && !h.can_undo());
        assert_eq!(h.redo(), Some(two()));
        h.record(102, &two());
        assert!(h.can_undo() && !h.can_redo());
    }

    #[test]
    fn a_new_change_after_an_undo_drops_the_redo() {
        let mut h = History::default();
        h.record(0, &one("a"));
        h.record(100, &two());
        h.undo();
        h.record(101, &one("a"));
        h.record(200, &one("b"));
        assert!(!h.can_redo());
    }

    #[test]
    fn the_focus_is_not_a_change() {
        let mut h = History::default();
        let mut a = two();
        h.record(0, &a);
        if let Some(t) = a.layout.tab_mut() {
            t.set_focus(1);
        }
        h.record(100, &a);
        assert!(!h.can_undo());
    }

    #[test]
    fn changes_in_a_burst_are_one_step() {
        let mut h = History::default();
        h.record(0, &one("a"));
        h.record(100, &one("b"));
        h.record(101, &one("c"));
        h.record(102, &one("d"));
        assert_eq!(h.undo(), Some(one("a")), "b, c and d came together");
        assert!(!h.can_undo());
    }

    #[test]
    fn a_change_right_after_an_undo_is_a_step_of_its_own() {
        let mut h = History::default();
        h.record(0, &one("a"));
        h.record(100, &one("b"));
        h.record(200, &one("c"));
        assert_eq!(h.undo(), Some(one("b")));
        h.record(201, &one("b"));
        h.record(202, &one("d"));
        assert_eq!(h.undo(), Some(one("b")), "not past the layout that was applied");
    }

    #[test]
    fn a_closed_pane_can_be_reopened_and_that_can_be_undone() {
        let mut h = History::default();
        h.record(0, &two());
        h.record(100, &one("a"));
        assert!(h.can_reopen());
        assert_eq!(h.reopen_closed(), Some(two()));
        h.record(101, &two());
        assert!(!h.can_reopen());
        assert_eq!(h.undo(), Some(one("a")));
    }

    #[test]
    fn applying_a_layout_does_not_count_as_closing_a_pane() {
        let mut h = History::default();
        h.record(0, &two());
        h.record(100, &one("a"));
        h.reopen_closed();
        // The panes came back with a session gone: one pane, not what was handed out.
        h.record(101, &one("a"));
        assert!(!h.can_reopen());
    }

    #[test]
    fn a_layout_is_kept_each_quarter_hour_if_it_changed() {
        let mut h = History::default();
        h.record(0, &one("a"));
        h.record(60, &one("b"));
        assert_eq!(h.timed.len(), 1, "too soon");
        assert!(h.record(EVERY + 1, &one("b")));
        assert!(!h.record(2 * EVERY + 2, &one("b")), "same as the last kept");
        assert_eq!(h.timed.len(), 2);
    }

    #[test]
    fn only_so_many_are_kept() {
        let mut h = History::default();
        for i in 0..(KEEP as u64 + 10) {
            h.record(i * EVERY, &one(&i.to_string()));
        }
        assert_eq!(h.timed.len(), KEEP);
        assert_eq!(h.undo.len(), KEEP);
    }

    #[test]
    fn older_layouts_are_listed_newest_first_and_can_be_brought_back() {
        let mut h = History::default();
        h.record(0, &one("a"));
        h.record(EVERY + 5, &two());
        h.record(3 * EVERY, &one("b"));
        let entries = h.entries();
        assert_eq!(entries.iter().map(|e| e.layout.clone()).collect::<Vec<_>>(), [two(), one("a")]);
        let old = entries[1].clone();
        assert_eq!(h.restore(&old), Some(one("a")));
        h.record(3 * EVERY + 1, &one("a"));
        assert_eq!(h.undo(), Some(one("b")));
    }

    #[test]
    fn a_snapshot_says_what_it_holds() {
        let at = |layout| Snapshot { at: 0, layout };
        assert_eq!(at(one("a")).summary(), "1 pane");
        assert_eq!(at(two()).summary(), "2 panes");
    }

    #[test]
    fn only_the_timed_layouts_are_saved() {
        let mut h = History::default();
        h.record(0, &one("a"));
        h.record(100, &two());
        let back: History = serde_json::from_str(&serde_json::to_string(&h).unwrap()).unwrap();
        assert_eq!(back.timed, h.timed);
        assert!(!back.can_undo());
        assert_eq!(serde_json::from_str::<History>("{}").unwrap(), History::default());
    }
}
