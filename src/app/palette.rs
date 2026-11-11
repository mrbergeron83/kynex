//! The command palette (Ctrl+K): one search over every open project's
//! sessions, the project tabs and the workspace's actions; Enter does the
//! picked one. `kynex_core::palette` ranks them; this lists them and acts.
//!
//! Each project's past sessions are read in the background as it opens; the
//! open ones' live state shows beside them.

use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use crate::ui::px;
use gpui::{
    Action, AnyElement, Context, Entity, Focusable, FontWeight, HighlightStyle, MouseButton, SharedString, StyledText,
    Window, div, prelude::*,
};
use kynex_core::layout::Direction;
use kynex_core::live::Key;
use kynex_core::palette::{self, Entry, Filter, Kind, Match, Palette, Section};
use kynex_core::paths::project_name;
use kynex_core::session::{self, SessionInfo};

use super::KynexApp;
use super::app_settings::{APPEARANCE_SECTION, KEYBOARD_SECTION, STORAGE_SECTION};
use super::goto::Which;
use super::keys;
use super::palette_filters::filter_chips;
use super::state::Active;
use crate::theme::{THEMES, Theme, mono, theme};
use crate::ui::input::{self, TextInput};
use crate::ui::motion::{self, fade, pop};
use crate::ui::wash::wash;
use crate::ui::{Icon, icon, section_label, spinner};

/// Results listed at most; the list scrolls through them.
const LISTED: usize = 60;
/// Rows shown at once.
const ROWS: usize = 10;
/// Actions remembered as recently used, for an empty query.
const RECENT: usize = 8;

#[derive(Clone)]
pub(super) struct PaletteView {
    input: Entity<TextInput>,
    rows: Rc<Vec<Row>>,
    palette: Rc<Palette>,
    /// The folders whose sessions are listed: each project's workspaces.
    folders: Rc<Vec<Folder>>,
    /// Each folder's past sessions, as they come in.
    sessions: Rc<HashMap<PathBuf, Vec<SessionInfo>>>,
    /// Which sessions are listed, by state.
    filter: Filter,
    /// What `found` is for, and the selection in it.
    query: String,
    found: Rc<Vec<Match>>,
    selected: usize,
    /// The entries changed (sessions came in): match again, keeping the selection.
    stale: bool,
}

/// A workspace's folder, and where its sessions go in the list.
pub(super) struct Folder {
    pub(super) project: PathBuf,
    pub(super) path: PathBuf,
    pub(super) project_name: String,
    pub(super) section: Section,
}

/// What a result is besides its title: where it goes and what's shown with it.
pub(super) struct Row {
    pub(super) target: Target,
    pub(super) detail: SharedString,
    /// A shortcut, or a session's state ("working", "needs you").
    pub(super) hint: SharedString,
}

#[derive(Clone)]
pub(super) enum Target {
    Session {
        project: PathBuf,
        workspace: PathBuf,
        id: String,
    },
    /// An open session with no transcript listed yet.
    Open {
        project: PathBuf,
        workspace: PathBuf,
        key: Key,
    },
    Tab(Active),
    /// One of a project's workspaces (checkouts).
    Workspace {
        project: PathBuf,
        path: PathBuf,
    },
    Command(Cmd),
    /// A custom command, by name.
    UserCommand(String),
    /// A new agent pane running the agent with this rule id.
    Agent(String),
    /// An agent pane that's open.
    AgentPane {
        project: PathBuf,
        workspace: PathBuf,
        pane: kynex_core::layout::PaneId,
    },
}

/// What an action in the palette does.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Cmd {
    NewSession,
    OpenProject,
    CloseTab,
    NextTab,
    PrevTab,
    NextWorkspace,
    PrevWorkspace,
    SplitRight,
    SplitDown,
    ClosePane,
    NewPaneTab,
    ClosePaneTab,
    NextPaneTab,
    PrevPaneTab,
    NextPane,
    PrevPane,
    Focus(Direction),
    Swap(Direction),
    ZoomPane,
    /// Move the focused pane to tab `n` of its workspace.
    MovePaneToTab(usize),
    MovePaneToNewTab,
    ResizePanes,
    RenamePaneTab,
    PickWorkspace,
    NewWorkspace,
    CloseWorkspace,
    PickProject,
    UndoLayout,
    RedoLayout,
    ReopenPane,
    LayoutSnapshots,
    NextNeedsYou,
    ToggleSessions,
    ToggleTerminal,
    ToggleTimeline,
    ExpandTools,
    Rewind,
    Compact,
    PickModel,
    CycleMode,
    SearchHistory,
    StashDraft,
    EditInEditor,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    Settings(usize),
    ProjectSettings,
    Extensions,
    SessionScratch,
    ProjectScratch,
    About,
    CheckForUpdates,
    /// Archive the project's sessions not used in this many days.
    ArchiveOlder(u32),
    Theme(Theme),
    Quit,
}

impl Cmd {
    /// Its workspace action, for the shortcut shown and to run it the way the key does.
    fn action(self) -> Option<Box<dyn Action>> {
        Some(match self {
            Cmd::NewSession => Box::new(keys::NewSession),
            Cmd::OpenProject => Box::new(keys::OpenProject),
            Cmd::CloseTab => Box::new(keys::CloseTab),
            Cmd::NextTab => Box::new(keys::NextTab),
            Cmd::PrevTab => Box::new(keys::PrevTab),
            Cmd::NextWorkspace => Box::new(keys::NextWorkspace),
            Cmd::PrevWorkspace => Box::new(keys::PrevWorkspace),
            Cmd::SplitRight => Box::new(keys::SplitRight),
            Cmd::SplitDown => Box::new(keys::SplitDown),
            Cmd::ClosePane => Box::new(keys::ClosePane),
            Cmd::NewPaneTab => Box::new(keys::NewPaneTab),
            Cmd::ClosePaneTab => Box::new(keys::ClosePaneTab),
            Cmd::NextPaneTab => Box::new(keys::NextPaneTab),
            Cmd::PrevPaneTab => Box::new(keys::PrevPaneTab),
            Cmd::NextPane => Box::new(keys::NextPane),
            Cmd::Focus(d) => Box::new(keys::FocusPane(d)),
            Cmd::Swap(d) => Box::new(keys::SwapPane(d)),
            Cmd::ZoomPane => Box::new(keys::ZoomPane),
            Cmd::ResizePanes => Box::new(keys::ResizeMode),
            Cmd::RenamePaneTab => Box::new(keys::RenamePaneTab),
            Cmd::PickWorkspace => Box::new(keys::PickWorkspace),
            Cmd::NewWorkspace => Box::new(keys::NewWorkspace),
            Cmd::CloseWorkspace => Box::new(keys::CloseWorkspace),
            Cmd::PickProject => Box::new(keys::PickProject),
            Cmd::UndoLayout => Box::new(keys::UndoLayout),
            Cmd::RedoLayout => Box::new(keys::RedoLayout),
            Cmd::ReopenPane => Box::new(keys::ReopenPane),
            Cmd::LayoutSnapshots => Box::new(keys::LayoutSnapshots),
            Cmd::NextNeedsYou => Box::new(keys::NextNeedsYou),
            Cmd::Settings(KEYBOARD_SECTION) => Box::new(keys::KeyboardSettings),
            Cmd::ToggleSessions => Box::new(keys::ToggleSessions),
            Cmd::ToggleTerminal => Box::new(keys::ToggleTerminal),
            Cmd::ToggleTimeline => Box::new(keys::ToggleTimeline),
            Cmd::ExpandTools => Box::new(keys::ExpandTools),
            Cmd::Rewind => Box::new(keys::Rewind),
            Cmd::PickModel => Box::new(keys::PickModel),
            Cmd::CycleMode => Box::new(keys::CycleMode),
            Cmd::SearchHistory => Box::new(keys::SearchHistory),
            Cmd::StashDraft => Box::new(keys::StashDraft),
            Cmd::EditInEditor => Box::new(keys::EditInEditor),
            Cmd::ZoomIn => Box::new(keys::ZoomIn),
            Cmd::ZoomOut => Box::new(keys::ZoomOut),
            Cmd::ZoomReset => Box::new(keys::ZoomReset),
            Cmd::Settings(0) => Box::new(keys::OpenSettings),
            Cmd::Quit => Box::new(keys::Quit),
            _ => return None,
        })
    }
}

/// Every action: what it's called, and other words it's found by.
fn commands() -> Vec<(Cmd, String, &'static str)> {
    let mut list: Vec<(Cmd, String, &'static str)> = [
        (Cmd::NewSession, "New session", "clear start conversation"),
        (Cmd::OpenProject, "Open project…", "folder add tab"),
        (Cmd::CloseTab, "Close tab", "project"),
        (Cmd::NextTab, "Next tab", "project"),
        (Cmd::PrevTab, "Previous tab", "project"),
        (Cmd::NextWorkspace, "Next workspace", "worktree checkout branch"),
        (Cmd::PrevWorkspace, "Previous workspace", "worktree checkout branch"),
        (Cmd::SplitRight, "Split pane right", "tile side by side new session"),
        (Cmd::SplitDown, "Split pane down", "tile below new session"),
        (Cmd::ClosePane, "Close pane", "tile split"),
        (Cmd::NewPaneTab, "New pane tab", "layout tiles workspace"),
        (Cmd::ClosePaneTab, "Close pane tab", "layout tiles workspace"),
        (Cmd::NextPaneTab, "Next pane tab", "layout tiles workspace"),
        (Cmd::PrevPaneTab, "Previous pane tab", "layout tiles workspace"),
        (Cmd::NextPane, "Next pane", "tile focus split"),
        (Cmd::PrevPane, "Previous pane", "tile focus split"),
        (Cmd::Focus(Direction::Left), "Focus pane to the left", "tile move split"),
        (Cmd::Focus(Direction::Down), "Focus pane below", "tile move split"),
        (Cmd::Focus(Direction::Up), "Focus pane above", "tile move split"),
        (Cmd::Focus(Direction::Right), "Focus pane to the right", "tile move split"),
        (Cmd::Swap(Direction::Left), "Swap pane with the one to the left", "tile move split"),
        (Cmd::Swap(Direction::Down), "Swap pane with the one below", "tile move split"),
        (Cmd::Swap(Direction::Up), "Swap pane with the one above", "tile move split"),
        (Cmd::Swap(Direction::Right), "Swap pane with the one to the right", "tile move split"),
        (Cmd::ZoomPane, "Zoom pane", "maximize fullscreen tile toggle"),
        (Cmd::ResizePanes, "Resize panes", "tile grow shrink split size drag"),
        (Cmd::RenamePaneTab, "Rename pane tab", "layout tiles workspace name"),
        (Cmd::PickWorkspace, "Go to workspace…", "worktree checkout branch switch"),
        (Cmd::NewWorkspace, "New workspace", "worktree checkout branch add"),
        (Cmd::CloseWorkspace, "Close workspace", "remove worktree checkout delete"),
        (Cmd::PickProject, "Go to project…", "tab switch open folder"),
        (Cmd::UndoLayout, "Undo layout change", "panes split close arrangement back"),
        (Cmd::RedoLayout, "Redo layout change", "panes split close arrangement forward"),
        (Cmd::ReopenPane, "Reopen closed pane", "undo close restore"),
        (Cmd::LayoutSnapshots, "Restore an older layout…", "snapshots history panes yesterday"),
        (Cmd::NextNeedsYou, "Next session that needs you", "attention waiting blocked permission question"),
        (Cmd::ToggleSessions, "Toggle sessions panel", "sidebar list"),
        (Cmd::ToggleTerminal, "Toggle terminal", "shell panel console"),
        (Cmd::ToggleTimeline, "Toggle timeline", "turns steps panel"),
        (Cmd::ExpandTools, "Expand / collapse tool calls", "transcript open"),
        (Cmd::Rewind, "Rewind to an earlier message", "undo checkpoint back"),
        (Cmd::Compact, "Compact the conversation", "context summarize free"),
        (Cmd::PickModel, "Change model…", "opus sonnet haiku fable"),
        (Cmd::CycleMode, "Next permission mode", "plan auto accept edits"),
        (Cmd::SearchHistory, "Search sent messages", "history"),
        (Cmd::StashDraft, "Stash the draft", "message put aside"),
        (Cmd::EditInEditor, "Write the message in your editor", "external"),
        (Cmd::ZoomIn, "Zoom in", "bigger larger scale"),
        (Cmd::ZoomOut, "Zoom out", "smaller scale"),
        (Cmd::ZoomReset, "Reset zoom", "100% scale"),
        (Cmd::Settings(0), "Settings", "preferences general cli shell"),
        (Cmd::Settings(APPEARANCE_SECTION), "Settings: Appearance", "theme font motion toasts footer"),
        (Cmd::Settings(KEYBOARD_SECTION), "Settings: Keyboard", "shortcuts keymap bindings"),
        (Cmd::Settings(STORAGE_SECTION), "Settings: Storage", "disk space size clean delete old sessions"),
        (Cmd::ProjectSettings, "Project settings", "rules permissions effort model directories"),
        (Cmd::Extensions, "Extensions", "mcp servers plugins"),
        (Cmd::SessionScratch, "Files of this session", "scratch scratchpad edited images screenshots output explorer"),
        (
            Cmd::ProjectScratch,
            "Scratch files of this project",
            "temporary files scratchpad explorer browse every session",
        ),
        (Cmd::About, "About Kynex", "version"),
        (Cmd::CheckForUpdates, "Check for updates", "kynex claude cli version"),
    ]
    .into_iter()
    .map(|(c, label, words)| (c, label.to_string(), words))
    .collect();
    list.extend(super::session_tidy::TIDY_AGES.iter().map(|(days, label)| {
        (Cmd::ArchiveOlder(*days), format!("Archive sessions older than {label}"), "tidy old clean up hide")
    }));
    list.extend(THEMES.iter().map(|&(t, name)| (Cmd::Theme(t), format!("Theme: {name}"), "appearance colours")));
    list.push((Cmd::Quit, "Quit Kynex".into(), "exit close"));
    list
}

impl KynexApp {
    /// Ctrl+K: opens the palette, or closes it.
    pub(super) fn toggle_palette(&mut self, _: &keys::CommandPalette, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.take().is_some() {
            self.focus_input(window, cx);
            return cx.notify();
        }
        self.menu = None;
        self.refresh_user_commands(cx);
        self.refresh_agent_rules(cx);
        let input = cx.new(|cx| TextInput::new(cx).placeholder("Go to a session, project or action…"));
        input.read(cx).focus_handle(cx).focus(window, cx);
        let view = PaletteView {
            input,
            rows: Rc::default(),
            palette: Rc::default(),
            folders: Rc::new(self.palette_folders(cx)),
            sessions: Rc::default(),
            filter: Filter::All,
            query: String::new(),
            found: Rc::default(),
            selected: 0,
            stale: true,
        };
        self.palette = Some(view);
        self.rebuild_palette(cx);
        let folders: Vec<PathBuf> =
            self.palette.iter().flat_map(|v| v.folders.iter().map(|f| f.path.clone())).collect();
        for path in folders {
            cx.spawn(async move |this, cx| {
                let dir = path.clone();
                let mut items = cx.background_spawn(async move { session::list_sessions(&dir) }).await;
                this.update(cx, |this, cx| {
                    for i in &mut items {
                        if let Some(name) = this.given_name(&path, &i.id) {
                            i.title = name;
                        }
                    }
                    let Some(view) = &mut this.palette else { return };
                    Rc::make_mut(&mut view.sessions).insert(path, items);
                    this.rebuild_palette(cx);
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
        cx.notify();
    }

    /// Every project's workspaces' folders, in the order their sessions are listed.
    fn palette_folders(&mut self, cx: &mut Context<Self>) -> Vec<Folder> {
        let projects: Vec<PathBuf> = self.saved.projects.iter().map(|p| p.path.clone()).collect();
        let mut folders = Vec::new();
        for (at, project) in projects.into_iter().enumerate() {
            let list = self.workspaces_of(&project, cx);
            let places =
                palette::workspace_places(&list.iter().map(|(c, _)| (c.main, c.label.as_str())).collect::<Vec<_>>());
            let name = project_name(&project);
            for ((c, _), place) in list.iter().zip(places) {
                let label = palette::section_label(&name, &c.label, list.len());
                folders.push(Folder {
                    project: project.clone(),
                    path: c.path.clone(),
                    project_name: format!("{name} {}", c.label),
                    section: Section { project: at, workspace: place, label },
                });
            }
        }
        folders
    }

    /// Lists every entry again: after sessions come in.
    fn rebuild_palette(&mut self, cx: &Context<Self>) {
        let Some(view) = &self.palette else { return };
        let (mut rows, mut entries) = self.palette_session_rows(&view.folders, &view.sessions);
        // A project's other checkouts, once it has any.
        for p in &self.saved.projects {
            let Some(w) = self.worktrees.get(&p.path) else { continue };
            let workspaces = kynex_core::workspace::from_worktrees(&p.path, &w.read(cx).list);
            if workspaces.len() < 2 {
                continue;
            }
            let name = project_name(&p.path);
            for c in workspaces {
                rows.push(Row {
                    target: Target::Workspace { project: p.path.clone(), path: c.path.clone() },
                    detail: format!("{name} · workspace").into(),
                    hint: "".into(),
                });
                entries.push(Entry {
                    kind: Kind::Project,
                    title: format!("{name}: {}", c.label),
                    keywords: format!("workspace worktree checkout {}", c.branch.unwrap_or_default()),
                    when: None,
                    section: None,
                    state: None,
                });
            }
        }
        let tabs =
            [(Target::Tab(Active::Chat), "Chat".to_string(), String::new())].into_iter().chain(
                self.saved.projects.iter().enumerate().map(|(i, p)| {
                    (Target::Tab(Active::Project(i)), project_name(&p.path), p.path.display().to_string())
                }),
            );
        for (target, name, path) in tabs {
            rows.push(Row { target, detail: path.clone().into(), hint: "".into() });
            entries.push(Entry {
                kind: Kind::Project,
                title: name,
                keywords: path,
                when: None,
                section: None,
                state: None,
            });
        }
        for (cmd, label, words) in commands() {
            let hint = cmd.action().and_then(|a| crate::ui::shortcut(a.as_ref(), cx)).unwrap_or_default();
            rows.push(Row { target: Target::Command(cmd), detail: "".into(), hint: hint.into() });
            entries.push(Entry {
                kind: Kind::Action,
                title: label,
                keywords: words.into(),
                when: None,
                section: None,
                state: None,
            });
        }
        self.palette_more_rows(&mut rows, &mut entries);
        let view = self.palette.as_mut().unwrap();
        view.rows = Rc::new(rows);
        view.palette = Rc::new(Palette::new(entries));
        view.stale = true;
    }

    /// Lists every entry again if the palette is open: the custom commands came in.
    pub(super) fn rebuild_palette_if_open(&mut self, cx: &Context<Self>) {
        if self.palette.is_some() {
            self.rebuild_palette(cx);
        }
    }

    /// The results for what's typed, matched once per change.
    fn palette_found(&mut self, cx: &Context<Self>) -> Option<Rc<Vec<Match>>> {
        let view = self.palette.as_mut()?;
        let query = view.input.read(cx).text().to_string();
        if view.stale || view.query != query {
            let recent: Vec<&str> = self.palette_recent.iter().map(String::as_str).collect();
            let found = view.palette.matching_in(&query, &recent, LISTED, view.filter);
            // Grouped under their labels: sessions by project and workspace.
            view.found = Rc::new(view.palette.grouped(found));
            if view.query != query {
                view.selected = 0;
            }
            view.query = query;
            view.stale = false;
            view.selected = view.selected.min(view.found.len().saturating_sub(1));
        }
        Some(view.found.clone())
    }

    pub(super) fn palette_set_filter(&mut self, filter: Filter, cx: &mut Context<Self>) {
        let Some(view) = &mut self.palette else { return };
        if view.filter != filter {
            view.filter = filter;
            view.selected = 0;
            view.stale = true;
            cx.notify();
        }
    }

    /// Tab / Shift+Tab: the next or previous filter.
    pub(super) fn palette_step_filter(&mut self, by: isize, cx: &mut Context<Self>) {
        if let Some(view) = &self.palette {
            self.palette_set_filter(view.filter.step(by), cx);
        }
    }

    fn palette_move(&mut self, by: isize, cx: &mut Context<Self>) {
        let Some(n) = self.palette_found(cx).map(|f| f.len()).filter(|&n| n > 0) else { return };
        let view = self.palette.as_mut().unwrap();
        view.selected = (view.selected as isize + by).rem_euclid(n as isize) as usize;
        cx.notify();
    }

    /// Closes the palette and does result `i`.
    fn take_palette(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(found) = self.palette_found(cx) else { return };
        let Some(view) = self.palette.take() else { return };
        let Some(m) = found.get(i) else { return };
        let target = view.rows[m.index].target.clone();
        match target {
            Target::Session { project, workspace, id } => {
                self.edit_groups(&workspace, |g| {
                    g.unarchive(&id);
                });
                self.reveal_session(&project, &workspace, Which::Id(&id), window, cx);
            }
            Target::Open { project, workspace, key } => {
                self.reveal_session(&project, &workspace, Which::Open(key), window, cx);
            }
            Target::Workspace { project, path } => {
                if let Some(i) = self.saved.projects.iter().position(|p| p.path == project) {
                    self.select(Active::Project(i), window, cx);
                    self.select_workspace(&path, cx);
                }
                self.focus_input(window, cx);
            }
            Target::Tab(Active::Chat) => self.jump(0, window, cx),
            Target::Tab(tab) => {
                self.select(tab, window, cx);
                self.focus_input(window, cx);
            }
            Target::UserCommand(name) => self.run_user_command(name, window, cx),
            Target::Agent(id) => self.open_agent_pane(id, None, window, cx),
            Target::AgentPane { project, workspace, pane } => {
                if let Some(i) = self.saved.projects.iter().position(|p| p.path == project) {
                    self.select(Active::Project(i), window, cx);
                    self.select_workspace(&workspace, cx);
                    self.sync_tiles(window, cx);
                    self.focus_pane(pane, window, cx);
                }
                self.focus_input(window, cx);
            }
            Target::Command(cmd) => {
                let title = view.palette.entries[m.index].title.clone();
                self.palette_recent.retain(|r| *r != title);
                self.palette_recent.insert(0, title);
                self.palette_recent.truncate(RECENT);
                self.run_palette_command(cmd, window, cx);
            }
        }
        cx.notify();
    }

    fn run_palette_command(&mut self, cmd: Cmd, window: &mut Window, cx: &mut Context<Self>) {
        // The pane to move is the one focused now, before the message box takes the keys.
        if let Cmd::MovePaneToTab(_) | Cmd::MovePaneToNewTab = cmd {
            let focus = self.tiling_scope().and_then(|(_, ws, _)| self.tiles.get(&ws)?.focus());
            if let Some(pane) = focus {
                let tab = if let Cmd::MovePaneToTab(i) = cmd { Some(i) } else { None };
                self.move_pane(pane, tab, window, cx);
            }
            return self.focus_input(window, cx);
        }
        // Keys that only work in the message box (Ctrl+R, Ctrl+S…) find it focused.
        self.focus_input(window, cx);
        if let Some(action) = cmd.action() {
            return window.dispatch_action(action, cx);
        }
        match cmd {
            Cmd::PrevPane => self.step_pane(-1, window, cx),
            Cmd::Compact => self.compact(self.focused_key(), cx),
            Cmd::Settings(section) => {
                self.app_settings = None;
                self.open_app_settings(cx);
                if let Some(s) = &mut self.app_settings {
                    s.section = section;
                }
            }
            Cmd::ProjectSettings => {
                if let Some(p) = self.active_project() {
                    let path = p.path.clone();
                    self.open_settings(path, cx);
                }
            }
            Cmd::Extensions => self.open_extensions(0, cx),
            Cmd::SessionScratch => {
                if let Some(key) = self.focused_key() {
                    self.open_session_scratch(key, window, cx);
                }
            }
            Cmd::ProjectScratch => self.open_project_scratch(window, cx),
            Cmd::About => self.open_about(cx),
            Cmd::CheckForUpdates => {
                self.check_for_update(cx);
                self.check_cli_now(cx);
            }
            Cmd::ArchiveOlder(days) => self.archive_older_than(days, cx),
            Cmd::Theme(t) => self.set_theme(t, cx),
            _ => {}
        }
    }

    pub(super) fn render_palette(
        &mut self,
        leaving: Option<f32>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let found = self.palette_found(cx)?;
        let view = self.palette.clone()?;
        let t = theme(cx).clone();
        let start = (view.selected + 1).saturating_sub(ROWS);
        let mut last = None;
        let mut rows: Vec<AnyElement> = Vec::new();
        for (i, m) in found.iter().enumerate().skip(start).take(ROWS) {
            let entry = &view.palette.entries[m.index];
            let kind = entry.kind;
            // A header per kind; sessions get one per project and workspace.
            let place = (kind, entry.section.as_ref().map(|s| (s.project, s.workspace)));
            if last != Some(place) {
                let label = match (kind, &entry.section) {
                    (_, Some(s)) => s.label.as_str(),
                    (Kind::Session, None) => "Sessions",
                    (Kind::Project, _) => "Projects",
                    (Kind::Action, _) => "Actions",
                };
                let header = section_label(label, cx).px_2().pb_1();
                rows.push(if last.is_none() { header.pt_1p5() } else { header.pt_3() }.into_any_element());
                last = Some(place);
            }
            rows.push(palette_row(&view, i, m, &t, cx));
        }
        let more = found.len().saturating_sub(start + ROWS);
        let empty = found.is_empty();
        // Projects whose sessions are still being listed.
        let listing = view.sessions.len() < view.folders.len();
        let chips = filter_chips(view.filter, view.palette.counts(), &t, cx);

        let card = div()
            .id("palette")
            .occlude()
            .flex()
            .flex_col()
            .w_full()
            .max_w(px(560.))
            .rounded_lg()
            .overflow_hidden()
            .border_1()
            .border_color(t.border)
            .bg(t.elevated)
            .shadow_lg()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            // The field keeps the focus; the list takes ↑ / ↓, Enter and Esc from it.
            .capture_action(cx.listener(|this, _: &input::Up, _, cx| {
                this.palette_move(-1, cx);
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(|this, _: &input::Down, _, cx| {
                this.palette_move(1, cx);
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(|this, _: &input::Enter, window, cx| {
                let selected = this.palette.as_ref().map_or(0, |v| v.selected);
                this.take_palette(selected, window, cx);
                cx.stop_propagation();
            }))
            .capture_key_down(cx.listener(|this, ev: &gpui::KeyDownEvent, _, cx| {
                let k = &ev.keystroke;
                if k.key == "tab" && !k.modifiers.modified() {
                    this.palette_step_filter(1, cx);
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, _: &keys::Interrupt, window, cx| {
                this.palette = None;
                this.focus_input(window, cx);
                cx.notify();
                cx.stop_propagation();
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .h(px(34.))
                    .px_3()
                    .border_b_1()
                    .border_color(t.border_variant)
                    .child(icon(Icon::Search, t.text_muted).flex_none())
                    .child(div().flex_1().min_w_0().child(view.input.clone())),
            )
            .child(chips)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .p_1()
                    .children(rows)
                    .when(empty, |d| {
                        d.child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .px_2()
                                .py_2()
                                .text_sm()
                                .text_color(t.text_muted)
                                .when(listing, |d| d.child(spinner("palette-listing", t.text_muted, px(12.))))
                                .child(if listing { "Looking through sessions…" } else { "Nothing matches" }),
                        )
                    })
                    .when(more > 0, |d| {
                        d.child(
                            div().px_2().pt_1().text_xs().text_color(t.text_placeholder).child(format!("{more} more")),
                        )
                    }),
            );

        Some(fade(
            "palette-fade",
            div()
                .id("palette-backdrop")
                .absolute()
                .inset_0()
                .flex()
                .flex_col()
                .items_center()
                .px_10()
                .pt(px(120.))
                .bg(crate::theme::theme(cx).scrim())
                .occlude()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| {
                        this.palette = None;
                        this.focus_input(window, cx);
                        cx.notify();
                    }),
                )
                .child(pop("palette-in", card, gpui::px(6.), leaving)),
            motion::QUICK,
            leaving,
        ))
    }
}

/// Result `i` of the palette, `m`: its icon, title with the matched letters
/// bold, detail and hint.
fn palette_row(
    view: &PaletteView,
    i: usize,
    m: &Match,
    t: &crate::theme::Palette,
    cx: &mut Context<KynexApp>,
) -> AnyElement {
    let row = &view.rows[m.index];
    let entry = &view.palette.entries[m.index];
    let bold = HighlightStyle { font_weight: Some(FontWeight::BOLD), color: Some(t.text), ..Default::default() };
    let title = &entry.title;
    let highlights: Vec<_> = m
        .positions
        .iter()
        .filter(|&&p| title.is_char_boundary(p) && p < title.len())
        .map(|&p| (p..p + title[p..].chars().next().map_or(1, char::len_utf8), bold))
        .collect();
    let glyph = match entry.kind {
        Kind::Session => Icon::Message,
        Kind::Project => Icon::Folder,
        Kind::Action => Icon::ChevronRight,
    };
    let hint_colour = match row.hint.as_ref() {
        "needs you" => t.warning,
        "working" => t.accent,
        "done" => t.success,
        _ => t.text_placeholder,
    };
    let mono_hint = matches!(row.target, Target::Command(_));
    div()
        .id(("palette-row", i))
        .relative()
        .flex()
        .items_center()
        .gap_2()
        .h(px(28.))
        .px_2()
        .rounded_md()
        .cursor_pointer()
        .child(wash(t.hover, (i == view.selected).then_some(t.active)))
        .child(icon(glyph, t.text_muted).size_3p5().flex_none())
        .child(
            div()
                .flex_none()
                .max_w(px(360.))
                .truncate()
                .text_sm()
                .text_color(t.text)
                .child(StyledText::new(title.clone()).with_highlights(highlights)),
        )
        .child(div().flex_1().min_w_0().truncate().text_xs().text_color(t.text_muted).child(row.detail.clone()))
        .when(!row.hint.is_empty(), |d| {
            d.child(
                div()
                    .flex_none()
                    .text_size(px(11.))
                    .text_color(hint_colour)
                    .when(mono_hint, |d| d.font_family(mono()))
                    .child(row.hint.clone()),
            )
        })
        .on_click(cx.listener(move |this, _, window, cx| this.take_palette(i, window, cx)))
        .into_any_element()
}
