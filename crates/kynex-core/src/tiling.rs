//! What one workspace shows: its [`Layout`] (tabs of split panes) and the session each pane
//! holds. The app keeps this in step with the sessions that are open and the one the
//! window talks to ([`Live::current`](crate::live::Live::current)): picking a session
//! somewhere else (the sessions panel, a workspace) shows it in the focused pane, or
//! focuses the pane that already holds it.
//!
//! A pane holds a session or a tool: a terminal, the workspace's Git view, or the timeline
//! of the session pane focused last. Tool panes split, move and zoom like the others, but
//! sessions never land in them: picking a session while a tool has the focus shows it in
//! the session pane focused last. There is always a session pane.

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

use crate::layout::{Axis, Direction, Layout, PaneId, Rect, Rects};
use crate::live::Key;

/// What a terminal pane is, beyond being one.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TermPane {
    /// The custom command it runs ([`crate::user_commands`]), by name; `None` for a shell.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// The agent CLI it runs ([`crate::agent_panes`]), by rule id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// The id the agent printed to be resumed with, for after a restart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume: Option<String>,
}

impl TermPane {
    /// A plain shell, which Ctrl+` and the run buttons use.
    pub fn is_shell(&self) -> bool {
        self.command.is_none() && self.agent.is_none()
    }
}

/// What a pane holds besides a session, as saved.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Tool {
    Terminal(TermPane),
    /// The workspace's changes: its Git view.
    Git,
    /// The timeline of the session pane focused last.
    Timeline,
}

impl Tool {
    pub fn content(&self) -> Content {
        match self {
            Self::Terminal(_) => Content::Terminal,
            Self::Git => Content::Git,
            Self::Timeline => Content::Timeline,
        }
    }
}

/// What a pane holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Content {
    Session(Key),
    Terminal,
    Git,
    Timeline,
}

/// Where a terminal goes when it's shown and the tab has none: below everything.
pub const TERMINAL_RATIO: f32 = 0.7;

/// One tab of a workspace, for the tab bar.
#[derive(Clone, Debug, PartialEq)]
pub struct TabInfo {
    pub name: String,
    /// The sessions in its panes, in reading order.
    pub sessions: Vec<Key>,
    /// Every pane in it, in reading order.
    pub panes: Vec<PaneId>,
    pub active: bool,
}

/// A [`Tiling`] as it is saved: the layout, and the Claude session id each pane held
/// (a pane with no entry was a new session with nothing said yet). Session keys are
/// process-local, so ids are what survive a restart.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SavedLayout {
    pub layout: Layout,
    pub sessions: BTreeMap<PaneId, String>,
    /// The tool panes (terminals, Git, timeline). `None` in a layout saved before they were
    /// panes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<BTreeMap<PaneId, Tool>>,
}

impl SavedLayout {
    /// Whether it can be restored: a layout that holds together and has a pane.
    pub fn is_valid(&self) -> bool {
        !self.layout.is_empty() && self.layout.is_valid()
    }
}

pub struct Tiling {
    layout: Layout,
    keys: HashMap<PaneId, Key>,
    tools: HashMap<PaneId, Tool>,
    /// The session pane that had the focus last, where a session goes while a tool has it.
    last_session: Option<PaneId>,
    /// The session the window talked to at the last [`Self::reconcile`].
    shown: Option<Key>,
}

impl Tiling {
    /// What to save: the layout, from `id_of` each pane's session id (`None` for a session
    /// that has none to resume), and the tool panes.
    pub fn save(&self, id_of: impl Fn(Key) -> Option<String>) -> SavedLayout {
        let sessions = self.layout.panes().into_iter().filter_map(|p| Some((p, id_of(self.key_of(p)?)?))).collect();
        let tools = self.tools.iter().map(|(p, t)| (*p, t.clone())).collect();
        SavedLayout { layout: self.layout.clone(), sessions, tools: Some(tools) }
    }

    /// Brings a saved layout back. Its panes get fresh ids from `*next_pane` up (advancing
    /// it), so they can't collide with panes in use. `key_for` gives the session for a
    /// pane's saved id (`None` id: a new session); `None` back means it is gone and its
    /// pane closes, as does a pane whose session another pane has. `None` if the layout
    /// is broken or no pane is left. With `keep`, the pane that was focused takes that id
    /// instead (one the caller knows is free), for state the caller holds under it.
    pub fn restore(
        saved: &SavedLayout,
        next_pane: &mut PaneId,
        keep: Option<PaneId>,
        mut key_for: impl FnMut(Option<&str>) -> Option<Key>,
    ) -> Option<Self> {
        if !saved.is_valid() {
            return None;
        }
        let old = saved.layout.panes();
        let start = *next_pane;
        let renamed: HashMap<PaneId, PaneId> = old.iter().zip(start..).map(|(p, n)| (*p, n)).collect();
        *next_pane = start + old.len() as PaneId;
        let mut layout = saved.layout.clone();
        layout.renumber(|p| renamed[&p]);
        let mut keys: HashMap<PaneId, Key> = HashMap::new();
        let mut tools: HashMap<PaneId, Tool> = HashMap::new();
        let mut seen: Vec<&str> = Vec::new();
        for p in old {
            if let Some(tool) = saved.tools.as_ref().and_then(|t| t.get(&p)) {
                tools.insert(renamed[&p], tool.clone());
                continue;
            }
            let id = saved.sessions.get(&p).map(String::as_str);
            let repeat = id.is_some_and(|id| seen.contains(&id));
            seen.extend(id);
            let key = if repeat { None } else { key_for(id) };
            let pane = renamed[&p];
            match key {
                Some(k) if !keys.values().any(|x| *x == k) => {
                    keys.insert(pane, k);
                }
                _ => {
                    layout.close_pane(pane);
                }
            }
        }
        // Tools alone aren't a workspace: there has to be a session.
        if keys.is_empty() {
            return None;
        }
        // A tool can't keep the focus: the session state up belongs to a session pane.
        if let Some(focus) = layout.focus()
            && tools.contains_key(&focus)
            && let Some(p) = layout.tab().and_then(|t| t.panes().into_iter().find(|p| keys.contains_key(p)))
            && let Some(t) = layout.tab_mut()
        {
            t.set_focus(p);
        }
        // The focused session pane takes `keep`.
        let focus = layout.focus().filter(|f| keys.contains_key(f)).or_else(|| keys.keys().min().copied());
        if let Some(k) = keep
            && let Some(focus) = focus
            && !layout.panes().contains(&k)
        {
            layout.renumber(|p| if p == focus { k } else { p });
            if let Some(key) = keys.remove(&focus) {
                keys.insert(k, key);
            }
        }
        (!layout.is_empty()).then_some(Self { layout, keys, tools, last_session: None, shown: None })
    }

    /// One tab with one pane `pane`, showing session `key`.
    pub fn new(pane: PaneId, key: Key) -> Self {
        Self {
            layout: Layout::new("1", pane),
            keys: HashMap::from([(pane, key)]),
            tools: HashMap::new(),
            last_session: Some(pane),
            shown: None,
        }
    }

    /// What pane `pane` holds.
    pub fn content(&self, pane: PaneId) -> Option<Content> {
        match self.keys.get(&pane) {
            Some(k) => Some(Content::Session(*k)),
            None => self.tools.get(&pane).map(Tool::content),
        }
    }

    pub fn is_terminal(&self, pane: PaneId) -> bool {
        self.content(pane) == Some(Content::Terminal)
    }

    /// Whether pane `pane` holds a tool rather than a session.
    pub fn is_tool(&self, pane: PaneId) -> bool {
        self.tools.contains_key(&pane)
    }

    /// The panes holding `kind` in every tab, in reading order.
    pub fn panes_of(&self, kind: Content) -> Vec<PaneId> {
        self.layout.panes().into_iter().filter(|p| self.content(*p) == Some(kind)).collect()
    }

    /// The panes holding `kind` in the active tab, in reading order (a zoomed tab's other
    /// panes included).
    pub fn tab_panes_of(&self, kind: Content) -> Vec<PaneId> {
        let panes = self.layout.tab().map(|t| t.panes()).unwrap_or_default();
        panes.into_iter().filter(|p| self.content(*p) == Some(kind)).collect()
    }

    /// The terminal panes of every tab, in reading order.
    pub fn terminals(&self) -> Vec<PaneId> {
        self.panes_of(Content::Terminal)
    }

    /// The shell panes of the active tab (not a custom command's), in reading order.
    pub fn tab_terminals(&self) -> Vec<PaneId> {
        let shell = |p: &PaneId| matches!(self.tools.get(p), Some(Tool::Terminal(t)) if t.is_shell());
        self.tab_panes_of(Content::Terminal).into_iter().filter(shell).collect()
    }

    /// What terminal pane `pane` is beyond being one.
    pub fn term_pane(&self, pane: PaneId) -> Option<&TermPane> {
        match self.tools.get(&pane) {
            Some(Tool::Terminal(t)) => Some(t),
            _ => None,
        }
    }

    /// The agent panes of every tab, with their agents' ids.
    pub fn agents(&self) -> Vec<(PaneId, String)> {
        let of = |p: PaneId| Some((p, self.term_pane(p)?.agent.clone()?));
        self.layout.panes().into_iter().filter_map(of).collect()
    }

    /// Notes the id agent pane `pane` can be resumed with. True if it changed.
    pub fn set_resume(&mut self, pane: PaneId, id: String) -> bool {
        match self.tools.get_mut(&pane) {
            Some(Tool::Terminal(t)) if t.agent.is_some() && t.resume.as_deref() != Some(id.as_str()) => {
                t.resume = Some(id);
                true
            }
            _ => false,
        }
    }

    /// The custom command terminal pane `pane` runs, if it runs one.
    pub fn command_of(&self, pane: PaneId) -> Option<&str> {
        match self.tools.get(&pane) {
            Some(Tool::Terminal(t)) => t.command.as_deref(),
            _ => None,
        }
    }

    /// Every pane in sight, sessions and tools, in reading order.
    pub fn visible_panes(&self) -> Vec<PaneId> {
        self.layout.visible()
    }

    /// The pane the window's session state belongs to: the focused one if it holds a
    /// session, else the session pane focused last, else the first.
    pub fn session_focus(&self) -> Option<PaneId> {
        let focus = self.focus().filter(|f| self.keys.contains_key(f));
        let last = self.last_session.filter(|p| self.keys.contains_key(p));
        focus.or(last).or_else(|| self.layout.panes().into_iter().find(|p| self.keys.contains_key(p)))
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

    /// A terminal pane `pane` at `side` of pane `target` (a terminal beside a terminal, or
    /// one beside a session), and focused. False if there's no room.
    pub fn split_terminal(&mut self, target: PaneId, side: Direction, pane: PaneId, bounds: Rect) -> bool {
        self.split_tool(target, side, pane, Tool::Terminal(TermPane::default()), 0.5, bounds)
    }

    /// A pane `pane` holding `tool` at `side` of pane `target`, focused. `ratio` of the room
    /// goes to the first of the two (left or top). False if there's no room.
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

    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    pub fn has(&self, pane: PaneId) -> bool {
        self.layout.tab_of(pane).is_some()
    }

    pub fn key_of(&self, pane: PaneId) -> Option<Key> {
        self.keys.get(&pane).copied()
    }

    /// The focused pane of the active tab.
    pub fn focus(&self) -> Option<PaneId> {
        self.layout.focus()
    }

    /// The panes in sight, with their sessions, in reading order.
    pub fn visible(&self) -> Vec<(PaneId, Key)> {
        self.layout.visible().into_iter().filter_map(|p| Some((p, self.key_of(p)?))).collect()
    }

    /// The sessions in the panes of every tab, in reading order.
    pub fn sessions(&self) -> Vec<Key> {
        self.layout.panes().into_iter().filter_map(|p| self.key_of(p)).collect()
    }

    /// Every pane in every tab.
    pub fn panes(&self) -> Vec<PaneId> {
        self.layout.panes()
    }

    /// The tabs, in order, for the tab bar.
    pub fn tabs(&self) -> Vec<TabInfo> {
        let active = self.layout.active();
        self.layout
            .tabs()
            .iter()
            .enumerate()
            .map(|(i, t)| TabInfo {
                name: t.name.clone(),
                sessions: t.panes().into_iter().filter_map(|p| self.key_of(p)).collect(),
                panes: t.panes(),
                active: i == active,
            })
            .collect()
    }

    /// A new tab after the active one, holding `pane` (showing `key`), and active. It is
    /// named with the next number. False if `key` or `pane` is here already.
    pub fn new_tab(&mut self, pane: PaneId, key: Key) -> bool {
        if self.keys.values().any(|k| *k == key) {
            return false;
        }
        let name = self.next_tab_name();
        if !self.layout.new_tab(name, pane) {
            return false;
        }
        self.keys.insert(pane, key);
        true
    }

    /// Shows tab `i`. False if there is no such tab.
    pub fn select_tab(&mut self, i: usize) -> bool {
        self.layout.select(i)
    }

    /// Closes tab `i` unless it is the only one, or the only one with sessions. Returns the
    /// panes it held with their sessions (which stay open, off screen); its tools go.
    pub fn close_tab(&mut self, i: usize) -> Vec<(PaneId, Key)> {
        let Some(tab) = self.layout.tabs().get(i) else { return Vec::new() };
        let mine = tab.panes();
        let others_have_sessions = self.keys.keys().any(|p| !mine.contains(p));
        if self.layout.tabs().len() < 2 || !others_have_sessions {
            return Vec::new();
        }
        let panes = self.layout.close_tab(i);
        for p in &panes {
            self.tools.remove(p);
        }
        panes.into_iter().filter_map(|p| Some((p, self.keys.remove(&p)?))).collect()
    }

    /// Names tab `i`; a blank name leaves it as it was.
    pub fn rename_tab(&mut self, i: usize, name: &str) {
        let name = name.trim();
        if !name.is_empty() {
            self.layout.rename_tab(i, name);
        }
    }

    /// Moves tab `from` to place `to`; the active tab stays the same tab.
    pub fn move_tab(&mut self, from: usize, to: usize) {
        self.layout.move_tab(from, to);
    }

    /// Focuses `pane` (its tab comes up). False if it isn't here.
    pub fn focus_pane(&mut self, pane: PaneId) -> bool {
        let Some(i) = self.layout.tab_of(pane) else { return false };
        self.layout.select(i);
        if self.keys.contains_key(&pane) {
            self.last_session = Some(pane);
        }
        self.layout.tab_mut().is_some_and(|t| t.set_focus(pane))
    }

    /// Focuses the pane next to the focused one on the `dir` side. False if there is none.
    pub fn focus_dir(&mut self, dir: Direction, bounds: Rect) -> bool {
        self.layout.tab_mut().is_some_and(|t| t.focus_dir(dir, bounds))
    }

    /// Swaps the focused pane with its neighbour on the `dir` side (focus stays on the
    /// pane). False if there is none.
    pub fn swap_dir(&mut self, dir: Direction, bounds: Rect) -> bool {
        self.layout.tab_mut().is_some_and(|t| t.swap_dir(dir, bounds))
    }

    /// Grows the focused pane `px` towards `dir`. False if its edge that way is the tab's.
    pub fn resize_focused(&mut self, dir: Direction, px: f32, bounds: Rect) -> bool {
        let Some(t) = self.layout.tab_mut() else { return false };
        let focus = t.focus();
        t.resize(focus, dir, px, bounds)
    }

    /// Where the panes and dividers of the active tab sit in `bounds`.
    pub fn rects(&self, bounds: Rect) -> Rects {
        self.layout.tab().map(|t| t.rects(bounds)).unwrap_or_default()
    }

    /// Moves the divider of the split at `path` to `ratio` of its split (kept inside the panes'
    /// minimum sizes). False if there is no such split.
    pub fn set_ratio(&mut self, path: &[bool], ratio: f32, bounds: Rect) -> bool {
        self.layout.tab_mut().is_some_and(|t| t.set_ratio(path, ratio, bounds))
    }

    /// Splits the focused pane, the new one (showing `key`) on its right (`Row`) or below
    /// (`Column`) and focused. False if it would leave a pane too small for `bounds`, or
    /// `key` is on show already.
    pub fn split(&mut self, axis: Axis, pane: PaneId, key: Key, bounds: Rect) -> bool {
        if self.keys.values().any(|k| *k == key) {
            return false;
        }
        let Some(target) = self.focus() else { return false };
        if !self.layout.split(target, axis, pane, bounds) {
            return false;
        }
        self.keys.insert(pane, key);
        true
    }

    /// The pane showing session `key`, in any tab.
    pub fn pane_of(&self, key: Key) -> Option<PaneId> {
        self.layout.panes().into_iter().find(|p| self.key_of(*p) == Some(key))
    }

    /// Puts a new pane (showing `key`) at `side` of pane `target`, and focuses it. False if
    /// it would leave a pane too small for `bounds`, `target` isn't here or `key` is on show.
    pub fn split_at(&mut self, target: PaneId, side: Direction, pane: PaneId, key: Key, bounds: Rect) -> bool {
        if self.keys.values().any(|k| *k == key) || !self.place(target, side, pane, bounds) {
            return false;
        }
        self.keys.insert(pane, key);
        true
    }

    /// Splits `target` with `pane` at its `side`.
    fn place(&mut self, target: PaneId, side: Direction, pane: PaneId, bounds: Rect) -> bool {
        self.place_with(target, side, pane, 0.5, bounds)
    }

    /// [`Self::place`], `ratio` of the room going to the first of the two.
    fn place_with(&mut self, target: PaneId, side: Direction, pane: PaneId, ratio: f32, bounds: Rect) -> bool {
        let axis = match side {
            Direction::Left | Direction::Right => Axis::Row,
            Direction::Up | Direction::Down => Axis::Column,
        };
        if !self.layout.split_with(target, axis, pane, ratio, bounds) {
            return false;
        }
        // A split puts the new pane after its target; before it is the two swapped.
        if matches!(side, Direction::Left | Direction::Up)
            && let Some(t) = self.layout.tab_mut()
        {
            t.swap(target, pane);
        }
        true
    }

    /// Shows session `key` in pane `pane` in place of what it had. False if the pane isn't
    /// here, is a tool, or the session is in another pane.
    pub fn show_in(&mut self, pane: PaneId, key: Key) -> bool {
        if !self.has(pane) || self.is_tool(pane) || self.pane_of(key).is_some_and(|p| p != pane) {
            return false;
        }
        self.keys.insert(pane, key);
        true
    }

    /// The pane filling the active tab, if one is zoomed.
    pub fn zoomed(&self) -> Option<PaneId> {
        self.layout.tab().and_then(|t| t.zoomed())
    }

    /// Zooms the focused pane to fill its tab, or puts the others back.
    pub fn toggle_zoom(&mut self) {
        if let Some(t) = self.layout.tab_mut() {
            t.toggle_zoom();
        }
    }

    /// Closes `pane` when the window has others to keep (a session stays open, off screen),
    /// though never the last session pane. Returns what it held.
    pub fn close_pane(&mut self, pane: PaneId) -> Option<Content> {
        let content = self.content(pane)?;
        let last_session = matches!(content, Content::Session(_)) && self.keys.len() < 2;
        if last_session || self.layout.panes().len() < 2 || !self.layout.close_pane(pane) {
            return None;
        }
        self.keys.remove(&pane);
        self.tools.remove(&pane);
        Some(content)
    }

    /// Moves `pane` to tab `tab`, beside that tab's focused pane. False if it is there
    /// already or has no room.
    pub fn move_to_tab(&mut self, pane: PaneId, tab: usize, bounds: Rect) -> bool {
        self.layout.move_to_tab(pane, tab, bounds)
    }

    /// Moves `pane` to a new tab of its own. False if it is alone in its tab already.
    pub fn move_to_new_tab(&mut self, pane: PaneId) -> bool {
        let Some(from) = self.layout.tab_of(pane) else { return false };
        if self.layout.tabs()[from].panes().len() < 2 {
            return false;
        }
        let name = self.next_tab_name();
        self.layout.take(pane) && self.layout.new_tab(name, pane)
    }

    fn next_tab_name(&self) -> String {
        let numbers = self.layout.tabs().iter().filter_map(|t| t.name.parse::<usize>().ok());
        (numbers.max().unwrap_or(0).max(self.layout.tabs().len()) + 1).to_string()
    }

    /// Closes the focused pane when the window has others to keep (its session stays open,
    /// off screen). Returns the pane that closed.
    pub fn close_focused(&mut self) -> Option<PaneId> {
        let pane = self.focus()?;
        self.close_pane(pane).map(|_| pane)
    }

    /// Brings the panes in line with the sessions: a pane whose session closed goes (the
    /// last one stays), and `current`, the session the window talks to, ends up in the
    /// focused pane: focusing the pane already showing it, else taking the focused pane's
    /// place. Returns the panes that went.
    ///
    /// Tool panes stay. While one has the focus and `current` is the session it was last
    /// time, the focus stays on it; a session not on show goes into [`Self::session_focus`].
    pub fn reconcile(&mut self, open: &[Key], current: Key) -> Vec<PaneId> {
        let mut gone = Vec::new();
        for pane in self.layout.panes() {
            let Some(key) = self.key_of(pane) else { continue };
            if !open.contains(&key) && self.keys.len() > 1 && self.layout.close_pane(pane) {
                self.keys.remove(&pane);
                gone.push(pane);
            }
        }
        let on_tool = self.focus().is_some_and(|f| self.is_tool(f));
        let unchanged = self.shown == Some(current);
        self.shown = Some(current);
        match self.keys.iter().find(|(_, k)| **k == current).map(|(p, _)| *p) {
            Some(_) if on_tool && unchanged => {}
            Some(pane) => {
                self.focus_pane(pane);
            }
            None => {
                if let Some(pane) = self.session_focus() {
                    self.keys.insert(pane, current);
                    if !on_tool || !unchanged {
                        self.focus_pane(pane);
                    }
                }
            }
        }
        if let Some(f) = self.focus().filter(|f| self.keys.contains_key(f)) {
            self.last_session = Some(f);
        }
        gone
    }
}

/// Which side of the pane at `r` a drop at (`x`, `y`) is for: the nearest edge when it is
/// within the outer 30%, else `None` (the middle: the pane itself).
pub fn drop_side(r: Rect, x: f32, y: f32) -> Option<Direction> {
    if r.w <= 0.0 || r.h <= 0.0 {
        return None;
    }
    let (fx, fy) = ((x - r.x) / r.w, (y - r.y) / r.h);
    let sides = [(fx, Direction::Left), (1.0 - fx, Direction::Right), (fy, Direction::Up), (1.0 - fy, Direction::Down)];
    let (near, side) = sides.into_iter().min_by(|a, b| a.0.total_cmp(&b.0))?;
    (near < 0.3).then_some(side)
}

#[cfg(test)]
mod tests;
