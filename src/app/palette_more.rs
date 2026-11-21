//! The palette's dynamic rows: launching any provider CLI in a new `agent_cli` pane,
//! and custom commands.

use kynex_core::palette::{Entry, Kind};

use super::KynexApp;
use super::palette::{Row, Target};

impl KynexApp {
    /// Adds provider launches and custom commands to `rows` and `entries`.
    pub(super) fn palette_more_rows(&self, rows: &mut Vec<Row>, entries: &mut Vec<Entry>) {
        for p in self.provider_catalog() {
            rows.push(Row {
                target: Target::Agent(p.id.clone()),
                detail: p.description.clone().into(),
                hint: "".into(),
            });
            entries.push(Entry {
                kind: Kind::Action,
                title: format!("New session: {}", p.label),
                keywords: format!("agent cli provider {}", p.id),
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
