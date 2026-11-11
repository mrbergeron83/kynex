//! The palette's rows that come and go with what's open: moving the focused pane to another
//! tab, new agent panes, and the custom commands.

use kynex_core::palette::{Entry, Kind};

use super::KynexApp;
use super::palette::{Cmd, Row, Target};

impl KynexApp {
    /// Adds the moves, agents and custom commands to `rows` and `entries`.
    pub(super) fn palette_more_rows(&self, rows: &mut Vec<Row>, entries: &mut Vec<Entry>) {
        // The focused pane to another tab of its workspace.
        if let Some((_, ws, _)) = self.tiling_scope()
            && let Some(t) = self.tiles.get(&ws)
        {
            let tabs = t.tabs();
            let alone =
                t.focus().and_then(|f| t.layout().tab_of(f)).is_none_or(|i| t.layout().tabs()[i].panes().len() < 2);
            let mut moves = Vec::new();
            if !alone {
                moves.push((Cmd::MovePaneToNewTab, "Move pane to a new tab".to_string()));
            }
            for (i, tab) in tabs.iter().enumerate().filter(|(_, tab)| !tab.active) {
                moves.push((Cmd::MovePaneToTab(i), format!("Move pane to tab {}", tab.name)));
            }
            for (cmd, title) in moves {
                rows.push(Row { target: Target::Command(cmd), detail: "".into(), hint: "".into() });
                entries.push(Entry {
                    kind: Kind::Action,
                    title,
                    keywords: "pane move tab".into(),
                    when: None,
                    section: None,
                    state: None,
                });
            }
        }
        for r in &self.agent_rules {
            rows.push(Row { target: Target::Agent(r.id.clone()), detail: r.argv.join(" ").into(), hint: "".into() });
            entries.push(Entry {
                kind: Kind::Action,
                title: format!("New agent pane: {}", r.name),
                keywords: format!("agent cli {}", r.id),
                when: None,
                section: None,
                state: None,
            });
        }
        for c in &self.user_commands {
            rows.push(Row {
                target: Target::UserCommand(c.name.clone()),
                detail: c.argv.join(" ").into(),
                hint: c.key.clone().unwrap_or_default().into(),
            });
            entries.push(Entry {
                kind: Kind::Action,
                title: format!("Run {}", c.name),
                keywords: format!("custom command {}", c.argv.join(" ")),
                when: None,
                section: None,
                state: None,
            });
        }
    }
}
