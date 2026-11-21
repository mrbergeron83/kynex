//! The palette's session rows: each workspace's past sessions on disk and its active
//! `agent_cli` panes with their live status lamps.

use std::collections::HashMap;
use std::path::PathBuf;

use kynex_core::fmt::ago;
use kynex_core::palette::{Entry, Kind, State};
use kynex_core::sessions::SessionInfo;

use super::KynexApp;
use super::palette::{Folder, Row, Target};
use crate::ui::Lamp;

impl KynexApp {
    /// The rows and entries for sessions and active `agent_cli` panes, in `folders`' order.
    pub(super) fn palette_session_rows(
        &self,
        folders: &[Folder],
        sessions: &HashMap<PathBuf, Vec<SessionInfo>>,
    ) -> (Vec<Row>, Vec<Entry>) {
        let mut rows = Vec::new();
        let mut entries = Vec::new();
        let lamp_state = |l: Lamp| match l {
            Lamp::Idle => State::Idle,
            Lamp::Working => State::Working,
            Lamp::NeedsYou => State::NeedsYou,
            Lamp::Done => State::Done,
        };
        for f in folders.iter() {
            let mut listed_ids: std::collections::HashSet<&str> = std::collections::HashSet::new();
            for s in sessions.get(&f.path).into_iter().flatten() {
                listed_ids.insert(s.id.as_str());
                let state = self.live_agent_of_session(&f.path, &s.id).map(|id| lamp_state(self.instance_lamp(id)));
                rows.push(Row {
                    target: Target::Session { project: f.project.clone(), workspace: f.path.clone(), id: s.id.clone() },
                    detail: ago(s.modified).into(),
                    hint: state.map_or("", State::word).into(),
                });
                entries.push(Entry {
                    kind: Kind::Session,
                    title: s.title.clone(),
                    keywords: f.project_name.clone(),
                    when: Some(s.modified),
                    section: Some(f.section.clone()),
                    state,
                });
            }
            // Live `agent_cli` instances in this workspace not already represented by a disk session row.
            for (agent_id, sid, name, lamp) in self.live_agents_in(&f.path) {
                if sid.as_deref().is_some_and(|id| listed_ids.contains(id)) {
                    continue;
                }
                let state = lamp_state(lamp);
                rows.push(Row {
                    target: Target::AgentPane(agent_id),
                    detail: "live session".into(),
                    hint: state.word().into(),
                });
                entries.push(Entry {
                    kind: Kind::Session,
                    title: name,
                    keywords: format!("agent {}", f.project_name),
                    when: Some(std::time::SystemTime::now()),
                    section: Some(f.section.clone()),
                    state: Some(state),
                });
            }
        }
        (rows, entries)
    }
}
