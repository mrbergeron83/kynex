//! Named layouts: a workspace's panes (tabs, splits, sizes, tools) kept under a name for every
//! workspace to apply, as files in `<data dir>/layouts/<name>.json` so they can be shared.
//!
//! A [`NamedLayout`] is a [`SavedLayout`] with what belongs to one workspace's history taken
//! out: no session ids (its session slots start new sessions), no terminal resume ids or
//! scrollback (terminals start fresh, running the same command or agent), no remembered Git
//! spot. Files come from outside, so [`parse`] checks them.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::layout::{Layout, PaneId};
use crate::tiling::{GitTab, SavedLayout, TermPane, Tool};

/// A layout file bigger than this isn't one.
const MAX_BYTES: usize = 256 * 1024;
/// More panes than this isn't a layout anyone drew.
const MAX_PANES: usize = 64;
/// Longest name, in characters.
const MAX_NAME: usize = 64;

/// A layout that can be applied to any workspace.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct NamedLayout {
    pub layout: Layout,
    /// The tool panes; every other pane is a session slot.
    #[serde(default)]
    pub tools: BTreeMap<PaneId, Tool>,
    #[serde(default)]
    pub git_tab: GitTab,
}

/// What `saved` looks like as a named layout.
pub fn export(saved: &SavedLayout) -> NamedLayout {
    let tools = saved
        .tools
        .iter()
        .flatten()
        .map(|(p, t)| {
            let tool = match t {
                Tool::Terminal(tp) => Tool::Terminal(TermPane {
                    command: tp.command.clone(),
                    agent: tp.agent.clone(),
                    ..TermPane::default()
                }),
                Tool::LegacyWorkspaces => Tool::Tree,
                other => other.clone(),
            };
            (*p, tool)
        })
        .collect();
    NamedLayout { layout: saved.layout.clone(), tools, git_tab: saved.git_tab }
}

/// The layout to restore for a workspace: `named`'s panes, each session slot a new session.
/// The tree isn't put in again: the layout says where it goes, or that it doesn't.
pub fn apply(named: &NamedLayout) -> SavedLayout {
    SavedLayout {
        layout: named.layout.clone(),
        sessions: BTreeMap::new(),
        tools: Some(named.tools.clone()),
        seeded: true,
        git_spot: None,
        git_tab: named.git_tab,
    }
}

impl NamedLayout {
    /// Whether it holds together: panes that make a layout, tools on panes it has, and a
    /// session slot (a workspace needs a session).
    pub fn is_valid(&self) -> bool {
        let panes = self.layout.panes();
        !panes.is_empty()
            && panes.len() <= MAX_PANES
            && self.layout.is_valid()
            && self.tools.keys().all(|p| panes.contains(p))
            && panes.iter().any(|p| !self.tools.contains_key(p))
    }

    /// A short description for a menu: "3 panes: 2 sessions, 1 terminal".
    pub fn summary(&self) -> String {
        let panes = self.layout.panes().len();
        let terminals = self.tools.values().filter(|t| matches!(t, Tool::Terminal(_))).count();
        let sessions = panes - self.tools.len().min(panes);
        let others = self.tools.len() - terminals;
        let mut parts = vec![format!("{sessions} session{}", plural(sessions))];
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
    for tool in named.tools.values_mut() {
        match tool {
            Tool::LegacyWorkspaces => *tool = Tool::Tree,
            Tool::Terminal(tp) => {
                tp.resume = None;
                tp.history = None;
            }
            _ => {}
        }
    }
    if !named.is_valid() {
        return Err("That layout is empty or broken, or has no session pane.".to_string());
    }
    Ok(named)
}

/// A name fit for a file name: trimmed, without path or reserved characters, not too long.
/// `None` if nothing is left.
pub fn clean_name(name: &str) -> Option<String> {
    let kept: String =
        name.trim().chars().map(|c| if c.is_control() || "<>:\"/\\|?*".contains(c) { '-' } else { c }).collect();
    let kept: String = kept.trim().trim_matches('.').trim().chars().take(MAX_NAME).collect();
    (!kept.is_empty() && !is_reserved(&kept)).then_some(kept)
}

/// A Windows device name (CON, NUL, COM1...), with or without an extension: refused on every OS
/// so a layout file can be shared to Windows.
fn is_reserved(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or("").trim_end().to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ["COM", "LPT"]
            .iter()
            .any(|p| stem.strip_prefix(p).is_some_and(|n| n.len() == 1 && matches!(n.as_bytes()[0], b'1'..=b'9')))
}

/// Where named layouts are kept.
pub fn dir() -> PathBuf {
    crate::paths::data_dir().join("layouts")
}

fn file_in(dir: &Path, name: &str) -> Option<PathBuf> {
    Some(dir.join(format!("{}.json", clean_name(name)?)))
}

/// The names of the layouts kept in `dir`, sorted. Blocks on the file system.
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

/// Keeps `layout` as `name` in `dir`, replacing one of that name. Blocks on the file system.
pub fn save_in(dir: &Path, name: &str, layout: &NamedLayout) -> Result<String, String> {
    let name = clean_name(name).ok_or("Give the layout a name.")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("Couldn't create {}: {e}", dir.display()))?;
    let path = file_in(dir, &name).ok_or("Give the layout a name.")?;
    crate::project::write_json(&path, layout)?;
    Ok(name)
}

/// The layout kept as `name` in `dir`. Blocks on the file system.
pub fn load_in(dir: &Path, name: &str) -> Result<NamedLayout, String> {
    let path = file_in(dir, name).ok_or("Give the layout a name.")?;
    let text = std::fs::read_to_string(&path).map_err(|e| format!("Couldn't read {}: {e}", path.display()))?;
    parse(&text)
}

/// Deletes the layout kept as `name` in `dir`. Blocks on the file system.
pub fn remove_in(dir: &Path, name: &str) -> Result<(), String> {
    let path = file_in(dir, name).ok_or("Give the layout a name.")?;
    std::fs::remove_file(&path).map_err(|e| format!("Couldn't delete {}: {e}", path.display()))
}

/// Writes `layout` to `path` (an export). Blocks on the file system.
pub fn export_to(path: &Path, layout: &NamedLayout) -> Result<(), String> {
    let s = serde_json::to_string_pretty(layout).map_err(|e| e.to_string())?;
    std::fs::write(path, s).map_err(|e| format!("Couldn't write {}: {e}", path.display()))
}

/// Reads a layout file from `path`, and its name (the file's). Blocks on the file system.
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
        let mut t = Tiling::new(1, 101);
        assert!(t.split(Axis::Row, 2, 102, B));
        assert!(t.add_terminal(3, 0.7, B));
        t.set_resume(3, "x".into());
        t.set_history(3, "scroll".into());
        let mut s = t.save(|k| Some(format!("id-{k}")));
        s.git_spot = None;
        s
    }

    #[test]
    fn export_drops_sessions_resume_and_history() {
        let named = export(&saved());
        assert!(named.is_valid());
        assert_eq!(named.layout.panes().len(), 3);
        let Some(Tool::Terminal(tp)) = named.tools.values().next() else { panic!("a terminal") };
        assert!(tp.resume.is_none() && tp.history.is_none());
        assert_eq!(named.summary(), "3 panes: 2 sessions, 1 terminal");
    }

    #[test]
    fn a_terminal_keeps_its_command_and_agent() {
        let mut s = saved();
        let tools = s.tools.as_mut().unwrap();
        tools.insert(
            3,
            Tool::Terminal(TermPane { command: Some("dev".into()), resume: Some("r".into()), ..Default::default() }),
        );
        let named = export(&s);
        assert_eq!(named.tools[&3], Tool::Terminal(TermPane { command: Some("dev".into()), ..Default::default() }));
    }

    #[test]
    fn applied_it_restores_with_new_sessions() {
        let applied = apply(&export(&saved()));
        assert!(applied.sessions.is_empty());
        let mut next = 50;
        let mut made = 0;
        let t = Tiling::restore(&applied, &mut next, None, |id| {
            assert!(id.is_none(), "no session is resumed");
            made += 1;
            Some(200 + made)
        })
        .expect("restores");
        assert_eq!(t.sessions().len(), 2);
        assert_eq!(t.terminals().len(), 1);
        assert!(t.term_pane(t.terminals()[0]).is_some_and(|tp| tp.is_shell()));
    }

    #[test]
    fn a_file_round_trips() {
        let named = export(&saved());
        let text = serde_json::to_string(&named).unwrap();
        assert_eq!(parse(&text).unwrap(), named);
    }

    #[test]
    fn bad_files_are_refused() {
        assert!(parse("").is_err());
        assert!(parse("{}").is_err());
        assert!(parse("[1,2]").is_err());
        let mut named = export(&saved());
        named.tools.insert(99, Tool::Git);
        assert!(parse(&serde_json::to_string(&named).unwrap()).is_err(), "a tool on a pane that isn't there");
        let mut named = export(&saved());
        for p in named.layout.panes() {
            named.tools.insert(p, Tool::Git);
        }
        assert!(parse(&serde_json::to_string(&named).unwrap()).is_err(), "no session slot");
        assert!(parse(&" ".repeat(MAX_BYTES + 1)).is_err());
    }

    #[test]
    fn names_are_made_safe() {
        assert_eq!(clean_name("  my layout "), Some("my layout".into()));
        assert_eq!(clean_name("a/b\\c:d"), Some("a-b-c-d".into()));
        assert_eq!(clean_name(".."), None);
        assert_eq!(clean_name("   "), None);
        for bad in ["CON", "con", "Nul", "aux.txt", "COM1", "lpt9.json", "prn.a.b", "Com5"] {
            assert_eq!(clean_name(bad), None, "{bad}");
        }
        assert_eq!(clean_name("com0"), Some("com0".into()));
        assert_eq!(clean_name("console"), Some("console".into()));
        assert_eq!(clean_name(&"x".repeat(200)).unwrap().chars().count(), MAX_NAME);
    }

    #[test]
    fn keeps_lists_loads_and_removes() {
        let dir = std::env::temp_dir().join(format!("kynex-layouts-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(list_in(&dir).is_empty());
        let named = export(&saved());
        assert_eq!(save_in(&dir, "Wide / dev", &named).unwrap(), "Wide - dev");
        save_in(&dir, "alpha", &named).unwrap();
        assert_eq!(list_in(&dir), vec!["alpha".to_string(), "Wide - dev".to_string()]);
        assert_eq!(load_in(&dir, "alpha").unwrap(), named);
        let out = dir.join("shared.json");
        export_to(&out, &named).unwrap();
        let (name, back) = import_from(&out).unwrap();
        assert_eq!((name.as_str(), &back), ("shared", &named));
        remove_in(&dir, "alpha").unwrap();
        assert!(load_in(&dir, "alpha").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
