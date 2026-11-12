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

#[derive(Serialize, Deserialize)]
pub struct ProjectState {
    pub path: PathBuf,
    /// What the tab showed when a project had views: `"Git"` was the full-window Git view,
    /// which is the Git pane now. Read from an older `state.json` and kept (written back as it
    /// was, for an older Kynex) until the pane has opened in its place; anything else is dropped.
    #[serde(default, rename = "view", skip_serializing_if = "Option::is_none")]
    old_view: Option<String>,
    /// Claude Code session to resume when the project is reopened.
    #[serde(default)]
    pub session_id: Option<String>,
    /// Opened on purpose as a tab of its own (a workspace, from its menu): it doesn't
    /// fold into its project at startup.
    #[serde(default)]
    pub own_tab: bool,
}

impl ProjectState {
    pub fn new(path: PathBuf) -> Self {
        Self { path, old_view: None, session_id: None, own_tab: false }
    }

    /// It was left on the old full-window Git view: the Git pane opens in its place.
    pub fn was_on_git(&self) -> bool {
        self.old_view.as_deref() == Some("Git")
    }

    /// The Git pane has taken the old Git view's place.
    pub fn left_git(&mut self) {
        self.old_view = None;
    }
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
    /// The theme file (its name without `.json`, in the data dir's `themes/`) in use instead of `theme`.
    pub custom_theme: Option<String>,
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
    /// The browser Claude sessions get (`kynex_core::browser`), or none.
    pub browser: kynex_core::browser::Pick,
    /// A Claude Code release the user chose to skip; newer ones are offered again.
    pub skipped_cli_version: Option<String>,
    /// The old terminal dock was showing: the first workspace shown gets a terminal pane.
    pub terminal_open: bool,
    /// The old dock's height, which a terminal pane shown with Ctrl+` takes.
    pub terminal_height: f32,
    /// The session timeline is showing beside the transcript in chat (a project's is a pane).
    pub timeline_open: bool,
    /// Whether `timeline_open` from before the timeline was a pane has been turned into one.
    pub timeline_migrated: bool,
    /// Terminal panes show their scrollback from last time when they reopen.
    pub terminal_history: bool,
    /// Unsent messages, by Claude Code session id.
    pub drafts: HashMap<String, String>,
    /// Claude names the most recent unnamed sessions too, not only the one
    /// shown (a small Haiku call each, on the user's quota).
    pub name_past_sessions: bool,
    /// Project sessions hand routine work to a Haiku subagent unless their
    /// project says otherwise (`kynex_core::delegate`). On unless turned off.
    pub delegate: bool,
    /// An OS notification when a session finishes or needs you while Kynex
    /// is in the background. Off unless turned on.
    pub notify: bool,
    /// A sound with those notes and toasts. Off unless turned on, and only with `notify`.
    pub sounds: bool,
    /// Kynex's shortcuts from an older `install.ps1` got its app ID (Windows; once).
    pub shortcuts_stamped: bool,
    /// How wide the transcript and composer are.
    pub column: ColumnWidth,
    /// What Appearance changes of the pane tiles' look; the theme gives the rest.
    pub tiles: kynex_core::tile_style::TileTweaks,
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
    /// OpenRouter: My models, the default, the output cap, the last key check
    /// (Settings → Providers). The key itself is in the OS keychain.
    pub openrouter: kynex_core::openrouter::OpenRouterConfig,
    /// The sessions panel splits ungrouped sessions by provider, under a header each.
    pub sessions_by_provider: bool,
    /// Grok is offered as a provider (Settings → Providers); off until opted in.
    pub grok: bool,
    /// A setup check has passed here before, so the next launch shows the
    /// workspace while it checks again, rather than "Checking Claude Code…".
    pub setup_checked: bool,
    /// The file's format version (`kynex_core::migrate`).
    pub version: u32,
    /// `state.json` was written by a newer Kynex, which version it was; this
    /// session then leaves the file alone.
    #[serde(skip)]
    pub newer_than_us: Option<u32>,
}

impl Default for Saved {
    fn default() -> Self {
        Self {
            version: kynex_core::migrate::CURRENT,
            newer_than_us: None,
            projects: Vec::new(),
            active: Active::Chat,
            chat_session_id: None,
            theme: Theme::default(),
            custom_theme: None,
            ui_font: String::new(),
            code_font: String::new(),
            motion: Motion::System,
            sessions_collapsed: false,
            sessions_width: 260.,
            window: None,
            cli_path: None,
            shell: None,
            browser: Default::default(),
            skipped_cli_version: None,
            terminal_open: false,
            terminal_height: 280.,
            timeline_open: false,
            timeline_migrated: false,
            terminal_history: true,
            drafts: HashMap::new(),
            name_past_sessions: false,
            delegate: true,
            notify: false,
            sounds: false,
            shortcuts_stamped: false,
            column: ColumnWidth::default(),
            tiles: Default::default(),
            toast_corner: Corner::default(),
            zoom: kynex_core::scale::NORMAL,
            footer: true,
            keep_awake: KeepAwake::default(),
            on_close: OnClose::default(),
            sleep_after: SleepAfter::default(),
            openrouter: Default::default(),
            sessions_by_provider: false,
            grok: false,
            setup_checked: false,
        }
    }
}

impl Saved {
    pub fn load() -> Self {
        let (mut saved, newer): (Saved, _) = kynex_core::migrate::read(&data_dir().join("state.json"));
        saved.newer_than_us = newer;
        saved.settle()
    }

    /// What was read, made sound: an active tab that exists, sizes in range, old views dropped.
    fn settle(mut self) -> Self {
        let saved = &mut self;
        if matches!(saved.active, Active::Project(i) if i >= saved.projects.len()) {
            saved.active = Active::Chat;
        }
        saved.sessions_width = saved.sessions_width.clamp(180., 480.);
        saved.terminal_height = saved.terminal_height.max(120.);
        saved.zoom = kynex_core::scale::clamp(saved.zoom);
        // Only the Git view is worth keeping of the old views (Code was the default).
        for p in &mut saved.projects {
            if !p.was_on_git() {
                p.left_git();
            }
        }
        self
    }

    pub fn write(&self) {
        // A file from a newer Kynex stays as it is; this session doesn't know its format.
        if self.newer_than_us.is_some() {
            return;
        }
        if let Ok(s) = kynex_core::migrate::stamp(self) {
            let _ = std::fs::write(data_dir().join("state.json"), s);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(json: &str) -> Saved {
        serde_json::from_str::<Saved>(json).unwrap().settle()
    }

    #[test]
    fn a_project_left_on_its_git_view_still_loads_and_asks_for_the_git_pane() {
        let saved = read(
            r#"{"projects":[{"path":"a","view":"Git"},{"path":"b","view":"Code"},
                {"path":"c","view":"Settings"},{"path":"d"}],"active":{"Project":0}}"#,
        );
        let git: Vec<bool> = saved.projects.iter().map(ProjectState::was_on_git).collect();
        assert_eq!(git, [true, false, false, false]);
        assert!(matches!(saved.active, Active::Project(0)));
        // Until the pane has opened, an older Kynex reads it as it wrote it; after, not at all.
        let mut p = serde_json::from_str::<ProjectState>(r#"{"path":"a","view":"Git"}"#).unwrap();
        assert!(serde_json::to_string(&p).unwrap().contains(r#""view":"Git""#));
        p.left_git();
        assert!(!serde_json::to_string(&p).unwrap().contains("view"));
        let code = &saved.projects[1];
        assert!(!serde_json::to_string(code).unwrap().contains("view"));
    }
}
