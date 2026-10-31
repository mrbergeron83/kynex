//! What survives a restart: open projects, the active tab, the model, theme,
//! window placement. Saved to `<data dir>/state.json`.

use std::collections::HashMap;
use std::path::PathBuf;

use kynex_core::awake::KeepAwake;
use kynex_core::column::ColumnWidth;
use kynex_core::live::SleepAfter;
use kynex_core::motion::Motion;
use kynex_core::paths::data_dir;
use kynex_core::terminal::ShellKind;
use kynex_core::toasts::Corner;
use kynex_core::tray::OnClose;
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
    /// Claude names the most recent unnamed sessions too, not only the one
    /// shown (a small Haiku call each, on the user's quota).
    pub name_past_sessions: bool,
    /// An OS notification when a session finishes or needs you while Kynex
    /// is in the background. Off unless turned on.
    pub notify: bool,
    /// Kynex's shortcuts from an older `install.ps1` got its app ID (Windows; once).
    pub shortcuts_stamped: bool,
    /// How wide the transcript and composer are.
    pub column: ColumnWidth,
    /// The window corner toasts gather in.
    pub toast_corner: Corner,
    /// The UI zoom, 1.0 for normal size (`kynex_core::scale`).
    pub zoom: f32,
    /// The status footer along the bottom of the window is showing.
    pub footer: bool,
    /// When to keep the computer from sleeping.
    pub keep_awake: KeepAwake,
    /// What closing the window does: hide Kynex (tray / Dock) or quit.
    pub on_close: OnClose,
    /// How long an idle session keeps its `claude` process.
    pub sleep_after: SleepAfter,
    /// The chief of staff (`kynex_core::chief`): the Chat tab shows it instead
    /// of chat, it was turned on past its warning, its session, and each
    /// project's chief worker session.
    pub chief_view: bool,
    pub chief_consented: bool,
    pub chief_session_id: Option<String>,
    pub chief_workers: HashMap<PathBuf, String>,
    /// What each chief worker has used, across runs; and the ones working
    /// when last saved, which a restart cut short.
    pub chief_spent: HashMap<PathBuf, kynex_core::spent::Spent>,
    pub chief_working: Vec<PathBuf>,
    /// OpenRouter: My models, the default, the output cap, the last key check
    /// (Settings → Providers). The key itself is in the OS keychain.
    pub openrouter: kynex_core::openrouter::OpenRouterConfig,
    /// The sessions panel splits ungrouped sessions by provider, under a header each.
    pub sessions_by_provider: bool,
}

impl Default for Saved {
    fn default() -> Self {
        Self {
            projects: Vec::new(),
            active: Active::Chat,
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
            name_past_sessions: false,
            notify: false,
            shortcuts_stamped: false,
            column: ColumnWidth::default(),
            toast_corner: Corner::default(),
            zoom: kynex_core::scale::NORMAL,
            footer: true,
            keep_awake: KeepAwake::default(),
            on_close: OnClose::default(),
            sleep_after: SleepAfter::default(),
            chief_view: false,
            chief_consented: false,
            chief_session_id: None,
            chief_workers: HashMap::new(),
            chief_spent: HashMap::new(),
            chief_working: Vec::new(),
            openrouter: Default::default(),
            sessions_by_provider: false,
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
        saved.zoom = kynex_core::scale::clamp(saved.zoom);
        saved
    }

    pub fn write(&self) {
        if let Ok(s) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(data_dir().join("state.json"), s);
        }
    }
}
