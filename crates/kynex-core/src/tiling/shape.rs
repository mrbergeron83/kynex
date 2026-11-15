//! One layout for every workspace: switching workspace keeps the panes where they are (tabs,
//! splits, sizes, tool panes) and only changes what they show. The workspace you go to takes
//! the shape of the one you leave, its sessions filling the session panes.

use std::collections::HashMap;

use super::{Content, PaneId, TermPane, Tiling, Tool};
use crate::live::Key;

/// What a pane is, for comparing shapes: a session, or which tool.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Session,
    Terminal,
    Git,
    Timeline,
    Tree,
    Agents,
}

impl Tiling {
    fn kind(&self, pane: PaneId) -> Option<Kind> {
        Some(match self.content(pane)? {
            Content::Session(_) => Kind::Session,
            Content::Terminal => Kind::Terminal,
            Content::Git => Kind::Git,
            Content::Timeline => Kind::Timeline,
            Content::Tree => Kind::Tree,
            Content::Agents => Kind::Agents,
        })
    }

    /// The layout with its panes numbered by reading order, and what each holds: equal for
    /// two tilings that look the same whatever their pane ids and sessions.
    fn shape(&self) -> (crate::layout::Layout, Vec<Option<Kind>>) {
        let panes = self.layout.panes();
        let index: HashMap<PaneId, PaneId> = panes.iter().zip(0..).map(|(p, i)| (*p, i)).collect();
        let mut layout = self.layout.clone();
        layout.renumber(|p| index.get(&p).copied().unwrap_or(p));
        (layout, panes.iter().map(|p| self.kind(*p)).collect())
    }

    /// Whether this tiling looks like `other`: the same tabs, splits, sizes, focus and tools.
    pub fn same_shape(&self, other: &Tiling) -> bool {
        self.shape() == other.shape()
    }

    /// Takes the shape of `from` (another workspace's tiling): its tabs, splits, sizes,
    /// focus and tool panes. `current` goes into the pane `from`'s session state belongs to;
    /// the other session panes take the sessions this tiling showed, in order, then what
    /// `spare` gives (it gets the sessions placed so far; `None` closes the pane). Panes keep
    /// their ids where they keep their session, as do terminals (in order) and the other
    /// tools, so their state carries over; the rest get fresh ids from `*next_pane` up.
    /// Terminals beyond the ones this tiling had start as fresh shells.
    pub fn adopt_shape(
        &mut self,
        from: &Tiling,
        next_pane: &mut PaneId,
        current: Key,
        mut spare: impl FnMut(&[Key]) -> Option<Key>,
    ) {
        let slots = from.layout.panes();
        let focus_slot = from.session_focus();
        // What this tiling had, to reuse: sessions (in reading order, `current` aside) and tools.
        let mine = self.layout.panes();
        let mut old_sessions: Vec<(PaneId, Key)> =
            mine.iter().filter_map(|p| Some((*p, self.key_of(*p)?))).filter(|(_, k)| *k != current).collect();
        old_sessions.reverse();
        let current_pane = mine.iter().copied().find(|p| self.key_of(*p) == Some(current));
        let mut old_terms: Vec<(PaneId, Tool)> = mine
            .iter()
            .filter_map(|p| self.tools.get(p).filter(|t| matches!(t, Tool::Terminal(_))).map(|t| (*p, t.clone())))
            .collect();
        old_terms.reverse();
        let mut old_tools: Vec<(PaneId, Kind)> = mine
            .iter()
            .filter(|p| self.tools.get(p).is_some_and(|t| !matches!(t, Tool::Terminal(_))))
            .filter_map(|p| Some((*p, self.kind(*p)?)))
            .collect();

        let mut fresh = || {
            *next_pane += 1;
            *next_pane - 1
        };
        // `from`'s panes may share no ids with ours: fresh ones never do, and ours are ours.
        let mut renamed: HashMap<PaneId, PaneId> = HashMap::new();
        let mut keys: HashMap<PaneId, Key> = HashMap::new();
        let mut tools: HashMap<PaneId, Tool> = HashMap::new();
        let mut empty: Vec<PaneId> = Vec::new();
        let mut placed: Vec<Key> = vec![current];
        for slot in &slots {
            if Some(*slot) == focus_slot {
                let id = current_pane.unwrap_or_else(&mut fresh);
                renamed.insert(*slot, id);
                keys.insert(id, current);
                continue;
            }
            match from.tools.get(slot) {
                None => {
                    if let Some((p, k)) = old_sessions.pop() {
                        renamed.insert(*slot, p);
                        keys.insert(p, k);
                        placed.push(k);
                    } else if let Some(k) = spare(&placed) {
                        let id = fresh();
                        renamed.insert(*slot, id);
                        keys.insert(id, k);
                        placed.push(k);
                    } else {
                        let id = fresh();
                        renamed.insert(*slot, id);
                        empty.push(id);
                    }
                }
                Some(Tool::Terminal(_)) => {
                    // A new one is a shell: what `from`'s ran (a dev server, an agent) belongs there.
                    let (id, tool) = old_terms.pop().unwrap_or_else(|| (fresh(), Tool::Terminal(TermPane::default())));
                    renamed.insert(*slot, id);
                    tools.insert(id, tool);
                }
                Some(tool) => {
                    let kind = from.kind(*slot);
                    let id = match old_tools.iter().position(|(_, k)| Some(*k) == kind) {
                        Some(i) => old_tools.remove(i).0,
                        None => fresh(),
                    };
                    renamed.insert(*slot, id);
                    tools.insert(id, tool.clone());
                }
            }
        }
        let mut layout = from.layout.clone();
        layout.renumber(|p| renamed[&p]);
        for p in empty {
            layout.close_pane(p);
        }
        self.git_spot = from.git_spot.as_ref().and_then(|s| s.renamed(|p| renamed.get(&p).copied()));
        self.last_session = focus_slot.and_then(|s| renamed.get(&s).copied());
        self.layout = layout;
        self.keys = keys;
        self.tools = tools;
        self.seeded = from.seeded;
        self.git_tab = from.git_tab;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{Axis, Rect};

    fn bounds() -> Rect {
        Rect::new(0.0, 0.0, 1600.0, 1000.0)
    }

    /// Workspace A: two sessions side by side, a terminal below.
    fn wide() -> Tiling {
        let mut t = Tiling::new(1, 101);
        assert!(t.split(Axis::Row, 2, 102, bounds()));
        assert!(t.add_terminal(3, 0.7, bounds()));
        t
    }

    #[test]
    fn takes_the_shape_and_keeps_its_own_sessions() {
        let a = wide();
        let mut b = Tiling::new(10, 201);
        let mut next = 50;
        let mut spares = vec![202];
        b.adopt_shape(&a, &mut next, 201, |_| spares.pop());
        assert!(b.same_shape(&a));
        let sessions = b.sessions();
        assert!(sessions.contains(&201) && sessions.contains(&202));
        assert_eq!(b.key_of(10), Some(201), "the pane that had the current session keeps its id");
        assert_eq!(b.terminals().len(), 1);
        assert!(b.panes().iter().all(|p| !a.panes().contains(p)), "no pane id is shared with A");
        let focus = b.session_focus().unwrap();
        assert_eq!(b.key_of(focus), Some(201));
    }

    #[test]
    fn a_session_pane_with_nothing_to_show_closes() {
        let a = wide();
        let mut b = Tiling::new(10, 201);
        let mut next = 50;
        b.adopt_shape(&a, &mut next, 201, |_| None);
        assert_eq!(b.sessions(), vec![201]);
        assert_eq!(b.terminals().len(), 1);
    }

    #[test]
    fn its_terminals_carry_over() {
        let a = wide();
        let mut b = Tiling::new(10, 201);
        assert!(b.add_terminal(11, 0.5, bounds()));
        let mut next = 50;
        b.adopt_shape(&a, &mut next, 201, |_| None);
        assert_eq!(b.terminals(), vec![11]);
    }

    #[test]
    fn same_shape_ignores_ids_and_sessions() {
        let a = wide();
        let mut b = Tiling::new(7, 300);
        assert!(b.split(Axis::Row, 8, 301, bounds()));
        assert!(b.add_terminal(9, 0.7, bounds()));
        assert!(b.same_shape(&a));
        assert!(!Tiling::new(1, 1).same_shape(&a));
    }
}
