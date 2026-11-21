//! The Agents pane's list: every `agent_cli` pane across all open projects that is working,
//! waiting on you, or finished unseen. Idle panes aren't listed.
//!
//! Waiting first, then finished, then working; inside each, ordered steadily by pane id so rows
//! don't jump as status updates arrive.

use crate::agent_panes::AgentState;
use crate::layout::PaneId;

/// Where a row stands, in the order rows are listed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum State {
    /// A permission prompt or question waits on you.
    Waiting,
    /// A turn ended and you haven't looked at it yet.
    Finished,
    /// Actively working.
    Working,
}

impl State {
    pub fn of_agent(state: AgentState) -> Option<Self> {
        match state {
            AgentState::NeedsYou => Some(Self::Waiting),
            AgentState::Done => Some(Self::Finished),
            AgentState::Working => Some(Self::Working),
            AgentState::Idle => None,
        }
    }
}

/// One active `agent_cli` pane, as the Agents pane lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub pane: PaneId,
    pub state: State,
    pub name: String,
    pub provider_badge: String,
    /// Its workspace, and project when that isn't the one shown.
    pub place: String,
    /// What it's doing, in a few words (hook detail or state label).
    pub doing: String,
}

/// Sorts `rows` into display order: Waiting → Finished → Working, steady by `pane` within each.
pub fn arrange(mut rows: Vec<Row>) -> Vec<Row> {
    rows.sort_by_key(|r| (r.state, r.pane));
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(pane: PaneId, state: State) -> Row {
        Row {
            pane,
            state,
            name: format!("Agent {pane}"),
            provider_badge: "Claude".into(),
            place: "kynex".into(),
            doing: String::new(),
        }
    }

    #[test]
    fn waiting_comes_first_then_finished_then_working() {
        let rows =
            vec![row(3, State::Working), row(7, State::Finished), row(2, State::Waiting), row(1, State::Working)];
        let ordered: Vec<PaneId> = arrange(rows).into_iter().map(|r| r.pane).collect();
        assert_eq!(ordered, [2, 7, 1, 3]);
    }

    #[test]
    fn idle_is_not_listed() {
        assert_eq!(State::of_agent(AgentState::Idle), None);
        assert_eq!(State::of_agent(AgentState::NeedsYou), Some(State::Waiting));
        assert_eq!(State::of_agent(AgentState::Done), Some(State::Finished));
        assert_eq!(State::of_agent(AgentState::Working), Some(State::Working));
    }
}
