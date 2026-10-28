//! What survives a restart: open projects, the active tab, the model, theme,
//! window placement. Saved to `<data dir>/state.json`.

use std::collections::HashMap;
use std::path::PathBuf;

use kynex_core::motion::Motion;
use kynex_core::paths::data_dir;
use kynex_core::terminal::ShellKind;
use serde::{Deserialize, Serialize};

use crate::theme::Theme;

/// Which top-level tab is showing. Projects sit at the top of the hierarchy,
/// with Chat pinned as the first tab.
#[derive(Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Active {
    #[default]
    Chat,
    Project(usize),
}

/// What a project tab shows.
#[derive(Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum View {
    #[default]
    Code,
    Git,
    /// Old Settings tab, now the Claude settings modal. Kept so saved state
    /// still loads; switched back to Code on sight.
    Settings,
}

#[derive(Serialize, Deserialize)]
pub struct ProjectState {
    pub path: PathBuf,
    #[serde(default)]
    pub view: View,
    /// Claude Code session to resume when the project is reopened.
    #[serde(default)]
    pub session_id: Option<String>,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
pub struct WindowState {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub maximized: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
pub struct Saved {
    pub projects: Vec<ProjectState>,
    pub active: Active,
    pub model: String,
    pub chat_session_id: Option<String>,
    pub theme: Theme,
    /// Interface and code font families; empty means the built-in ones.
    pub ui_font: String,
    pub code_font: String,
    /// Whether Kynex animates, or follows the OS's "reduce motion".
    pub motion: Motion,
    pub sessions_collapsed: bool,
    pub sessions_width: f32,
    pub window: Option<WindowState>,
    /// The `claude` to run instead of the one found on PATH (App settings).
    pub cli_path: Option<PathBuf>,
    /// The shell new terminals open; `None` is the OS's.
    pub shell: Option<ShellKind>,
    /// A Claude Code release the user chose to skip; newer ones are offered again.
    pub skipped_cli_version: Option<String>,
    /// The terminal panel under the conversation is showing.
    pub terminal_open: bool,
    pub terminal_height: f32,
    /// The session timeline is showing beside the transcript.
    pub timeline_open: bool,
    /// Unsent messages, by Claude Code session id.
    pub drafts: HashMap<String, String>,
}

impl Default for Saved {
    fn default() -> Self {
        Self {
            projects: Vec::new(),
            active: Active::Chat,
            model: String::new(),
            chat_session_id: None,
            theme: Theme::default(),
            ui_font: String::new(),
            code_font: String::new(),
            motion: Motion::System,
            sessions_collapsed: false,
            sessions_width: 260.,
            window: None,
            cli_path: None,
            shell: None,
            skipped_cli_version: None,
            terminal_open: false,
            terminal_height: 280.,
            timeline_open: false,
            drafts: HashMap::new(),
        }
    }
}

impl Saved {
    pub fn load() -> Self {
        let mut saved: Saved = std::fs::read_to_string(data_dir().join("state.json"))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        if matches!(saved.active, Active::Project(i) if i >= saved.projects.len()) {
            saved.active = Active::Chat;
        }
        saved.sessions_width = saved.sessions_width.clamp(180., 480.);
        saved.terminal_height = saved.terminal_height.max(120.);
        saved
    }

    pub fn write(&self) {
        if let Ok(s) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(data_dir().join("state.json"), s);
        }
    }
}
