//! One layout for every workspace: switching workspace keeps the panes where they are (tabs,
//! splits, sizes, tool panes) and only changes what they show.

use std::collections::HashMap;

use super::{AgentCliPane, Content, PaneId, TermPane, Tiling, Tool};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    AgentCli,
    Terminal,
    Git,
    Tree,
    Agents,
}

impl Tiling {
    fn kind(&self, pane: PaneId) -> Option<Kind> {
        Some(match self.content(pane)? {
            Content::AgentCli | Content::ClaudeChat => Kind::AgentCli,
            Content::Terminal => Kind::Terminal,
            Content::Git => Kind::Git,
            Content::Tree => Kind::Tree,
            Content::Agents => Kind::Agents,
        })
    }

    fn shape(&self) -> (crate::layout::Layout, Vec<Option<Kind>>) {
        let panes = self.layout.panes();
        let index: HashMap<PaneId, PaneId> = panes.iter().zip(0..).map(|(p, i)| (*p, i)).collect();
        let mut layout = self.layout.clone();
        layout.renumber(|p| index.get(&p).copied().unwrap_or(p));
        (layout, panes.iter().map(|p| self.kind(*p)).collect())
    }

    /// Whether this tiling looks like `other`: the same tabs, splits, sizes, focus and pane kinds.
    pub fn same_shape(&self, other: &Tiling) -> bool {
        self.shape() == other.shape()
    }

    /// Takes the shape of `from` (another workspace's tiling), reusing this workspace's existing
    /// `agent_cli` and terminal panes in reading order, and filling extra `agent_cli` slots with
    /// pending provider-picker panes.
    pub fn adopt_shape(&mut self, from: &Tiling, next_pane: &mut PaneId) {
        let slots = from.layout.panes();
        let focus_slot = from.agent_focus();
        let mine = self.layout.panes();
        let mut old_agents: Vec<(PaneId, AgentCliPane)> =
            mine.iter().filter_map(|p| Some((*p, self.agents.get(p)?.clone()))).collect();
        old_agents.reverse();
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
        let mut renamed: HashMap<PaneId, PaneId> = HashMap::new();
        let mut agents: HashMap<PaneId, AgentCliPane> = HashMap::new();
        let mut tools: HashMap<PaneId, Tool> = HashMap::new();
        for slot in &slots {
            match from.tools.get(slot) {
                None => {
                    let (id, agent) = old_agents.pop().unwrap_or_else(|| (fresh(), AgentCliPane::pending()));
                    renamed.insert(*slot, id);
                    agents.insert(id, agent);
                }
                Some(Tool::Terminal(_)) => {
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
        self.git_spot = from.git_spot.as_ref().and_then(|s| s.renamed(|p| renamed.get(&p).copied()));
        self.last_agent = focus_slot.and_then(|s| renamed.get(&s).copied());
        self.layout = layout;
        self.agents = agents;
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

    fn wide() -> Tiling {
        let mut t = Tiling::new(1, AgentCliPane::with_provider("claude", None, Some("s1".into())));
        assert!(t.split_agent(Axis::Row, 2, AgentCliPane::with_provider("grok", None, None), bounds()));
        assert!(t.add_terminal(3, 0.7, bounds()));
        t
    }

    #[test]
    fn takes_the_shape_and_keeps_its_own_agents() {
        let a = wide();
        let mut b = Tiling::new(10, AgentCliPane::with_provider("agy", None, Some("b1".into())));
        let mut next = 50;
        b.adopt_shape(&a, &mut next);
        assert!(b.same_shape(&a));
        assert_eq!(b.agent_panes().len(), 2);
        assert_eq!(b.agent(10).unwrap().session_id.as_deref(), Some("b1"));
        assert_eq!(b.terminals().len(), 1);
        assert!(b.panes().iter().all(|p| !a.panes().contains(p)));
    }
}
