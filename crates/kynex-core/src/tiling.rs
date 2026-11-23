//! What one workspace shows: its [`Layout`] (one tree of split panes) and what each pane holds.
//!
//! A pane holds either an interactive agent CLI ([`AgentCliPane`], or its pending New Session
//! provider picker while `provider` is `None`) or a tool: a plain/custom-command terminal, the
//! workspace's Git view, the Workspaces & Sessions tree, or the Agents overview pane.

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

use crate::layout::{Axis, Direction, Layout, PaneId, Rect, Rects, Spot};

/// One `agent_cli` pane: either showing the inline provider picker (`provider: None`) or
/// running a pure interactive provider CLI inside a GPU terminal.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentCliPane {
    /// Canonical provider ID (`claude`, `claude-openrouter`, `agy`, `agy-openrouter`, `grok`,
    /// `codex`…). `None` while the pane shows the New Session provider picker.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Optional model override (e.g. for OpenRouter overrides on `claude` or `agy`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Resumable session/conversation ID, reported by native hooks or discovered off-screen.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Optional display title for the pane header and Agents pane.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

impl AgentCliPane {
    /// A fresh pane showing the New Session provider picker.
    pub fn pending() -> Self {
        Self::default()
    }

    /// A pane launched on `provider` (with optional `model` and `session_id` to resume).
    pub fn with_provider(provider: impl Into<String>, model: Option<String>, session_id: Option<String>) -> Self {
        Self { provider: Some(provider.into()), model, session_id, title: None }
    }

    pub fn is_pending(&self) -> bool {
        self.provider.is_none()
    }
}

/// What a non-agent terminal pane is: a plain shell or a custom command.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TermPane {
    /// The custom command it runs ([`crate::user_commands`]), by name; `None` for a shell.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Where its scrollback was kept at the last quit ([`crate::scrollback`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history: Option<String>,
    /// Legacy fields from releases where agent panes lived inside `TermPane`: read only during
    /// [`Tiling::restore`] migration, never written back.
    #[doc(hidden)]
    #[serde(skip_serializing)]
    pub legacy_agent: Option<String>,
    #[doc(hidden)]
    #[serde(rename = "agent", skip_serializing)]
    pub old_agent: Option<String>,
    #[doc(hidden)]
    #[serde(rename = "resume", skip_serializing)]
    pub old_resume: Option<String>,
}

impl TermPane {
    /// A plain shell, which Ctrl+` uses.
    pub fn is_shell(&self) -> bool {
        self.command.is_none()
    }
}

/// What a pane holds besides an `agent_cli` pane, as saved.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Tool {
    Terminal(TermPane),
    /// The workspace's changes: its Git view.
    Git,
    /// The tree: the project's workspaces with their sessions under them.
    #[serde(alias = "Sessions")]
    Tree,
    /// What runs or wants you, across workspaces and projects.
    Agents,
    /// Legacy tool panes from older releases: dropped or migrated on [`Tiling::restore`].
    #[doc(hidden)]
    #[serde(rename = "Workspaces")]
    LegacyWorkspaces,
    #[doc(hidden)]
    #[serde(rename = "Timeline")]
    LegacyTimeline,
}

impl Tool {
    pub fn content(&self) -> Content {
        match self {
            Self::Terminal(_) => Content::Terminal,
            Self::Git => Content::Git,
            Self::Tree | Self::LegacyWorkspaces => Content::Tree,
            Self::Agents => Content::Agents,
            Self::LegacyTimeline => Content::AgentCli,
        }
    }
}

/// What a pane holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Content {
    AgentCli,
    ClaudeChat,
    Terminal,
    Git,
    Tree,
    Agents,
}

/// Where a terminal goes when it's shown and the workspace has none: below everything.
pub const TERMINAL_RATIO: f32 = 0.7;

/// A [`Tiling`] as it is saved to `workspaces.json`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedLayout {
    pub layout: Layout,
    /// Every `agent_cli` pane, by pane id.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub agents: BTreeMap<PaneId, AgentCliPane>,
    /// Legacy Claude session ids from older releases: migrated into `agents` on restore, never written.
    #[serde(skip_serializing)]
    pub sessions: BTreeMap<PaneId, String>,
    /// The tool panes (terminals, Git, Tree, Agents).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<BTreeMap<PaneId, Tool>>,
    /// The tree pane was seeded once; closing it stays closed.
    pub seeded: bool,
    /// Where the Git pane was when it was closed last, for [`Tiling::open_git`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_spot: Option<Spot>,
    /// The tab the Git pane shows.
    #[serde(skip_serializing_if = "GitTab::is_changes")]
    pub git_tab: GitTab,
}

/// The Git pane's tabs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum GitTab {
    History,
    Branches,
    #[default]
    #[serde(other)]
    Changes,
}

impl GitTab {
    fn is_changes(&self) -> bool {
        *self == Self::Changes
    }
}

impl SavedLayout {
    /// Whether it can be restored: a layout that holds together and has a pane.
    pub fn is_valid(&self) -> bool {
        !self.layout.is_empty() && self.layout.is_valid()
    }
}

pub struct Tiling {
    layout: Layout,
    agents: HashMap<PaneId, AgentCliPane>,
    tools: HashMap<PaneId, Tool>,
    /// The `agent_cli` pane focused last.
    last_agent: Option<PaneId>,
    /// The tabs got their tree pane once.
    seeded: bool,
    /// Where the Git pane was when it was closed last.
    git_spot: Option<Spot>,
    /// The tab the Git pane shows.
    git_tab: GitTab,
}

impl Tiling {
    /// What to save: the layout, every `agent_cli` pane, and the tool panes.
    pub fn save(&self) -> SavedLayout {
        let agents = self.layout.panes().into_iter().filter_map(|p| Some((p, self.agents.get(&p)?.clone()))).collect();
        let tools = self.tools.iter().map(|(p, t)| (*p, t.clone())).collect();
        SavedLayout {
            layout: self.layout.clone(),
            agents,
            sessions: BTreeMap::new(),
            tools: Some(tools),
            seeded: self.seeded,
            git_spot: self.git_spot.clone(),
            git_tab: self.git_tab,
        }
    }

    /// Brings a saved layout back, migrating legacy `sessions` / `TermPane.agent` / `Workspaces` /
    /// `Timeline` panes automatically. `resolve_provider` maps a legacy session id to its provider
    /// ID (e.g. from `sessions.json`). Returns `None` if the layout is invalid or has no `agent_cli`
    /// panes left.
    pub fn restore(
        saved: &SavedLayout,
        next_pane: &mut PaneId,
        mut resolve_provider: impl FnMut(&str) -> String,
    ) -> Option<Self> {
        if !saved.is_valid() {
            return None;
        }
        let old = saved.layout.panes();
        let start = *next_pane;
        let renamed: HashMap<PaneId, PaneId> = old.iter().zip(start..).map(|(p, n)| (*p, n)).collect();
        *next_pane = start + old.len() as PaneId;
        let old_of: HashMap<PaneId, PaneId> = renamed.iter().map(|(o, n)| (*n, *o)).collect();
        let mut layout = saved.layout.clone();
        layout.renumber(|p| renamed[&p]);

        // Migrate legacy Workspaces panes into a single Tree pane, drop LegacyTimeline, and
        // deduplicate singleton tool panes (Tree, Git, Agents) merged from legacy multi-tab layouts.
        let mut migrated_tools: HashMap<PaneId, Tool> = HashMap::new();
        let panes = layout.panes();
        let tool_of = |p: &PaneId| saved.tools.as_ref().and_then(|t| t.get(&old_of[p]));
        let mut kept_tree = false;
        let mut kept_git = false;
        let mut kept_agents = false;
        // Prefer an explicit Tree pane over LegacyWorkspaces if both exist.
        let has_tree = panes.iter().any(|p| matches!(tool_of(p), Some(Tool::Tree)));
        for p in panes {
            match tool_of(&p) {
                Some(Tool::Tree) => {
                    if kept_tree {
                        layout.close_pane(p);
                    } else {
                        kept_tree = true;
                    }
                }
                Some(Tool::LegacyWorkspaces) => {
                    if has_tree || kept_tree {
                        layout.close_pane(p);
                    } else {
                        migrated_tools.insert(p, Tool::Tree);
                        kept_tree = true;
                    }
                }
                Some(Tool::Git) => {
                    if kept_git {
                        layout.close_pane(p);
                    } else {
                        kept_git = true;
                    }
                }
                Some(Tool::Agents) => {
                    if kept_agents {
                        layout.close_pane(p);
                    } else {
                        kept_agents = true;
                    }
                }
                Some(Tool::LegacyTimeline) => {
                    layout.close_pane(p);
                }
                _ => {}
            }
        }

        let mut agents: HashMap<PaneId, AgentCliPane> = HashMap::new();
        let mut tools: HashMap<PaneId, Tool> = HashMap::new();
        let mut seen_sessions: Vec<String> = Vec::new();
        for p in old {
            let pane = renamed[&p];
            if !layout.panes().contains(&pane) {
                continue;
            }
            if let Some(tool) = saved.tools.as_ref().and_then(|t| t.get(&p)) {
                match tool {
                    Tool::LegacyWorkspaces => {
                        if let Some(tree) = migrated_tools.remove(&pane) {
                            tools.insert(pane, tree);
                        }
                    }
                    Tool::LegacyTimeline => {}
                    Tool::Terminal(tp) if tp.old_agent.is_some() || tp.legacy_agent.is_some() => {
                        let id = tp.old_agent.clone().or_else(|| tp.legacy_agent.clone());
                        agents.insert(
                            pane,
                            AgentCliPane {
                                provider: id.map(|s| crate::agent_cli::normalize_provider_id(&s)),
                                model: None,
                                session_id: tp.old_resume.clone(),
                                title: None,
                            },
                        );
                    }
                    other => {
                        tools.insert(pane, other.clone());
                    }
                }
                continue;
            }
            let mut agent = if let Some(a) = saved.agents.get(&p) {
                let mut a = a.clone();
                a.provider = a.provider.as_deref().map(crate::agent_cli::normalize_provider_id);
                a
            } else if let Some(sid) = saved.sessions.get(&p) {
                AgentCliPane {
                    provider: Some(resolve_provider(sid)),
                    model: None,
                    session_id: Some(sid.clone()),
                    title: None,
                }
            } else {
                AgentCliPane::pending()
            };
            if let Some(sid) = &agent.session_id {
                if seen_sessions.contains(sid) {
                    agent.session_id = None;
                } else {
                    seen_sessions.push(sid.clone());
                }
            }
            agents.insert(pane, agent);
        }

        if agents.is_empty() {
            return None;
        }
        let git_spot = saved.git_spot.as_ref().and_then(|s| s.renamed(|p| renamed.get(&p).copied()));
        let last_agent = layout
            .focus()
            .filter(|f| agents.contains_key(f))
            .or_else(|| layout.panes().into_iter().find(|p| agents.contains_key(p)));

        (!layout.is_empty()).then_some(Self {
            layout,
            agents,
            tools,
            last_agent,
            seeded: saved.seeded,
            git_spot,
            git_tab: saved.git_tab,
        })
    }

    /// A fresh workspace with one `agent_cli` pane `pane`.
    pub fn new(pane: PaneId, agent: AgentCliPane) -> Self {
        Self {
            layout: Layout::new(pane),
            agents: HashMap::from([(pane, agent)]),
            tools: HashMap::new(),
            last_agent: Some(pane),
            seeded: false,
            git_spot: None,
            git_tab: GitTab::Changes,
        }
    }

    /// What pane `pane` holds.
    pub fn content(&self, pane: PaneId) -> Option<Content> {
        if let Some(agent) = self.agents.get(&pane) {
            if agent.provider.as_deref().is_some_and(crate::agent_cli::is_claude_chat) {
                Some(Content::ClaudeChat)
            } else {
                Some(Content::AgentCli)
            }
        } else {
            self.tools.get(&pane).map(Tool::content)
        }
    }

    pub fn is_agent(&self, pane: PaneId) -> bool {
        self.agents.contains_key(&pane)
    }

    pub fn is_terminal(&self, pane: PaneId) -> bool {
        self.content(pane) == Some(Content::Terminal)
    }

    /// Whether pane `pane` holds a tool rather than an `agent_cli` pane.
    pub fn is_tool(&self, pane: PaneId) -> bool {
        self.tools.contains_key(&pane)
    }

    pub fn agent(&self, pane: PaneId) -> Option<&AgentCliPane> {
        self.agents.get(&pane)
    }

    pub fn agent_mut(&mut self, pane: PaneId) -> Option<&mut AgentCliPane> {
        self.agents.get_mut(&pane)
    }

    /// Every `agent_cli` pane across all tabs, in reading order.
    pub fn agent_panes(&self) -> Vec<(PaneId, &AgentCliPane)> {
        self.layout.panes().into_iter().filter_map(|p| Some((p, self.agents.get(&p)?))).collect()
    }

    /// The `agent_cli` panes in the active tab, in reading order.
    pub fn visible_agents(&self) -> Vec<(PaneId, &AgentCliPane)> {
        self.layout.visible().into_iter().filter_map(|p| Some((p, self.agents.get(&p)?))).collect()
    }

    /// The `agent_cli` pane currently holding `session_id`, in any tab.
    pub fn pane_of_session(&self, session_id: &str) -> Option<PaneId> {
        self.layout
            .panes()
            .into_iter()
            .find(|p| self.agents.get(p).and_then(|a| a.session_id.as_deref()) == Some(session_id))
    }

    /// Updates `pane`'s resumable session ID. Returns true if it changed.
    pub fn set_session_id(&mut self, pane: PaneId, id: String) -> bool {
        match self.agents.get_mut(&pane) {
            Some(a) if a.session_id.as_deref() != Some(id.as_str()) => {
                a.session_id = Some(id);
                true
            }
            _ => false,
        }
    }

    /// Sets `pane`'s provider & optional model/session (transitioning from pending picker to running).
    pub fn set_agent(&mut self, pane: PaneId, agent: AgentCliPane) -> bool {
        if !self.agents.contains_key(&pane) {
            return false;
        }
        self.agents.insert(pane, agent);
        true
    }

    /// The panes holding `kind`, in reading order.
    pub fn panes_of(&self, kind: Content) -> Vec<PaneId> {
        self.layout.panes().into_iter().filter(|p| self.content(*p) == Some(kind)).collect()
    }

    /// All terminal panes, in reading order.
    pub fn terminals(&self) -> Vec<PaneId> {
        self.panes_of(Content::Terminal)
    }

    /// The plain shell panes (not custom commands), in reading order.
    pub fn shell_terminals(&self) -> Vec<PaneId> {
        let shell = |p: &PaneId| matches!(self.tools.get(p), Some(Tool::Terminal(t)) if t.is_shell());
        self.panes_of(Content::Terminal).into_iter().filter(shell).collect()
    }

    /// What terminal pane `pane` is beyond being one.
    pub fn term_pane(&self, pane: PaneId) -> Option<&TermPane> {
        match self.tools.get(&pane) {
            Some(Tool::Terminal(t)) => Some(t),
            _ => None,
        }
    }

    /// Notes where terminal pane `pane`'s scrollback is kept.
    pub fn set_history(&mut self, pane: PaneId, id: String) {
        if let Some(Tool::Terminal(t)) = self.tools.get_mut(&pane) {
            t.history = Some(id);
        }
    }

    /// The custom command terminal pane `pane` runs, if it runs one.
    pub fn command_of(&self, pane: PaneId) -> Option<&str> {
        match self.tools.get(&pane) {
            Some(Tool::Terminal(t)) => t.command.as_deref(),
            _ => None,
        }
    }

    /// Every pane in sight, agents and tools, in reading order.
    pub fn visible_panes(&self) -> Vec<PaneId> {
        self.layout.visible()
    }

    /// The `agent_cli` pane focused now, or focused last, or the first one in the layout.
    pub fn agent_focus(&self) -> Option<PaneId> {
        let focus = self.focus().filter(|f| self.agents.contains_key(f));
        let last = self.last_agent.filter(|p| self.agents.contains_key(p));
        focus.or(last).or_else(|| self.layout.panes().into_iter().find(|p| self.agents.contains_key(p)))
    }

    /// A terminal pane `pane` below everything in the active tab, `ratio` of the height
    /// above it, and focused. False if there's no room.
    pub fn add_terminal(&mut self, pane: PaneId, ratio: f32, bounds: Rect) -> bool {
        if !self.layout.split_root(Axis::Column, pane, ratio, bounds) {
            return false;
        }
        self.tools.insert(pane, Tool::Terminal(TermPane::default()));
        true
    }

    /// A terminal pane `pane` at `side` of pane `target`, focused. False if there's no room.
    pub fn split_terminal(&mut self, target: PaneId, side: Direction, pane: PaneId, bounds: Rect) -> bool {
        self.split_tool(target, side, pane, Tool::Terminal(TermPane::default()), 0.5, bounds)
    }

    /// A pane `pane` holding `tool` at `side` of pane `target`, focused.
    pub fn split_tool(
        &mut self,
        target: PaneId,
        side: Direction,
        pane: PaneId,
        tool: Tool,
        ratio: f32,
        bounds: Rect,
    ) -> bool {
        if !self.place_with(target, side, pane, ratio, bounds) {
            return false;
        }
        self.tools.insert(pane, tool);
        true
    }

    /// Opens the workspace's Git pane as `pane`, focused.
    pub fn open_git(&mut self, pane: PaneId, bounds: Rect) -> bool {
        if !self.panes_of(Content::Git).is_empty() || self.layout.contains(pane) {
            return false;
        }
        let spot = self.git_spot.clone();
        if let Some(s) = &spot
            && self.layout.put_back(pane, s, bounds)
        {
            self.tools.insert(pane, Tool::Git);
            return true;
        }
        let panes = self.layout.panes();
        let agent = |p: &PaneId| self.agents.contains_key(p);
        let Some(target) =
            self.agent_focus().or_else(|| panes.iter().copied().find(agent)).or_else(|| panes.first().copied())
        else {
            return false;
        };
        let (side, ratio) = spot.map_or((Direction::Right, 0.5), |s| (s.side(), s.ratio()));
        self.split_tool(target, side, pane, Tool::Git, ratio, bounds)
    }

    pub fn git_tab(&self) -> GitTab {
        self.git_tab
    }

    pub fn set_git_tab(&mut self, tab: GitTab) {
        self.git_tab = tab;
    }

    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    pub fn has(&self, pane: PaneId) -> bool {
        self.layout.contains(pane)
    }

    pub fn focus(&self) -> Option<PaneId> {
        self.layout.focus()
    }

    pub fn panes(&self) -> Vec<PaneId> {
        self.layout.panes()
    }

    pub fn focus_pane(&mut self, pane: PaneId) -> bool {
        if !self.layout.set_focus(pane) {
            return false;
        }
        if self.agents.contains_key(&pane) {
            self.last_agent = Some(pane);
        }
        true
    }

    pub fn focus_dir(&mut self, dir: Direction, bounds: Rect) -> bool {
        let ok = self.layout.focus_dir(dir, bounds);
        if ok && let Some(f) = self.focus().filter(|f| self.agents.contains_key(f)) {
            self.last_agent = Some(f);
        }
        ok
    }

    pub fn swap_dir(&mut self, dir: Direction, bounds: Rect) -> bool {
        self.layout.swap_dir(dir, bounds)
    }

    pub fn move_pane(&mut self, pane: PaneId, target: PaneId, side: Option<Direction>, bounds: Rect) -> bool {
        self.layout.move_pane(pane, target, side, bounds) && self.moved(pane)
    }

    pub fn move_dir(&mut self, pane: PaneId, dir: Direction, bounds: Rect) -> bool {
        self.layout.move_dir(pane, dir, bounds) && self.moved(pane)
    }

    pub fn can_move_dir(&self, pane: PaneId, dir: Direction, bounds: Rect) -> bool {
        self.layout.clone().move_dir(pane, dir, bounds)
    }

    fn moved(&mut self, pane: PaneId) -> bool {
        if self.agents.contains_key(&pane) {
            self.last_agent = Some(pane);
        }
        true
    }

    pub fn resize_focused(&mut self, dir: Direction, px: f32, bounds: Rect) -> bool {
        let Some(focus) = self.layout.focus() else { return false };
        self.layout.resize(focus, dir, px, bounds)
    }

    pub fn rects(&self, bounds: Rect) -> Rects {
        self.layout.rects(bounds)
    }

    pub fn set_ratio(&mut self, path: &[bool], ratio: f32, bounds: Rect) -> bool {
        self.layout.set_ratio(path, ratio, bounds)
    }

    /// Splits the focused pane with a new `agent_cli` pane on its right (`Row`) or below (`Column`).
    pub fn split_agent(&mut self, axis: Axis, pane: PaneId, agent: AgentCliPane, bounds: Rect) -> bool {
        let Some(target) = self.focus() else { return false };
        if !self.layout.split(target, axis, pane, bounds) {
            return false;
        }
        self.agents.insert(pane, agent);
        self.last_agent = Some(pane);
        true
    }

    /// Puts a new `agent_cli` pane at `side` of `target`, and focuses it.
    pub fn split_agent_at(
        &mut self,
        target: PaneId,
        side: Direction,
        pane: PaneId,
        agent: AgentCliPane,
        bounds: Rect,
    ) -> bool {
        if !self.place(target, side, pane, bounds) {
            return false;
        }
        self.agents.insert(pane, agent);
        self.last_agent = Some(pane);
        true
    }

    fn place(&mut self, target: PaneId, side: Direction, pane: PaneId, bounds: Rect) -> bool {
        self.place_with(target, side, pane, 0.5, bounds)
    }

    fn place_with(&mut self, target: PaneId, side: Direction, pane: PaneId, ratio: f32, bounds: Rect) -> bool {
        let axis = match side {
            Direction::Left | Direction::Right => Axis::Row,
            Direction::Up | Direction::Down => Axis::Column,
        };
        if !self.layout.split_with(target, axis, pane, ratio, bounds) {
            return false;
        }
        if matches!(side, Direction::Left | Direction::Up) {
            self.layout.swap(target, pane);
        }
        true
    }

    pub fn zoomed(&self) -> Option<PaneId> {
        self.layout.zoomed()
    }

    pub fn toggle_zoom(&mut self) {
        self.layout.toggle_zoom();
    }

    /// Closes `pane` when the workspace keeps at least one agent pane (and >1 pane total).
    pub fn close_pane(&mut self, pane: PaneId) -> Option<Content> {
        let content = self.content(pane)?;
        let last_agent = matches!(content, Content::AgentCli | Content::ClaudeChat) && self.agents.len() < 2;
        if last_agent || self.layout.panes().len() < 2 {
            return None;
        }
        let spot = self.layout.spot(pane);
        if !self.layout.close_pane(pane) {
            return None;
        }
        if content == Content::Git && spot.is_some() {
            self.git_spot = spot;
        }
        self.agents.remove(&pane);
        self.tools.remove(&pane);
        if self.last_agent == Some(pane) {
            self.last_agent = self.agent_focus();
        }
        Some(content)
    }

    pub fn close_focused(&mut self) -> Option<PaneId> {
        let pane = self.focus()?;
        self.close_pane(pane).map(|_| pane)
    }
}

/// Which side of the pane at `r` a drop at (`x`, `y`) is for: the nearest edge when it is
/// within the outer 30%, else `None` (the middle: the pane itself).
pub fn drop_side(r: Rect, x: f32, y: f32) -> Option<Direction> {
    if r.w <= 0.0 || r.h <= 0.0 {
        return None;
    }
    let rx = (x - r.x) / r.w;
    let ry = (y - r.y) / r.h;
    let sides = [(Direction::Left, rx), (Direction::Right, 1.0 - rx), (Direction::Up, ry), (Direction::Down, 1.0 - ry)];
    sides.into_iter().filter(|(_, d)| *d < 0.3).min_by(|a, b| a.1.total_cmp(&b.1)).map(|(side, _)| side)
}

mod shape;
mod side;
#[cfg(test)]
mod tests;
