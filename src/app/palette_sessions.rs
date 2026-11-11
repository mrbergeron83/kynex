//! The palette's session rows: each workspace's past sessions, and the open ones the listing
//! doesn't have yet.

use std::collections::HashMap;
use std::path::PathBuf;

use kynex_core::fmt::ago;
use kynex_core::palette::{Entry, Kind, State};
use kynex_core::session::{self, Item, SessionInfo};
use kynex_core::workspace;

use super::KynexApp;
use super::palette::{Folder, Row, Target};
use crate::ui::Lamp;

impl KynexApp {
    /// The rows and entries for sessions, in `folders`' order.
    pub(super) fn palette_session_rows(
        &self,
        folders: &[Folder],
        sessions: &HashMap<PathBuf, Vec<SessionInfo>>,
    ) -> (Vec<Row>, Vec<Entry>) {
        let mut rows = Vec::new();
        let mut entries = Vec::new();
        let state_of = |o: &session::Session| match Lamp::of(o.busy, o.waiting(), o.finished) {
            Lamp::Idle => State::Idle,
            Lamp::Working => State::Working,
            Lamp::NeedsYou => State::NeedsYou,
            Lamp::Done => State::Done,
        };
        for f in folders.iter() {
            let live = self.code.get(&f.project);
            for s in sessions.get(&f.path).into_iter().flatten() {
                let open = live.and_then(|l| l.sessions.iter().find(|o| o.session_id.as_deref() == Some(&s.id)));
                let state = open.map(state_of);
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
        }
        // Open sessions the listing doesn't have (new ones, with no log on disk yet), in their
        // workspace's group. One with an id waits for its folder's listing, so it isn't shown twice.
        for f in folders.iter().filter(|f| f.section.workspace == 0) {
            let Some(live) = self.code.get(&f.project) else { continue };
            for o in &live.sessions {
                let home = folders
                    .iter()
                    .find(|g| g.project == f.project && workspace::same_folder(&g.path, &o.cwd))
                    .unwrap_or(f);
                if let Some(id) = &o.session_id {
                    let listed = |g: &Folder| sessions.get(&g.path).map(|l| l.iter().any(|i| i.id == *id));
                    let all = folders.iter().filter(|g| g.project == f.project);
                    if all.clone().any(|g| listed(g) == Some(true)) || listed(home) != Some(false) {
                        continue;
                    }
                }
                let title = o.items.iter().find_map(|i| match i {
                    Item::User { text, .. } if !text.is_empty() => {
                        Some(text.split_whitespace().collect::<Vec<_>>().join(" "))
                    }
                    _ => None,
                });
                let state = state_of(o);
                rows.push(Row {
                    target: Target::Open { project: f.project.clone(), workspace: home.path.clone(), key: o.uid },
                    detail: "".into(),
                    hint: state.word().into(),
                });
                entries.push(Entry {
                    kind: Kind::Session,
                    title: title.unwrap_or_else(|| "New session".into()),
                    keywords: home.project_name.clone(),
                    when: Some(std::time::SystemTime::now()),
                    section: Some(home.section.clone()),
                    state: Some(state),
                });
            }
        }
        // Agent panes, with their state read off their screens, so Go To's filters find them.
        for f in folders.iter() {
            for (pane, name, lamp) in self.agents_in(&f.path) {
                let state = match lamp {
                    Lamp::Idle => State::Idle,
                    Lamp::Working => State::Working,
                    Lamp::NeedsYou => State::NeedsYou,
                    Lamp::Done => State::Done,
                };
                rows.push(Row {
                    target: Target::AgentPane { project: f.project.clone(), workspace: f.path.clone(), pane },
                    detail: "agent pane".into(),
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
