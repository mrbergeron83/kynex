//! Named layouts: a workspace's panes (tabs, splits, sizes, providers, tools) kept under a
//! name for every workspace to apply, as files in `<data dir>/layouts/<name>.json`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::layout::{Layout, PaneId};
use crate::tiling::{AgentCliPane, GitTab, SavedLayout, TermPane, Tool};

const MAX_BYTES: usize = 256 * 1024;
const MAX_PANES: usize = 64;
const MAX_NAME: usize = 64;

/// A layout that can be applied to any workspace.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NamedLayout {
    pub layout: Layout,
    /// `agent_cli` pane slots (keeping provider/model if set, never session_id).
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub agents: BTreeMap<PaneId, AgentCliPane>,
    /// The tool panes; any pane not in `tools` is an `agent_cli` slot.
    pub tools: BTreeMap<PaneId, Tool>,
    pub git_tab: GitTab,
}

/// What `saved` looks like as a named layout (strips per-workspace session IDs and scrollback).
pub fn export(saved: &SavedLayout) -> NamedLayout {
    let agents = saved
        .agents
        .iter()
        .map(|(p, a)| {
            (*p, AgentCliPane { provider: a.provider.clone(), model: a.model.clone(), session_id: None, title: None })
        })
        .collect();
    let tools = saved
        .tools
        .iter()
        .flatten()
        .filter_map(|(p, t)| {
            let tool = match t {
                Tool::Terminal(tp) => Tool::Terminal(TermPane { command: tp.command.clone(), ..TermPane::default() }),
                Tool::LegacyWorkspaces => Tool::Tree,
                Tool::LegacyTimeline => return None,
                other => other.clone(),
            };
            Some((*p, tool))
        })
        .collect();
    NamedLayout { layout: saved.layout.clone(), agents, tools, git_tab: saved.git_tab }
}

/// The layout to restore for a workspace: `named`'s panes with fresh `agent_cli` slots.
pub fn apply(named: &NamedLayout) -> SavedLayout {
    SavedLayout {
        layout: named.layout.clone(),
        agents: named.agents.clone(),
        sessions: BTreeMap::new(),
        tools: Some(named.tools.clone()),
        seeded: true,
        git_spot: None,
        git_tab: named.git_tab,
    }
}

impl NamedLayout {
    pub fn is_valid(&self) -> bool {
        let panes = self.layout.panes();
        !panes.is_empty()
            && panes.len() <= MAX_PANES
            && self.layout.is_valid()
            && self.tools.keys().all(|p| panes.contains(p))
            && panes.iter().any(|p| !self.tools.contains_key(p))
    }

    pub fn summary(&self) -> String {
        let panes = self.layout.panes().len();
        let terminals = self.tools.values().filter(|t| matches!(t, Tool::Terminal(_))).count();
        let agents = panes - self.tools.len().min(panes);
        let others = self.tools.len() - terminals;
        let mut parts = vec![format!("{agents} agent{}", plural(agents))];
        if terminals > 0 {
            parts.push(format!("{terminals} terminal{}", plural(terminals)));
        }
        if others > 0 {
            parts.push(format!("{others} other"));
        }
        format!("{panes} pane{}: {}", plural(panes), parts.join(", "))
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// Reads a layout file's text. Never panics, whatever the text.
pub fn parse(text: &str) -> Result<NamedLayout, String> {
    if text.len() > MAX_BYTES {
        return Err("That file is too big to be a layout.".to_string());
    }
    let mut named: NamedLayout =
        serde_json::from_str(text).map_err(|e| format!("That isn't a Kynex layout file: {e}"))?;
    named.tools.retain(|_, t| !matches!(t, Tool::LegacyTimeline));
    for tool in named.tools.values_mut() {
        match tool {
            Tool::LegacyWorkspaces => *tool = Tool::Tree,
            Tool::Terminal(tp) => {
                tp.history = None;
                tp.old_resume = None;
            }
            _ => {}
        }
    }
    for agent in named.agents.values_mut() {
        agent.session_id = None;
    }
    if !named.is_valid() {
        return Err("That layout is empty or broken, or has no agent pane.".to_string());
    }
    Ok(named)
}

pub fn clean_name(name: &str) -> Option<String> {
    let kept: String =
        name.trim().chars().map(|c| if c.is_control() || "<>:\"/\\|?*".contains(c) { '-' } else { c }).collect();
    let kept: String = kept.trim().trim_matches('.').trim().chars().take(MAX_NAME).collect();
    (!kept.is_empty() && !is_reserved(&kept)).then_some(kept)
}

fn is_reserved(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or("").trim_end().to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ["COM", "LPT"]
            .iter()
            .any(|p| stem.strip_prefix(p).is_some_and(|n| n.len() == 1 && matches!(n.as_bytes()[0], b'1'..=b'9')))
}

pub fn dir() -> PathBuf {
    crate::paths::data_dir().join("layouts")
}

fn file_in(dir: &Path, name: &str) -> Option<PathBuf> {
    Some(dir.join(format!("{}.json", clean_name(name)?)))
}

pub fn list_in(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let path = e.path();
            (path.extension().is_some_and(|x| x == "json")).then(|| path.file_stem()?.to_str().map(str::to_string))?
        })
        .collect();
    names.sort_by_key(|n| n.to_lowercase());
    names
}

pub fn save_in(dir: &Path, name: &str, layout: &NamedLayout) -> Result<String, String> {
    let name = clean_name(name).ok_or("Give the layout a name.")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("Couldn't create {}: {e}", dir.display()))?;
    let path = file_in(dir, &name).ok_or("Give the layout a name.")?;
    crate::project::write_json(&path, layout)?;
    Ok(name)
}

pub fn load_in(dir: &Path, name: &str) -> Result<NamedLayout, String> {
    let path = file_in(dir, name).ok_or("Give the layout a name.")?;
    let text = std::fs::read_to_string(&path).map_err(|e| format!("Couldn't read {}: {e}", path.display()))?;
    parse(&text)
}

pub fn remove_in(dir: &Path, name: &str) -> Result<(), String> {
    let path = file_in(dir, name).ok_or("Give the layout a name.")?;
    std::fs::remove_file(&path).map_err(|e| format!("Couldn't delete {}: {e}", path.display()))
}

pub fn export_to(path: &Path, layout: &NamedLayout) -> Result<(), String> {
    let s = serde_json::to_string_pretty(layout).map_err(|e| e.to_string())?;
    std::fs::write(path, s).map_err(|e| format!("Couldn't write {}: {e}", path.display()))
}

pub fn import_from(path: &Path) -> Result<(String, NamedLayout), String> {
    let len = std::fs::metadata(path).map_err(|e| format!("Couldn't read {}: {e}", path.display()))?.len();
    if len > MAX_BYTES as u64 {
        return Err("That file is too big to be a layout.".to_string());
    }
    let text = std::fs::read_to_string(path).map_err(|e| format!("Couldn't read {}: {e}", path.display()))?;
    let name = path.file_stem().and_then(|s| s.to_str()).and_then(clean_name).unwrap_or_else(|| "Imported".to_string());
    Ok((name, parse(&text)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{Axis, Rect};
    use crate::tiling::Tiling;

    const B: Rect = Rect { x: 0.0, y: 0.0, w: 1600.0, h: 1000.0 };

    fn saved() -> SavedLayout {
        let mut t = Tiling::new(1, AgentCliPane::with_provider("claude", None, Some("s1".into())));
        assert!(t.split_agent(Axis::Row, 2, AgentCliPane::with_provider("grok", None, Some("s2".into())), B));
        assert!(t.add_terminal(3, 0.7, B));
        t.set_history(3, "scroll".into());
        let mut s = t.save();
        s.git_spot = None;
        s
    }

    #[test]
    fn export_drops_session_ids_and_history_but_keeps_providers() {
        let named = export(&saved());
        assert!(named.is_valid());
        assert_eq!(named.agents[&1].provider.as_deref(), Some("claude"));
        assert!(named.agents[&1].session_id.is_none());
        let Some(Tool::Terminal(tp)) = named.tools.values().next() else { panic!("a terminal") };
        assert!(tp.history.is_none());
        assert_eq!(named.summary(), "3 panes: 2 agents, 1 terminal");
    }

    #[test]
    fn applied_restores_fresh_agent_panes() {
        let applied = apply(&export(&saved()));
        let mut next = 50;
        let t = Tiling::restore(&applied, &mut next, |_| "claude".into()).expect("restores");
        assert_eq!(t.agent_panes().len(), 2);
        assert!(t.agent_panes().iter().all(|(_, a)| a.session_id.is_none()));
        assert_eq!(t.terminals().len(), 1);
    }

    #[test]
    fn bad_files_are_refused() {
        assert!(parse("").is_err());
        assert!(parse("{}").is_err());
        let mut named = export(&saved());
        named.tools.insert(99, Tool::Git);
        assert!(parse(&serde_json::to_string(&named).unwrap()).is_err());
    }
}
