//! A workspace's recent layouts: undo / redo of changes to its panes, reopening a closed pane,
//! and timed snapshots to restore.

use serde::{Deserialize, Serialize};

use crate::layout::{Axis, PaneId, Rect};
use crate::tiling::{AgentCliPane, SavedLayout, Tool};

pub const KEEP: usize = 48;
pub const EVERY: u64 = 15 * 60;
const CLOSED: usize = 10;
const COALESCE: u64 = 3;

#[derive(Clone, Debug, PartialEq)]
struct Closed {
    agent: Option<AgentCliPane>,
    tool: Option<Tool>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub at: u64,
    pub layout: SavedLayout,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct History {
    pub timed: Vec<Snapshot>,
    #[serde(skip)]
    undo: Vec<Snapshot>,
    #[serde(skip)]
    redo: Vec<Snapshot>,
    #[serde(skip)]
    closed: Vec<Closed>,
    #[serde(skip)]
    current: Option<Snapshot>,
    #[serde(skip)]
    applying: bool,
    #[serde(skip)]
    after_apply: bool,
}

impl Snapshot {
    pub fn summary(&self) -> String {
        let panes = self.layout.layout.panes().len();
        if panes == 1 { "1 pane".to_string() } else { format!("{panes} panes") }
    }
}

pub fn same_shape(a: &SavedLayout, b: &SavedLayout) -> bool {
    a.layout.root() == b.layout.root() && a.agents == b.agents && a.tools == b.tools
}

impl History {
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
                    let now_there = layout.layout.panes();
                    for pane in cur.layout.layout.panes().into_iter().filter(|p| !now_there.contains(p)) {
                        let tool = cur.layout.tools.as_ref().and_then(|t| t.get(&pane)).cloned();
                        let agent = cur.layout.agents.get(&pane).cloned();
                        self.closed.push(Closed { agent, tool });
                        if self.closed.len() > CLOSED {
                            self.closed.remove(0);
                        }
                    }
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

    pub fn undo(&mut self) -> Option<SavedLayout> {
        let back = self.undo.pop()?;
        if let Some(cur) = self.current.take() {
            self.redo.push(cur);
        }
        self.apply(back)
    }

    pub fn redo(&mut self) -> Option<SavedLayout> {
        let forward = self.redo.pop()?;
        if let Some(cur) = self.current.take() {
            self.undo.push(cur);
        }
        self.apply(forward)
    }

    pub fn reopen_closed(&mut self) -> Option<SavedLayout> {
        let cur = self.current.clone()?;
        let mut layout = cur.layout;
        let closed = self.closed.pop()?;
        let id: PaneId = layout.layout.panes().into_iter().max().map_or(1, |m| m + 1);
        let bounds = Rect::new(0.0, 0.0, 100_000.0, 100_000.0);
        let target = layout.layout.focus()?;
        if !layout.layout.split(target, Axis::Row, id, bounds) {
            return None;
        }
        if let Some(agent) = closed.agent {
            layout.agents.insert(id, agent);
        }
        if let Some(tool) = closed.tool {
            layout.tools.get_or_insert_with(Default::default).insert(id, tool);
        }
        self.change_to(Snapshot { at: cur.at, layout })
    }

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

    fn one(sid: &str) -> SavedLayout {
        SavedLayout {
            layout: Layout::new(1),
            agents: BTreeMap::from([(1, AgentCliPane::with_provider("claude", None, Some(sid.to_string())))]),
            ..Default::default()
        }
    }

    fn two() -> SavedLayout {
        let mut layout = Layout::new(1);
        layout.split(1, Axis::Row, 2, Rect::new(0.0, 0.0, 2000.0, 1000.0));
        SavedLayout {
            layout,
            agents: BTreeMap::from([
                (1, AgentCliPane::with_provider("claude", None, Some("a".into()))),
                (2, AgentCliPane::with_provider("grok", None, Some("b".into()))),
            ]),
            ..Default::default()
        }
    }

    #[test]
    fn undo_redo_and_reopen_work_on_agent_cli_layouts() {
        let mut h = History::default();
        assert!(h.record(0, &two()));
        h.record(100, &one("a"));
        assert!(h.can_undo() && h.can_reopen());
        let reopened = h.reopen_closed().unwrap();
        assert_eq!(reopened.layout.panes().len(), 2);
        assert!(reopened.agents.values().any(|a| a.session_id.as_deref() == Some("b")));
        h.record(101, &reopened);
        assert_eq!(h.undo(), Some(one("a")));
    }
}
