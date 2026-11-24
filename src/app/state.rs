//! What survives a restart: open projects, the active tab, theme, window
//! placement, OpenRouter settings, and simple background notifications.
//! Saved to `<data dir>/state.json`.

use std::path::PathBuf;

use kynex_core::awake::KeepAwake;
use kynex_core::motion::Motion;
use kynex_core::paths::data_dir;
use kynex_core::terminal::ShellKind;
use kynex_core::toasts::Corner;
use serde::{Deserialize, Serialize};

use crate::theme::Theme;

#[derive(Serialize, Deserialize)]
pub struct ProjectState {
    pub path: PathBuf,
    /// Legacy field from before Git became a pane: opens the Git pane once, then drops.
    #[serde(default, rename = "view", skip_serializing_if = "Option::is_none")]
    old_view: Option<String>,
    /// Opened on purpose as a tab of its own (a workspace, from its menu): it doesn't
    /// fold into its project at startup.
    #[serde(default)]
    pub own_tab: bool,
}

impl ProjectState {
    pub fn new(path: PathBuf) -> Self {
        Self { path, old_view: None, own_tab: false }
    }

    pub fn was_on_git(&self) -> bool {
        self.old_view.as_deref() == Some("Git")
    }

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
    /// Which project tab is active, or `None` when no projects are open.
    pub active_project: Option<usize>,
    pub theme: Theme,
    /// The theme file (its name without `.json`, in the data dir's `themes/`) in use instead of `theme`.
    pub custom_theme: Option<String>,
    /// Interface and code font families; empty means the built-in ones.
    pub ui_font: String,
    pub code_font: String,
    /// Whether Kynex animates, or follows the OS's "reduce motion".
    pub motion: Motion,
    pub window: Option<WindowState>,
    /// The `claude` binary to run instead of the one found on PATH (Settings → General).
    pub cli_path: Option<PathBuf>,
    /// Where new worktrees go unless a project says otherwise; `None` is under the data dir.
    pub worktree_root: Option<PathBuf>,
    /// The shell new terminals open; `None` is the OS's default.
    pub shell: Option<ShellKind>,
    /// A Claude Code release the user chose to skip; newer ones are offered again.
    pub skipped_cli_version: Option<String>,
    /// Legacy dock flag: migrates into a terminal pane once.
    pub terminal_open: bool,
    /// Height used when toggling a bottom terminal pane with Ctrl+`.
    pub terminal_height: f32,
    /// Plain terminal panes show their scrollback from last time when they reopen.
    pub terminal_history: bool,
    /// Show a simple OS notification when an agent finishes or needs attention while Kynex is in the background.
    pub background_notifications: bool,
    /// Kynex's shortcuts from an older `install.ps1` got its app ID (Windows; once).
    pub shortcuts_stamped: bool,
    /// The version whose icon the shell was last told to show (Windows; once per version).
    pub icons_shown_for: Option<String>,
    /// What Appearance changes of the pane tiles' look; the theme gives the rest.
    pub tiles: kynex_core::tile_style::TileTweaks,
    /// The window corner toasts gather in.
    pub toast_corner: Corner,
    /// How wide the Claude pane's transcript column is.
    pub column: kynex_core::claude::column::ColumnWidth,
    /// The UI zoom, 1.0 for normal size (`kynex_core::scale`).
    pub zoom: f32,
    /// The status footer along the bottom of the window is showing.
    pub footer: bool,
    /// When to keep the computer from sleeping.
    pub keep_awake: KeepAwake,
    /// How long an idle Claude pane keeps its `claude` process.
    pub sleep_after: kynex_core::claude::sleep::SleepAfter,
    /// OpenRouter: My models, default model, output cap, last key check (key is in OS keychain).
    pub openrouter: kynex_core::openrouter::OpenRouterConfig,
    /// The sessions panel splits ungrouped sessions by provider, under a header each.
    pub sessions_by_provider: bool,
    /// The sessions tree shows archived sessions (dimmed, last under their workspace).
    pub sessions_show_archived: bool,
    /// Bypass permissions joins the Claude panes' mode cycle (`--allow-dangerously-skip-permissions`).
    pub allow_bypass: bool,
    /// The file's format version (`kynex_core::migrate`).
    pub version: u32,
    /// `state.json` was written by a newer Kynex; this session leaves the file alone.
    #[serde(skip)]
    pub newer_than_us: Option<u32>,
}

impl Default for Saved {
    fn default() -> Self {
        Self {
            version: kynex_core::migrate::CURRENT,
            newer_than_us: None,
            projects: Vec::new(),
            active_project: None,
            theme: Theme::default(),
            custom_theme: None,
            ui_font: String::new(),
            code_font: String::new(),
            motion: Motion::System,
            window: None,
            cli_path: None,
            worktree_root: None,
            shell: None,
            skipped_cli_version: None,
            terminal_open: false,
            terminal_height: 280.,
            terminal_history: true,
            background_notifications: true,
            shortcuts_stamped: false,
            icons_shown_for: None,
            tiles: Default::default(),
            toast_corner: Corner::default(),
            column: Default::default(),
            zoom: kynex_core::scale::NORMAL,
            footer: true,
            keep_awake: KeepAwake::default(),
            sleep_after: Default::default(),
            openrouter: Default::default(),
            sessions_by_provider: false,
            sessions_show_archived: false,
            allow_bypass: false,
        }
    }
}

impl Saved {
    pub fn load() -> Self {
        let (mut saved, newer): (Saved, _) = kynex_core::migrate::read(&data_dir().join("state.json"));
        saved.newer_than_us = newer;
        saved.settle()
    }

    fn settle(mut self) -> Self {
        if self.projects.is_empty() {
            self.active_project = None;
        } else if self.active_project.is_none_or(|i| i >= self.projects.len()) {
            self.active_project = Some(0);
        }
        self.terminal_height = self.terminal_height.max(120.);
        self.zoom = kynex_core::scale::clamp(self.zoom);
        for p in &mut self.projects {
            if !p.was_on_git() {
                p.left_git();
            }
        }
        self
    }

    pub fn write(&self) {
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
                {"path":"c","view":"Settings"},{"path":"d"}],"active_project":0}"#,
        );
        let git: Vec<bool> = saved.projects.iter().map(ProjectState::was_on_git).collect();
        assert_eq!(git, [true, false, false, false]);
        assert_eq!(saved.active_project, Some(0));
        let mut p = serde_json::from_str::<ProjectState>(r#"{"path":"a","view":"Git"}"#).unwrap();
        assert!(serde_json::to_string(&p).unwrap().contains(r#""view":"Git""#));
        p.left_git();
        assert!(!serde_json::to_string(&p).unwrap().contains("view"));
    }

    #[test]
    fn settle_clamps_active_project() {
        assert_eq!(read(r#"{"projects":[],"active_project":0}"#).active_project, None);
        assert_eq!(read(r#"{"projects":[{"path":"a"}],"active_project":5}"#).active_project, Some(0));
        assert_eq!(read(r#"{"projects":[{"path":"a"}]}"#).active_project, Some(0));
    }
}
