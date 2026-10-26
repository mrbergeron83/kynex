use std::collections::{HashMap, HashSet};
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use gpui::{
    Anchor, Animation, AnimationExt, AnyElement, App, ClickEvent, Context, Entity, ExternalPaths, FocusHandle,
    Focusable, FollowMode, FontWeight, Image, ImageFormat, KeyBinding, KeyDownEvent, ListAlignment, ListState,
    MouseButton, MouseMoveEvent, PathPromptOptions, Pixels, Point, SharedString, Subscription, Transformation, Window,
    WindowBounds, actions, anchored, deferred, div, img, list, percentage, prelude::*, px,
};
use serde::{Deserialize, Serialize};

use crate::chrome::{self, TITLE_BAR_HEIGHT};
use crate::cli::{self, UpdateState};
use crate::git::GitView;
use crate::markdown;
use crate::project::{self, ClaudeSettings, Mode, Permissions};
use crate::prompt::{self, PromptAnswer, PromptView, Request};
use crate::session::{self, Attachment, Item, Kind, PermState, Session, SessionInfo};
use crate::theme::{self, THEMES, Theme, mono, theme};
use crate::ui::input::{InputEvent, TextInput};
use crate::ui::{
    Icon, Tooltip, divider, ghost_button, icon, icon_button, menu_item, outline_button, popover,
};
use crate::wsl;

/// Until the CLI's own list arrives (or if it can't be had).
const MODELS: [(&str, &str); 5] =
    [("", "Default"), ("opus", "Opus"), ("fable", "Fable"), ("sonnet", "Sonnet"), ("haiku", "Haiku")];
const MAX_IMAGE_EDGE: u32 = 1568;
const THREAD_WIDTH: Pixels = px(780.);
const CLI_CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);

actions!(kynex, [OpenProject, NextTab, PrevTab, ToggleSessions, CycleMode, Interrupt]);

/// Workspace shortcuts and their defaults. `<data dir>/keymap.json` overrides
/// them by name, e.g. `{ "interrupt": "ctrl-c" }`; an empty string unbinds one.
const SHORTCUTS: [(&str, &str); 6] = [
    ("open_project", "secondary-o"),
    ("next_tab", "ctrl-tab"),
    ("prev_tab", "ctrl-shift-tab"),
    ("toggle_sessions", "secondary-b"),
    ("cycle_mode", "shift-tab"),
    ("interrupt", "escape"),
];

pub fn bind_keys(cx: &mut App) {
    let path = data_dir().join("keymap.json");
    let user: HashMap<String, String> = match std::fs::read_to_string(&path) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_else(|e| {
            eprintln!("kynex: ignoring {}: {e}", path.display());
            HashMap::new()
        }),
        Err(_) => {
            // Write the defaults out so there's something to edit.
            let defaults: std::collections::BTreeMap<_, _> = SHORTCUTS.iter().copied().collect();
            if let Ok(s) = serde_json::to_string_pretty(&defaults) {
                let _ = std::fs::create_dir_all(data_dir());
                let _ = std::fs::write(&path, s);
            }
            HashMap::new()
        }
    };
    let c = Some("Workspace");
    let mut bindings = Vec::new();
    for (name, default) in SHORTCUTS {
        let keys = crate::ui::keys(user.get(name).map_or(default, String::as_str));
        if keys.trim().is_empty() {
            continue;
        }
        if let Some(bad) = keys.split_whitespace().find(|k| gpui::Keystroke::parse(k).is_err()) {
            eprintln!("kynex: keymap.json: `{bad}` isn't a key for {name}; using {default}");
        }
        let keys = if keys.split_whitespace().all(|k| gpui::Keystroke::parse(k).is_ok()) {
            keys
        } else {
            crate::ui::keys(default)
        };
        bindings.push(match name {
            "open_project" => KeyBinding::new(keys, OpenProject, c),
            "next_tab" => KeyBinding::new(keys, NextTab, c),
            "prev_tab" => KeyBinding::new(keys, PrevTab, c),
            "toggle_sessions" => KeyBinding::new(keys, ToggleSessions, c),
            "cycle_mode" => KeyBinding::new(keys, CycleMode, c),
            _ => KeyBinding::new(keys, Interrupt, c),
        });
    }
    cx.bind_keys(bindings);
}

/// Which top-level tab is showing. Projects sit at the top of the hierarchy,
/// with Chat pinned as the first tab.
#[derive(Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
enum Active {
    #[default]
    Chat,
    Project(usize),
}

/// What a project tab shows.
#[derive(Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
enum View {
    #[default]
    Code,
    Git,
    /// Old Settings tab, now the Claude settings modal. Kept so saved state
    /// still loads; switched back to Code on sight.
    Settings,
}

#[derive(Serialize, Deserialize)]
struct ProjectState {
    path: PathBuf,
    #[serde(default)]
    view: View,
    /// Claude Code session to resume when the project is reopened.
    #[serde(default)]
    session_id: Option<String>,
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
    projects: Vec<ProjectState>,
    active: Active,
    model: String,
    chat_session_id: Option<String>,
    theme: Theme,
    sessions_collapsed: bool,
    sessions_width: f32,
    pub window: Option<WindowState>,
    /// A Claude Code release the user chose to skip; newer ones are offered again.
    skipped_cli_version: Option<String>,
}

impl Default for Saved {
    fn default() -> Self {
        Self {
            projects: Vec::new(),
            active: Active::Chat,
            model: String::new(),
            chat_session_id: None,
            theme: Theme::default(),
            sessions_collapsed: false,
            sessions_width: 260.,
            window: None,
            skipped_cli_version: None,
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
        saved
    }

    pub fn theme(&self) -> Theme {
        self.theme
    }
}

/// Which conversation: Chat, or a project's by its folder.
type Key = Option<PathBuf>;

#[derive(Clone, PartialEq)]
enum Menu {
    Model,
    Theme,
    Mode,
    Effort,
    Tab(usize),
}

struct OpenMenu {
    which: Menu,
    at: Point<Pixels>,
    anchor: Anchor,
}

/// The transcript list for one conversation, and how much of it it has seen.
struct Thread {
    list: ListState,
    count: usize,
}

struct SettingsView {
    path: PathBuf,
    perms: Permissions,
    claude: ClaudeSettings,
    section: usize,
}

/// Past sessions for the active conversation's folder, re-read when the folder
/// or the current session changes, or a reply starts or finishes.
struct SessionList {
    cwd: PathBuf,
    current: Option<String>,
    busy: bool,
    items: Vec<SessionInfo>,
}

struct ActivePrompt {
    key: Key,
    id: String,
    view: Entity<PromptView>,
    _sub: Subscription,
}

pub struct KynexApp {
    saved: Saved,
    code: HashMap<PathBuf, Session>,
    chat: Session,
    git: HashMap<PathBuf, Entity<GitView>>,
    threads: HashMap<Key, Thread>,
    /// Tool calls and results opened up in the transcript.
    expanded: HashSet<(Key, usize)>,
    composer: Entity<TextInput>,
    new_rule: Entity<TextInput>,
    attachments: Vec<Attachment>,
    notice: Option<String>,
    sessions: Option<SessionList>,
    /// Session id whose delete button was clicked once and is awaiting confirmation.
    confirm_delete: Option<String>,
    /// The project settings being edited; the modal is open while this is set.
    settings: Option<SettingsView>,
    /// Projects whose rules changed mid-reply; their process restarts once it's idle.
    restart_when_idle: HashSet<PathBuf>,
    prompt: Option<ActivePrompt>,
    menu: Option<OpenMenu>,
    resizing_sessions: bool,
    /// Whether a newer Claude Code is out, or being installed.
    cli_update: UpdateState,
    focus: FocusHandle,
    wake: session::Wake,
}

/// The OS's per-user data dir plus `kynex`: `%APPDATA%\kynex` on Windows,
/// `~/Library/Application Support/kynex` on macOS, `$XDG_DATA_HOME/kynex` (or
/// `~/.local/share/kynex`) on Linux. `KYNEX_DATA_DIR` overrides it, to run a test
/// instance without touching your real state.
pub fn data_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("KYNEX_DATA_DIR") {
        return PathBuf::from(dir);
    }
    dirs::data_dir().unwrap_or_else(std::env::temp_dir).join("kynex")
}

fn project_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string())
}

impl KynexApp {
    pub fn new(saved: Saved, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let dir = data_dir();
        let _ = std::fs::create_dir_all(dir.join("chat"));

        // Session reader threads ring this; one task drains every session and repaints.
        let (tx, rx) = async_channel::bounded::<()>(1);
        let wake: session::Wake = Arc::new(move || {
            let _ = tx.try_send(());
        });
        cx.spawn(async move |this, cx| {
            while rx.recv().await.is_ok() {
                if this.update(cx, |this, cx| this.poll(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();

        // Look for a newer Claude Code now and every few hours after.
        cx.spawn(async move |this, cx| {
            loop {
                let found = cx.background_spawn(async { cli::check() }).await;
                if this.update(cx, |this, cx| this.cli_checked(found, cx)).is_err() {
                    break;
                }
                cx.background_executor().timer(CLI_CHECK_EVERY).await;
            }
        })
        .detach();

        let mut chat = match saved.chat_session_id.clone() {
            Some(id) => Session::resume(Kind::Chat, dir.join("chat"), id),
            None => Session::new(Kind::Chat, dir.join("chat")),
        };
        chat.model = saved.model.clone();
        chat.wake = wake.clone();

        let composer = cx.new(|cx| TextInput::new(cx).multiline());
        cx.subscribe_in(&composer, window, |this, _, ev, window, cx| match ev {
            InputEvent::Submit => this.send(window, cx),
            InputEvent::PasteImages(images) => {
                for i in images {
                    this.attach_bytes(&i.bytes, cx);
                }
            }
            InputEvent::Changed => {}
        })
        .detach();
        let new_rule = cx.new(|cx| TextInput::new(cx).mono().placeholder("Bash(npm test:*)"));
        cx.subscribe(&new_rule, |this, _, ev, cx| {
            if let InputEvent::Submit = ev {
                this.add_rule(false, cx);
            }
        })
        .detach();

        cx.observe_window_bounds(window, |this, window, _| {
            let bounds = window.window_bounds();
            let b = bounds.get_bounds();
            this.saved.window = Some(WindowState {
                x: b.origin.x.into(),
                y: b.origin.y.into(),
                width: b.size.width.into(),
                height: b.size.height.into(),
                maximized: matches!(bounds, WindowBounds::Maximized(_)),
            });
        })
        .detach();
        cx.on_app_quit(|this, _| {
            this.save();
            async {}
        })
        .detach();

        composer.read(cx).focus_handle(cx).focus(window, cx);
        Self {
            saved,
            code: HashMap::new(),
            chat,
            git: HashMap::new(),
            threads: HashMap::new(),
            expanded: HashSet::new(),
            composer,
            new_rule,
            attachments: Vec::new(),
            notice: None,
            sessions: None,
            confirm_delete: None,
            settings: None,
            restart_when_idle: HashSet::new(),
            prompt: None,
            menu: None,
            resizing_sessions: false,
            cli_update: UpdateState::Idle,
            focus: cx.focus_handle(),
            wake,
        }
    }

    fn save(&self) {
        if let Ok(s) = serde_json::to_string_pretty(&self.saved) {
            let _ = std::fs::write(data_dir().join("state.json"), s);
        }
    }

    fn poll(&mut self, cx: &mut Context<Self>) {
        self.chat.poll();
        for s in self.code.values_mut() {
            s.poll();
        }
        self.restart_idle();
        self.sync_session_ids();
        cx.notify();
    }

    /// Restart the projects whose settings changed, once they're idle.
    fn restart_idle(&mut self) {
        self.restart_when_idle.retain(|path| match self.code.get_mut(path) {
            Some(s) if s.busy => true,
            Some(s) => {
                s.kill();
                false
            }
            None => false,
        });
    }

    /// Takes in a background version check. Never interrupts an update in progress,
    /// and stays quiet about a release the user skipped.
    fn cli_checked(&mut self, found: Result<Option<(String, String)>, String>, cx: &mut Context<Self>) {
        if matches!(self.cli_update, UpdateState::Updating { .. }) {
            return;
        }
        match found {
            Ok(Some((installed, latest))) => {
                if self.saved.skipped_cli_version.as_deref() != Some(latest.as_str()) {
                    self.cli_update = UpdateState::Available { installed, latest };
                }
            }
            // Updated elsewhere (or by the CLI itself) since the last check.
            Ok(None) if matches!(self.cli_update, UpdateState::Available { .. }) => self.cli_update = UpdateState::Idle,
            Ok(None) => {}
            Err(e) => eprintln!("kynex: couldn't check for Claude Code updates: {e}"),
        }
        cx.notify();
    }

    /// Installs the newest Claude Code with `claude update`. Idle sessions stop
    /// first so their binary isn't in use, and resume on the new one with the next
    /// message; busy ones restart once they finish.
    fn update_cli(&mut self, cx: &mut Context<Self>) {
        let UpdateState::Available { latest, .. } = &self.cli_update else { return };
        self.cli_update = UpdateState::Updating { latest: latest.clone() };
        if !self.chat.busy {
            self.chat.kill();
        }
        for s in self.code.values_mut().filter(|s| !s.busy) {
            s.kill();
        }
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async { cli::update() }).await;
            this.update(cx, |this, cx| {
                this.cli_update = match result {
                    Ok(version) => {
                        let busy = this.code.iter().filter(|(_, s)| s.busy).map(|(p, _)| p.clone());
                        this.restart_when_idle.extend(busy.collect::<Vec<_>>());
                        UpdateState::Updated { version }
                    }
                    Err(e) => UpdateState::Failed(e),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn skip_cli_version(&mut self, cx: &mut Context<Self>) {
        if let UpdateState::Available { latest, .. } = &self.cli_update {
            self.saved.skipped_cli_version = Some(latest.clone());
            self.save();
        }
        self.cli_update = UpdateState::Idle;
        cx.notify();
    }

    /// Copy live session ids into the saved state so they survive a restart.
    fn sync_session_ids(&mut self) {
        let mut changed = false;
        if self.saved.chat_session_id != self.chat.session_id {
            self.saved.chat_session_id = self.chat.session_id.clone();
            changed = true;
        }
        for p in &mut self.saved.projects {
            if let Some(s) = self.code.get(&p.path) {
                if p.session_id != s.session_id {
                    p.session_id = s.session_id.clone();
                    changed = true;
                }
            }
        }
        if changed {
            self.save();
        }
    }

    fn active_project(&self) -> Option<&ProjectState> {
        match self.saved.active {
            Active::Project(i) => self.saved.projects.get(i),
            Active::Chat => None,
        }
    }

    fn active_key(&self) -> Key {
        self.active_project().map(|p| p.path.clone())
    }

    fn session_for(&mut self, key: &Key) -> Option<&mut Session> {
        match key {
            None => Some(&mut self.chat),
            Some(p) => self.code.get_mut(p),
        }
    }

    fn active_session(&self) -> Option<&Session> {
        match self.saved.active {
            Active::Chat => Some(&self.chat),
            Active::Project(i) => self.saved.projects.get(i).and_then(|p| self.code.get(&p.path)),
        }
    }

    /// The session the composer talks to, created (or resumed) on first use.
    fn session_mut(&mut self) -> Option<&mut Session> {
        let Active::Project(i) = self.saved.active else {
            return Some(&mut self.chat);
        };
        let p = self.saved.projects.get(i)?;
        if p.view != View::Code {
            return None;
        }
        let (path, resume) = (p.path.clone(), p.session_id.clone());
        let (model, wake) = (self.saved.model.clone(), self.wake.clone());
        Some(self.code.entry(path.clone()).or_insert_with(|| {
            let mut s = match resume {
                Some(id) => Session::resume(Kind::Code, path, id),
                None => Session::new(Kind::Code, path),
            };
            s.model = model;
            s.wake = wake;
            s
        }))
    }

    /// Replaces the active conversation (the old process, if any, is stopped).
    fn set_session(&mut self, mut s: Session) {
        s.model = self.saved.model.clone();
        s.wake = self.wake.clone();
        let key = self.active_key();
        self.threads.remove(&key);
        self.expanded.retain(|(k, _)| *k != key);
        match self.saved.active {
            Active::Chat => self.chat = s,
            Active::Project(i) => {
                if let Some(p) = self.saved.projects.get(i) {
                    self.code.insert(p.path.clone(), s);
                }
            }
        }
    }

    fn select(&mut self, a: Active, window: &mut Window, cx: &mut Context<Self>) {
        if self.saved.active != a {
            self.saved.active = a;
            self.menu = None;
            self.save();
            self.composer.read(cx).focus_handle(cx).focus(window, cx);
        }
        cx.notify();
    }

    fn add_project(&mut self, path: PathBuf) {
        let idx = match self.saved.projects.iter().position(|p| p.path == path) {
            Some(i) => i,
            None => {
                self.saved.projects.push(ProjectState { path, view: View::Code, session_id: None });
                self.saved.projects.len() - 1
            }
        };
        self.saved.active = Active::Project(idx);
        self.save();
    }

    fn close_project(&mut self, i: usize) {
        let p = self.saved.projects.remove(i);
        self.code.remove(&p.path);
        self.git.remove(&p.path);
        self.threads.remove(&Some(p.path.clone()));
        self.saved.active = match self.saved.active {
            Active::Project(a) if a > i => Active::Project(a - 1),
            Active::Project(a) if a == i && i > 0 => Active::Project(i - 1),
            Active::Project(a) if a == i => {
                if self.saved.projects.is_empty() { Active::Chat } else { Active::Project(0) }
            }
            other => other,
        };
        self.save();
    }

    fn pick_project(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open project".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = paths.await {
                if let Some(dir) = paths.into_iter().next() {
                    this.update(cx, |this, cx| {
                        this.add_project(dir);
                        cx.notify();
                    })
                    .ok();
                }
            }
        })
        .detach();
    }

    fn open_project(&mut self, _: &OpenProject, _: &mut Window, cx: &mut Context<Self>) {
        self.pick_project(cx);
    }

    fn tab_count(&self) -> usize {
        self.saved.projects.len() + 1
    }

    fn tab_pos(&self) -> usize {
        match self.saved.active {
            Active::Chat => 0,
            Active::Project(i) => i + 1,
        }
    }

    fn jump(&mut self, t: usize, window: &mut Window, cx: &mut Context<Self>) {
        if t < self.tab_count() {
            self.select(if t == 0 { Active::Chat } else { Active::Project(t - 1) }, window, cx);
        }
    }

    fn next_tab(&mut self, _: &NextTab, window: &mut Window, cx: &mut Context<Self>) {
        let t = (self.tab_pos() + 1) % self.tab_count();
        self.jump(t, window, cx);
    }

    fn prev_tab(&mut self, _: &PrevTab, window: &mut Window, cx: &mut Context<Self>) {
        let n = self.tab_count();
        let t = (self.tab_pos() + n - 1) % n;
        self.jump(t, window, cx);
    }

    fn toggle_sessions(&mut self, _: &ToggleSessions, _: &mut Window, cx: &mut Context<Self>) {
        self.saved.sessions_collapsed = !self.saved.sessions_collapsed;
        self.save();
        cx.notify();
    }

    /// Like Shift+Tab in the CLI: the next permission mode, live.
    /// Esc by default: closes an open menu or the settings modal first, then
    /// stops a running reply. Otherwise the key goes on to whoever else wants it.
    fn interrupt_action(&mut self, _: &Interrupt, _: &mut Window, cx: &mut Context<Self>) {
        if self.menu.take().is_some() || self.settings.take().is_some() {
            return cx.notify();
        }
        if self.active_session().is_some_and(|s| s.busy) {
            self.interrupt(cx);
        } else {
            cx.propagate();
        }
    }

    fn cycle_mode(&mut self, _: &CycleMode, _: &mut Window, cx: &mut Context<Self>) {
        let Some(s) = self.session_mut().filter(|s| s.kind == Kind::Code) else { return };
        let next = Mode::ALL.iter().position(|m| *m == s.mode).map_or(0, |i| (i + 1) % Mode::ALL.len());
        s.set_mode(Mode::ALL[next]);
        cx.notify();
    }

    fn key_down(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &ev.keystroke;
        if k.key == "escape" && (self.menu.is_some() || self.settings.is_some()) {
            if self.menu.take().is_none() {
                self.settings = None;
            }
            cx.notify();
            cx.stop_propagation();
            return;
        }
        // Ctrl+1 is Chat, Ctrl+2 the first project, and so on.
        if k.modifiers.secondary(cx) && !k.modifiers.shift && !k.modifiers.alt {
            if let Some(n) = k.key.parse::<usize>().ok().filter(|n| (1..=9).contains(n)) {
                self.jump(n - 1, window, cx);
                cx.stop_propagation();
            }
        }
    }

    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let busy = self.session_mut().is_some_and(|s| s.busy);
        let text = self.composer.read(cx).text().trim().to_string();
        if busy || (text.is_empty() && self.attachments.is_empty()) {
            return;
        }
        let atts = std::mem::take(&mut self.attachments);
        let model = self.saved.model.clone();
        if let Some(s) = self.session_mut() {
            if !s.is_running() {
                s.model = model;
            }
            s.send(text, atts);
        }
        self.composer.update(cx, |c, cx| c.set_text("", cx));
        if let Some(t) = self.threads.get(&self.active_key()) {
            t.list.scroll_to_end();
            t.list.set_follow_mode(FollowMode::Tail);
        }
        let _ = window;
        cx.notify();
    }

    fn interrupt(&mut self, cx: &mut Context<Self>) {
        if let Some(s) = self.session_mut() {
            s.interrupt();
        }
        cx.notify();
    }

    fn attach_files(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Attach".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = paths.await {
                this.update(cx, |this, cx| {
                    for p in paths {
                        this.drop_path(p, cx);
                    }
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    /// A dropped (or picked) path: folders become projects, images attach,
    /// anything else goes into the message as its path.
    fn drop_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if path.is_dir() {
            self.add_project(path);
        } else if is_image(&path) {
            match std::fs::read(&path) {
                Ok(bytes) => self.attach_bytes(&bytes, cx),
                Err(e) => self.notice = Some(format!("Couldn't read image {}: {e}", path.display())),
            }
        } else {
            self.composer.update(cx, |c, cx| {
                let mut text = c.text().to_string();
                if !text.is_empty() && !text.ends_with(' ') {
                    text.push(' ');
                }
                text.push_str(&path.display().to_string());
                c.set_text(text, cx);
            });
        }
    }

    fn on_drop(&mut self, paths: &ExternalPaths, _: &mut Window, cx: &mut Context<Self>) {
        for p in paths.paths() {
            self.drop_path(p.clone(), cx);
        }
        cx.notify();
    }

    fn attach_bytes(&mut self, bytes: &[u8], cx: &mut Context<Self>) {
        let in_code = self.active_project().is_none_or(|p| p.view == View::Code);
        if !in_code {
            return;
        }
        let img = match image::load_from_memory(bytes) {
            Ok(i) => i,
            Err(e) => {
                self.notice = Some(format!("Couldn't read image: {e}"));
                return cx.notify();
            }
        };
        let img = if img.width().max(img.height()) > MAX_IMAGE_EDGE {
            img.resize(MAX_IMAGE_EDGE, MAX_IMAGE_EDGE, image::imageops::FilterType::Triangle)
        } else {
            img
        };
        let mut png = Vec::new();
        if let Err(e) = img.to_rgba8().write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png) {
            self.notice = Some(format!("Couldn't encode image: {e}"));
            return cx.notify();
        }
        self.attachments.push(Attachment {
            media_type: "image/png".into(),
            base64: base64::engine::general_purpose::STANDARD.encode(&png),
            image: Arc::new(Image::from_bytes(ImageFormat::Png, png)),
        });
        cx.notify();
    }

    /// Adds "always allow in this project" rules to the project's Kynex settings.
    fn save_rules(&mut self, cwd: &Path, rules: Vec<String>) {
        if rules.is_empty() {
            return;
        }
        let mut perms = Permissions::load(cwd);
        if perms.allow(rules) {
            if let Err(e) = perms.save(cwd) {
                self.notice = Some(e);
            }
            if let Some(s) = &mut self.settings {
                s.perms = perms;
            }
        }
    }

    fn answer(&mut self, key: Key, request_id: String, answer: session::Answer, window: &mut Window, cx: &mut Context<Self>) {
        let mut rules = Vec::new();
        let mut cwd = None;
        if let Some(s) = self.session_for(&key) {
            let ix = s.items.iter().position(|i| matches!(i, Item::Permission { request_id: r, .. } if *r == request_id));
            if let Some(ix) = ix {
                rules = s.answer_permission(ix, answer);
                cwd = Some(s.cwd.clone());
            }
        }
        if let Some(cwd) = cwd {
            self.save_rules(&cwd, rules);
        }
        self.prompt = None;
        self.composer.read(cx).focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    /// Keeps the prompt view in step with the active session's oldest pending request.
    fn sync_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let key = self.active_key();
        let pending = self.active_session().and_then(|s| {
            let mut it = s.items.iter().filter_map(|i| match i {
                Item::Permission { request_id, tool, input, suggestions, description, state: PermState::Pending } => {
                    Some(Request {
                        id: request_id.clone(),
                        tool: tool.clone(),
                        input: input.clone(),
                        suggestions: suggestions.clone(),
                        description: description.clone(),
                        kind: s.kind,
                    })
                }
                _ => None,
            });
            let first = it.next()?;
            Some((first, it.count()))
        });
        let Some((req, queued)) = pending else {
            self.prompt = None;
            return;
        };
        if self.prompt.as_ref().is_none_or(|p| p.key != key || p.id != req.id) {
            let id = req.id.clone();
            let view = cx.new(|cx| PromptView::new(req, window, cx));
            let sub = cx.subscribe_in(&view, window, {
                let (key, id) = (key.clone(), id.clone());
                move |this, _, PromptAnswer(answer), window, cx| this.answer(key.clone(), id.clone(), answer.clone(), window, cx)
            });
            self.prompt = Some(ActivePrompt { key, id, view, _sub: sub });
        }
        if let Some(p) = &self.prompt {
            p.view.update(cx, |v, _| v.queued = queued);
        }
    }

    fn open_menu(&mut self, which: Menu, at: Point<Pixels>, anchor: Anchor, cx: &mut Context<Self>) {
        self.menu = if self.menu.as_ref().is_some_and(|m| m.which == which) {
            None
        } else {
            Some(OpenMenu { which, at, anchor })
        };
        cx.notify();
    }

    fn open_settings(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.settings = Some(SettingsView {
            perms: Permissions::load(&path),
            claude: ClaudeSettings::load(&path),
            path,
            section: 0,
        });
        cx.notify();
    }

    /// Applies an edit in the settings modal. Edits save immediately: the mode
    /// applies to the live session, everything else from the next process start.
    fn edit_settings(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut SettingsView)) {
        let Some(view) = &mut self.settings else { return };
        let (perms_before, claude_before) = (view.perms.clone(), view.claude.clone());
        f(view);
        let path = view.path.clone();
        let mut errors = Vec::new();
        if view.perms != perms_before {
            if let Err(e) = view.perms.save(&path) {
                errors.push(e);
            }
        }
        if view.claude != claude_before {
            if let Err(e) = view.claude.save(&path) {
                errors.push(e);
            }
            if view.claude.mode != claude_before.mode {
                if let Some(s) = self.code.get_mut(&path) {
                    s.set_mode(view.claude.mode);
                }
            }
        }
        let restart = view.perms != perms_before
            || view.claude.effort != claude_before.effort
            || view.claude.add_dirs != claude_before.add_dirs;
        if restart {
            // The CLI reads these at startup: restart the project's process so it picks them up.
            // `--resume` keeps the conversation going; rules allowed "this session" are dropped.
            self.restart_when_idle.insert(path);
            self.restart_idle();
        }
        if let Some(e) = errors.pop() {
            self.notice = Some(e);
        }
        cx.notify();
    }

    fn add_rule(&mut self, deny: bool, cx: &mut Context<Self>) {
        let rule = self.new_rule.read(cx).text().trim().to_string();
        if rule.is_empty() {
            return;
        }
        self.edit_settings(cx, |v| {
            let list = if deny { &mut v.perms.deny } else { &mut v.perms.allow };
            if !list.contains(&rule) {
                list.push(rule);
            }
        });
        self.new_rule.update(cx, |i, cx| i.set_text("", cx));
    }

    fn add_dir(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Add folder".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = paths.await {
                this.update(cx, |this, cx| {
                    for dir in paths {
                        this.edit_settings(cx, |v| {
                            if !v.claude.add_dirs.contains(&dir) {
                                v.claude.add_dirs.push(dir);
                            }
                        });
                    }
                })
                .ok();
            }
        })
        .detach();
    }

    // ---------------------------------------------------------------- rendering

    fn render_title_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = theme(cx).clone();
        let mut tabs = vec![self.render_tab(0, "Chat".into(), Icon::Message, None, self.chat.busy, false, cx)];
        for (i, p) in self.saved.projects.iter().enumerate() {
            let session = self.code.get(&p.path);
            let waiting =
                session.is_some_and(|s| s.items.iter().any(|it| matches!(it, Item::Permission { state: PermState::Pending, .. })));
            let busy = session.is_some_and(|s| s.busy);
            let tooltip = p.path.display().to_string();
            tabs.push(self.render_tab(i + 1, project_name(&p.path).into(), Icon::Folder, Some(tooltip), busy, waiting, cx));
        }

        div()
            .id("title-bar")
            .flex()
            .flex_none()
            .items_center()
            .h(TITLE_BAR_HEIGHT)
            .bg(t.title_bar)
            .border_b_1()
            .border_color(t.border)
            .pl(chrome::leading_inset() + px(10.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .pr_3()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.text_muted)
                    .child("kynex"),
            )
            .child(div().flex().items_center().gap_0p5().children(tabs))
            .child(
                icon_button("add-project", Icon::Plus, cx)
                    .ml_1()
                    .tooltip(Tooltip::with_meta("Open project folder", "Ctrl+O"))
                    .on_click(cx.listener(|this, _, _, cx| this.pick_project(cx))),
            )
            // The empty middle of the bar is the window's title bar.
            .child(chrome::drag_area(div().id("drag").flex_1().h_full()))
            .child(
                ghost_button("theme", cx)
                    .mr_2()
                    .text_color(t.text_muted)
                    .child(THEMES.iter().find(|x| x.0 == self.saved.theme).map_or("", |x| x.1))
                    .child(icon(Icon::ChevronDown, t.text_placeholder).size_3())
                    .on_click(cx.listener(|this, ev: &ClickEvent, _, cx| {
                        this.open_menu(Menu::Theme, ev.position(), Anchor::TopRight, cx)
                    })),
            )
            .child(chrome::window_controls(window, cx))
    }

    #[allow(clippy::too_many_arguments)]
    fn render_tab(
        &self,
        pos: usize,
        label: SharedString,
        which: Icon,
        tooltip: Option<String>,
        busy: bool,
        waiting: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx).clone();
        let active = self.tab_pos() == pos;
        let hover = t.hover;
        let fg = if active { t.text } else { t.text_muted };
        div()
            .id(("tab", pos))
            .flex()
            .items_center()
            .gap_1p5()
            .h(px(26.))
            .px_2p5()
            .rounded_md()
            .text_sm()
            .text_color(fg)
            .cursor_pointer()
            .when(active, |d| d.bg(t.active))
            .when(!active, |d| d.hover(move |s| s.bg(hover)))
            .child(icon(which, fg).size_3p5())
            .child(label)
            .when(waiting, |d| d.child(icon(Icon::Dot, t.warning).size_2()))
            .when(busy && !waiting, |d| d.child(spinner(("tab-spin", pos), t.accent, px(12.))))
            .when_some(tooltip, |d, tip| d.tooltip(Tooltip::with_meta(tip, "Middle-click to close · right-click for more")))
            .on_click(cx.listener(move |this, _, window, cx| this.jump(pos, window, cx)))
            .when(pos > 0, |d| {
                d.on_mouse_down(
                    MouseButton::Middle,
                    cx.listener(move |this, _, _, cx| {
                        this.close_project(pos - 1);
                        cx.notify();
                    }),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, ev: &gpui::MouseDownEvent, _, cx| {
                        this.open_menu(Menu::Tab(pos - 1), ev.position, Anchor::TopLeft, cx)
                    }),
                )
            })
            .into_any_element()
    }

    fn render_sessions_panel(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = theme(cx).clone();
        let Some(s) = self.session_mut() else { return div() };
        let (kind, cwd, current, busy) = (s.kind, s.cwd.clone(), s.session_id.clone(), s.busy);
        // Re-read when the folder or session changes, and when a reply finishes (titles and times move).
        if self.sessions.as_ref().is_none_or(|l| l.cwd != cwd || l.current != current || l.busy != busy) {
            self.sessions = Some(SessionList { items: session::list_sessions(&cwd), cwd: cwd.clone(), current: current.clone(), busy });
        }
        let items = &self.sessions.as_ref().unwrap().items;

        let header = div()
            .flex()
            .flex_none()
            .items_center()
            .h(px(36.))
            .pl_3()
            .pr_1p5()
            .gap_1()
            .child(div().flex_1().text_sm().font_weight(FontWeight::MEDIUM).child("Sessions"))
            .child(
                icon_button("new-session", Icon::Plus, cx)
                    .when(busy || current.is_none(), |b| b.opacity(0.4))
                    .tooltip(Tooltip::text(if busy {
                        "Stop the current reply first"
                    } else if current.is_none() {
                        "Already a new session"
                    } else {
                        "New session"
                    }))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let Some(s) = this.session_mut() else { return };
                        if s.busy || s.session_id.is_none() {
                            return;
                        }
                        let (kind, cwd) = (s.kind, s.cwd.clone());
                        this.set_session(Session::new(kind, cwd));
                        cx.notify();
                    })),
            )
            .child(
                icon_button("hide-sessions", Icon::PanelLeft, cx)
                    .tooltip(Tooltip::with_meta("Hide sessions", "Ctrl+B"))
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_sessions(&ToggleSessions, window, cx))),
            );

        let mut rows: Vec<AnyElement> = Vec::new();
        if current.is_none() {
            rows.push(session_row("new", "New session".into(), None, true, &t).into_any_element());
        }
        if items.is_empty() {
            rows.push(div().px_3().py_2().text_sm().text_color(t.text_placeholder).child("No past sessions yet").into_any_element());
        }
        for (n, info) in items.iter().enumerate() {
            let is_current = current.as_deref() == Some(info.id.as_str());
            let confirming = self.confirm_delete.as_deref() == Some(info.id.as_str());
            let id = info.id.clone();
            let (open_id, del_id) = (id.clone(), id.clone());
            let (kind2, cwd2, cwd3) = (kind, cwd.clone(), cwd.clone());
            let current2 = current.clone();
            let delete = if confirming {
                div()
                    .id(("confirm-delete", n))
                    .px_1p5()
                    .h(px(20.))
                    .flex()
                    .items_center()
                    .rounded_sm()
                    .text_xs()
                    .text_color(t.error)
                    .cursor_pointer()
                    .hover(|s| s.bg(t.hover))
                    .child("Delete?")
            } else {
                icon_button(("delete", n), Icon::Trash, cx).invisible().group_hover("session-row", |s| s.visible())
            };
            let row = session_row(("session", n), info.title.clone().into(), Some(ago(info.modified)), is_current, &t)
                .group("session-row")
                .tooltip(Tooltip::with_meta(info.title.clone(), ago(info.modified)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.confirm_delete = None;
                    if is_current || busy {
                        return;
                    }
                    this.set_session(Session::resume(kind2, cwd2.clone(), open_id.clone()));
                    cx.notify();
                }))
                .child(
                    delete
                        .when(busy && is_current, |d| d.opacity(0.4))
                        .tooltip(Tooltip::text("Delete this session's transcript"))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            if busy && is_current {
                                return;
                            }
                            if this.confirm_delete.as_deref() != Some(del_id.as_str()) {
                                this.confirm_delete = Some(del_id.clone());
                                return cx.notify();
                            }
                            this.confirm_delete = None;
                            if current2.as_deref() == Some(del_id.as_str()) {
                                this.set_session(Session::new(kind2, cwd3.clone()));
                            }
                            if let Err(e) = session::delete_session(&cwd3, &del_id) {
                                this.notice = Some(e);
                            }
                            this.sessions = None;
                            cx.notify();
                        })),
                );
            rows.push(row.into_any_element());
        }

        div()
            .flex()
            .flex_col()
            .size_full()
            .child(header)
            .child(div().id("session-list").flex_1().min_h_0().overflow_y_scroll().px_1p5().pb_1().children(rows))
            // Project sessions have Claude settings; chat has no tools.
            .when(kind == Kind::Code, |d| {
                d.child(divider(cx)).child(
                    div().p_1p5().child(
                        ghost_button("claude-settings", cx)
                            .w_full()
                            .text_color(t.text_muted)
                            .child(icon(Icon::Settings, t.text_muted))
                            .child("Claude settings")
                            .tooltip(Tooltip::text("Permissions, mode and effort for this project"))
                            .on_click(cx.listener(move |this, _, _, cx| this.open_settings(cwd.clone(), cx))),
                    ),
                )
            })
    }

    fn render_toolbar(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = theme(cx).clone();
        let project = self.active_project().map(|p| (p.path.clone(), p.view));
        let cost = self.active_session().map_or(0.0, |s| s.total_cost);
        let collapsed = self.saved.sessions_collapsed;
        let bar = div()
            .flex()
            .flex_none()
            .items_center()
            .gap_1()
            .h(px(36.))
            .px_2()
            .border_b_1()
            .border_color(t.border_variant)
            .when(collapsed, |d| {
                d.child(
                    icon_button("show-sessions", Icon::PanelLeft, cx)
                        .tooltip(Tooltip::with_meta("Show sessions", "Ctrl+B"))
                        .on_click(cx.listener(|this, _, window, cx| this.toggle_sessions(&ToggleSessions, window, cx))),
                )
            });
        let Some((path, view)) = project else {
            return bar
                .child(div().px_1().text_sm().font_weight(FontWeight::MEDIUM).child("Chat"))
                .child(div().flex_1())
                .when(cost > 0.0, |d| d.child(cost_label(cost, &t)));
        };
        let segment = |id: &'static str, label: &'static str, which: Icon, v: View, cx: &mut Context<Self>| {
            let selected = view == v;
            let hover = t.hover;
            div()
                .id(id)
                .flex()
                .items_center()
                .gap_1p5()
                .h(px(24.))
                .px_2()
                .rounded_md()
                .text_sm()
                .cursor_pointer()
                .text_color(if selected { t.text } else { t.text_muted })
                .when(selected, |d| d.bg(t.active))
                .when(!selected, |d| d.hover(move |s| s.bg(hover)))
                .child(icon(which, if selected { t.text } else { t.text_muted }).size_3p5())
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Active::Project(i) = this.saved.active {
                        if let Some(p) = this.saved.projects.get_mut(i) {
                            p.view = v;
                        }
                    }
                    this.save();
                    cx.notify();
                }))
        };
        let open = |id: &'static str, which: Icon, tip: &'static str, opener: Opener, path: PathBuf, cx: &mut Context<Self>| {
            icon_button(id, which, cx).tooltip(Tooltip::text(tip)).on_click(cx.listener(move |this, _, _, cx| {
                open_project(&mut this.notice, opener, &path);
                cx.notify();
            }))
        };
        bar.child(segment("view-code", "Code", Icon::Message, View::Code, cx))
            .child(segment("view-git", "Git", Icon::GitBranch, View::Git, cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .px_2()
                    .truncate()
                    .text_xs()
                    .text_color(t.text_placeholder)
                    .child(path.display().to_string()),
            )
            .when(cost > 0.0, |d| d.child(cost_label(cost, &t)))
            .child(open("open-code", Icon::Code, "Open in VS Code", Opener::Code, path.clone(), cx))
            .child(open("open-files", Icon::Folder, "Open in file manager", Opener::Explorer, path.clone(), cx))
            .child(open("open-term", Icon::Terminal, "Open a terminal here", Opener::Terminal, path, cx))
    }

    fn render_item(&mut self, ix: usize, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx).clone();
        let key = self.active_key();
        let Some(session) = self.active_session() else { return div().into_any_element() };
        let Some(item) = session.items.get(ix) else { return div().into_any_element() };
        let expanded = self.expanded.contains(&(key.clone(), ix));
        // Tool traffic sits tight together; messages get room to breathe.
        let compact = matches!(item, Item::ToolUse { .. } | Item::ToolResult { .. } | Item::Permission { .. } | Item::Thinking);
        let toggle = {
            let key = key.clone();
            cx.listener(move |this, _, _, cx| {
                let k = (key.clone(), ix);
                if !this.expanded.remove(&k) {
                    this.expanded.insert(k);
                }
                if let Some(th) = this.threads.get(&key) {
                    th.list.remeasure_items(ix..ix + 1);
                }
                cx.notify();
            })
        };

        let body: AnyElement = match item {
            Item::User { text, images } => div()
                .flex()
                .flex_col()
                .gap_2()
                .px_3()
                .py_2()
                .rounded_lg()
                .border_1()
                .border_color(t.border_variant)
                .bg(t.element)
                .when(!images.is_empty(), |d| {
                    d.child(div().flex().flex_wrap().gap_2().children(images.iter().map(|i| {
                        img(i.clone()).max_h(px(120.)).max_w(px(240.)).rounded_md()
                    })))
                })
                .when(!text.is_empty(), |d| d.child(div().text_sm().child(text.clone())))
                .into_any_element(),
            Item::Assistant(text) => {
                let to_copy = text.clone();
                div()
                    .group("assistant")
                    .relative()
                    .text_sm()
                    .child(markdown::render(("md", ix), text, cx))
                    .child(
                        div().absolute().top_0().right(px(-30.)).invisible().group_hover("assistant", |s| s.visible()).child(
                            crate::ui::icon_button(("copy-msg", ix), Icon::Copy, cx)
                                .tooltip(Tooltip::text("Copy message"))
                                .on_click(move |_, _, cx| {
                                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(to_copy.clone()))
                                }),
                        ),
                    )
                    .into_any_element()
            }
            Item::Thinking => div().text_sm().italic().text_color(t.text_placeholder).child("Thinking…").into_any_element(),
            Item::ToolUse { name, summary, input } => {
                // Claude's description of the call reads as prose; a bare command or path stays mono.
                let described = serde_json::from_str::<serde_json::Value>(input)
                    .ok()
                    .is_some_and(|v| v["description"].as_str().is_some_and(|d| !d.trim().is_empty()));
                let summary_el = div().flex_1().min_w_0().truncate().child(summary.clone());
                let summary_el = if described {
                    summary_el.text_sm().text_color(t.text)
                } else {
                    summary_el.font_family(mono()).text_xs().text_color(t.text_muted)
                };
                let head = disclosure_row(("tool", ix), expanded, &t)
                    .child(icon(Icon::Terminal, t.text_muted).size_3())
                    .child(div().flex_none().text_xs().font_weight(FontWeight::MEDIUM).text_color(t.text_muted).child(name.clone()))
                    .child(summary_el)
                    .on_click(toggle);
                div()
                    .flex()
                    .flex_col()
                    .child(head)
                    .when(expanded, |d| d.child(code_box(("tool-body", ix).into(), input.clone(), &t)))
                    .into_any_element()
            }
            Item::ToolResult { content, is_error } => {
                let first: String = content.lines().next().unwrap_or("").chars().take(160).collect();
                let color = if *is_error { t.error } else { t.text_placeholder };
                let head = disclosure_row(("result", ix), expanded, &t)
                    .pl_6()
                    .child(div().flex_1().min_w_0().truncate().font_family(mono()).text_xs().text_color(color).child(first))
                    .on_click(toggle);
                div()
                    .flex()
                    .flex_col()
                    .child(head)
                    .when(expanded, |d| d.child(code_box(("result-body", ix).into(), content.clone(), &t)))
                    .into_any_element()
            }
            // Answered in the prompt that takes the composer's place; the
            // transcript keeps a one-line record.
            Item::Permission { tool, input, state, .. } => {
                let (which, note, color) = match state {
                    PermState::Pending => (Icon::Dot, "waiting for you below".to_string(), t.warning),
                    PermState::Allowed(n) => (Icon::Check, n.clone(), t.success),
                    PermState::Denied(n) => (Icon::Close, n.clone(), t.error),
                };
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_1()
                    .text_xs()
                    .child(icon(which, color).size_3p5())
                    .child(div().text_color(t.text_muted).child(prompt::title(tool, input)))
                    .child(div().text_color(color).child(note))
                    .into_any_element()
            }
            Item::Result { cost, duration_ms, is_error } => div()
                .px_1()
                .text_xs()
                .text_color(t.text_placeholder)
                .child(format!(
                    "Done in {:.1}s · ${cost:.4}{}",
                    *duration_ms as f64 / 1000.0,
                    if *is_error { " · error" } else { "" }
                ))
                .into_any_element(),
            Item::Error(e) => div().px_1().text_sm().text_color(t.error).child(e.clone()).into_any_element(),
        };
        div()
            .w_full()
            .flex()
            .justify_center()
            .px_6()
            .map(|d| if compact { d.py_0p5() } else { d.py_2() })
            .child(div().w_full().max_w(THREAD_WIDTH).child(body))
            .into_any_element()
    }

    fn render_thread(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx).clone();
        let key = self.active_key();
        let count = self.active_session().map_or(0, |s| s.items.len());
        let busy = self.active_session().is_some_and(|s| s.busy);
        if count == 0 {
            let msg = match self.saved.active {
                Active::Chat => "Start a conversation. Uses your Claude Code login, no API key needed.",
                Active::Project(_) => "Ask Claude Code to work on this project.",
            };
            return div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(t.text_placeholder)
                .child(msg)
                .into_any_element();
        }
        let thread = self.threads.entry(key).or_insert_with(|| {
            let list = ListState::new(0, ListAlignment::Top, px(1000.));
            list.set_follow_mode(FollowMode::Tail);
            Thread { list, count: 0 }
        });
        if count < thread.count {
            thread.list.reset(count);
        } else if count > thread.count {
            thread.list.splice(thread.count..thread.count, count - thread.count);
        }
        // The last item grows while a reply streams in.
        if busy && count > 0 {
            thread.list.remeasure_items(count - 1..count);
        }
        thread.count = count;
        let state = thread.list.clone();
        list(state, cx.processor(|this, ix, window, cx| this.render_item(ix, window, cx)))
            .flex_1()
            .size_full()
            .py_2()
            .into_any_element()
    }

    fn render_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = theme(cx).clone();
        // At a glance: is Claude working, waiting on you, or idle?
        let waiting = self.prompt.is_some();
        let working = !waiting && self.active_session().is_some_and(|s| s.busy);
        let status = (working || waiting).then(|| working_bar(working, &t));
        let update = self.render_cli_update(cx);
        let shell = |child: AnyElement, focused: bool| {
            div().w_full().flex().justify_center().px_6().pb_4().pt_1().child(
                div().w_full().max_w(THREAD_WIDTH).flex().flex_col().gap_1p5().children(update).children(status).child(
                    div()
                        .w_full()
                        .p_3()
                        .rounded_lg()
                        .border_1()
                        .border_color(if focused { t.border_focused } else { t.border })
                        .bg(t.panel)
                        .shadow_sm()
                        .child(child),
                ),
            )
        };
        if let Some(p) = &self.prompt {
            let focused = p.view.read(cx).focus_handle(cx).contains_focused(window, cx);
            return shell(p.view.clone().into_any_element(), focused);
        }

        let session = self.active_session();
        let busy = session.is_some_and(|s| s.busy);
        let mode = session.filter(|s| s.kind == Kind::Code).map(|s| s.mode);
        fetch_models(cx);
        // The default shows what it resolves to, e.g. "Default (Opus 5.5)": the CLI's
        // description of it starts with the model's name.
        let model_label = match current_model(&self.saved.model) {
            Some(m) if self.saved.model.is_empty() => match m.description.split(" · ").next().filter(|s| !s.is_empty()) {
                Some(resolved) if m.description.contains(" · ") => format!("Default ({resolved})"),
                _ => "Default".to_string(),
            },
            Some(m) => m.name,
            None => MODELS.iter().find(|m| m.0 == self.saved.model).map_or(self.saved.model.clone(), |m| m.1.to_string()),
        };
        // Effort lives in the project's Claude settings (a tiny file); chat has none.
        let effort_label = self.active_project().filter(|_| mode.is_some()).map(|p| {
            let effort = ClaudeSettings::load(&p.path).effort;
            match project::EFFORTS.iter().find(|e| e.0 == effort) {
                Some((id, name)) if !id.is_empty() => format!("{name} effort"),
                _ => "Default effort".to_string(),
            }
        });
        let in_project = matches!(self.saved.active, Active::Project(_));
        let placeholder = if in_project {
            "Ask Claude Code to do something in this project…"
        } else {
            "Message Claude…"
        };
        self.composer.update(cx, |c, _| c.set_placeholder(placeholder));
        let focused = self.composer.read(cx).is_focused(window);
        let can_send = !busy && (!self.composer.read(cx).text().trim().is_empty() || !self.attachments.is_empty());
        let max_h = window.viewport_size().height * 0.4;

        let attachments = div().flex().flex_wrap().gap_2().children(self.attachments.iter().enumerate().map(|(i, a)| {
            div()
                .id(("attachment", i))
                .rounded_md()
                .border_1()
                .border_color(t.border)
                .overflow_hidden()
                .cursor_pointer()
                .hover(|s| s.opacity(0.7))
                .child(img(a.image.clone()).h(px(56.)).max_w(px(120.)))
                .tooltip(Tooltip::text("Click to remove"))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.attachments.remove(i);
                    cx.notify();
                }))
        }));

        let send = if busy {
            div()
                .id("stop")
                .flex()
                .items_center()
                .gap_1p5()
                .h(px(26.))
                .px_2()
                .rounded_md()
                .border_1()
                .border_color(t.border)
                .text_sm()
                .cursor_pointer()
                .hover(|s| s.bg(t.hover))
                .child(spinner("busy", t.accent, px(14.)))
                .child(icon(Icon::Stop, t.text).size_3())
                .child("Stop")
                .tooltip(Tooltip::with_meta("Interrupt Claude", "Esc · change it in keymap.json"))
                .on_click(cx.listener(|this, _, _, cx| this.interrupt(cx)))
        } else {
            div()
                .id("send")
                .flex()
                .items_center()
                .justify_center()
                .size(px(26.))
                .rounded_md()
                .bg(if can_send { t.accent } else { t.active })
                .when(can_send, |d| d.cursor_pointer().hover(|s| s.opacity(0.9)))
                .child(icon(Icon::ArrowUp, if can_send { t.editor } else { t.text_placeholder }))
                .tooltip(Tooltip::with_meta("Send", "Enter · Shift+Enter for a new line"))
                .on_click(cx.listener(|this, _, window, cx| this.send(window, cx)))
        };

        let body = div()
            .flex()
            .flex_col()
            .gap_2()
            .when(!self.attachments.is_empty(), |d| d.child(attachments))
            .child(div().id("composer-scroll").max_h(max_h).overflow_y_scroll().text_sm().child(self.composer.clone()))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        icon_button("attach", Icon::Paperclip, cx)
                            .tooltip(Tooltip::with_meta("Attach images or files", "or paste / drop them"))
                            .on_click(cx.listener(|this, _, _, cx| this.attach_files(cx))),
                    )
                    .child(
                        ghost_button("model", cx)
                            .text_color(t.text_muted)
                            .child(model_label)
                            .child(icon(Icon::ChevronDown, t.text_placeholder).size_3())
                            .tooltip(Tooltip::text("Model, for every tab; applies from the next session start"))
                            .on_click(cx.listener(|this, ev: &ClickEvent, _, cx| {
                                this.open_menu(Menu::Model, ev.position(), Anchor::BottomLeft, cx)
                            })),
                    )
                    .when_some(effort_label, |d, effort| {
                        d.child(
                            ghost_button("effort", cx)
                                .text_color(t.text_muted)
                                .child(effort)
                                .child(icon(Icon::ChevronDown, t.text_placeholder).size_3())
                                .tooltip(Tooltip::text("How hard Claude thinks in this project (--effort)"))
                                .on_click(cx.listener(|this, ev: &ClickEvent, _, cx| {
                                    this.open_menu(Menu::Effort, ev.position(), Anchor::BottomLeft, cx)
                                })),
                        )
                    })
                    .when_some(mode, |d, mode| {
                        d.child(
                            ghost_button("mode", cx)
                                .text_color(if mode == Mode::Plan { t.info } else { t.text_muted })
                                .child(mode.label())
                                .child(icon(Icon::ChevronDown, t.text_placeholder).size_3())
                                .tooltip(Tooltip::with_meta(mode.help(), "Shift+Tab to cycle"))
                                .on_click(cx.listener(|this, ev: &ClickEvent, _, cx| {
                                    this.open_menu(Menu::Mode, ev.position(), Anchor::BottomLeft, cx)
                                })),
                        )
                    })
                    .child(div().flex_1())
                    .child(send),
            );
        shell(body.into_any_element(), focused)
    }

    fn render_menu(&mut self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let open = self.menu.as_ref()?;
        let (which, at, anchor) = (open.which.clone(), open.at, open.anchor);
        let t = theme(cx).clone();
        let mut items: Vec<AnyElement> = Vec::new();
        match &which {
            Menu::Model => {
                // The account's own list from the CLI; the short built-in one until it arrives.
                let list = model_list().unwrap_or_else(|| {
                    MODELS
                        .iter()
                        .map(|(id, name)| session::ModelInfo {
                            value: if id.is_empty() { "default".into() } else { id.to_string() },
                            name: name.to_string(),
                            description: String::new(),
                            efforts: Vec::new(),
                        })
                        .collect()
                });
                for (n, m) in list.into_iter().enumerate() {
                    let id = if m.value == "default" { String::new() } else { m.value.clone() };
                    let selected = self.saved.model == id;
                    let hover = t.hover;
                    items.push(
                        div()
                            .id(("model", n))
                            .flex()
                            .items_center()
                            .gap_2()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .cursor_pointer()
                            .hover(move |s| s.bg(hover))
                            .when(selected, |d| d.bg(t.active))
                            .child(
                                div()
                                    .flex_1()
                                    .flex()
                                    .flex_col()
                                    .child(div().text_sm().text_color(t.text).child(m.name.clone()))
                                    .when(!m.description.is_empty(), |d| {
                                        d.child(div().text_xs().text_color(t.text_muted).child(m.description.clone()))
                                    }),
                            )
                            .when(selected, |d| d.child(icon(Icon::Check, t.text_muted)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.saved.model = id.clone();
                                this.menu = None;
                                this.save();
                                cx.notify();
                            }))
                            .into_any_element(),
                    );
                }
            }
            Menu::Theme => {
                for (n, (th, name)) in THEMES.iter().enumerate() {
                    let th = *th;
                    items.push(
                        menu_item(("theme", n), *name, self.saved.theme == th, cx)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.saved.theme = th;
                                this.menu = None;
                                this.save();
                                theme::set(th, cx);
                                cx.refresh_windows();
                            }))
                            .into_any_element(),
                    );
                }
            }
            Menu::Mode => {
                let current = self.active_session().map(|s| s.mode);
                for (n, m) in Mode::ALL.into_iter().enumerate() {
                    items.push(
                        menu_item(("mode", n), m.label(), current == Some(m), cx)
                            .h_auto()
                            .py_1()
                            .child(div())
                            .tooltip(Tooltip::text(m.help()))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(s) = this.session_mut() {
                                    s.set_mode(m);
                                }
                                this.menu = None;
                                cx.notify();
                            }))
                            .into_any_element(),
                    );
                }
            }
            Menu::Effort => {
                // From the settings modal, or from the composer for the active project.
                let path = self.settings.as_ref().map(|s| s.path.clone()).or_else(|| self.active_key());
                let current = match (&self.settings, &path) {
                    (Some(s), _) => s.claude.effort.clone(),
                    (None, Some(p)) => ClaudeSettings::load(p).effort,
                    _ => String::new(),
                };
                // Only the levels the selected model takes, once the CLI has said which.
                let allowed = current_model(&self.saved.model).map(|m| m.efforts);
                let levels = project::EFFORTS
                    .iter()
                    .filter(|(id, _)| id.is_empty() || allowed.as_ref().is_none_or(|a| a.iter().any(|e| e == id)));
                for (n, (id, name)) in levels.enumerate() {
                    let id = id.to_string();
                    let path = path.clone();
                    items.push(
                        menu_item(("effort", n), *name, current == id, cx)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let id = id.clone();
                                this.menu = None;
                                if this.settings.is_some() {
                                    return this.edit_settings(cx, |v| v.claude.effort = id);
                                }
                                let Some(path) = path.clone() else { return };
                                let mut claude = ClaudeSettings::load(&path);
                                if claude.effort != id {
                                    claude.effort = id;
                                    if let Err(e) = claude.save(&path) {
                                        this.notice = Some(e);
                                    }
                                    // The CLI reads it at startup: restart once idle; `--resume` keeps the conversation.
                                    this.restart_when_idle.insert(path);
                                    this.restart_idle();
                                }
                                cx.notify();
                            }))
                            .into_any_element(),
                    );
                }
            }
            Menu::Tab(i) => {
                let i = *i;
                let path = self.saved.projects.get(i).map(|p| p.path.clone()).unwrap_or_default();
                for (n, (label, opener)) in [
                    ("Open in VS Code", Opener::Code),
                    ("Open in file manager", Opener::Explorer),
                    ("Open terminal", Opener::Terminal),
                ]
                .into_iter()
                .enumerate()
                {
                    let path = path.clone();
                    items.push(
                        menu_item(("tab-menu", n), label, false, cx)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                open_project(&mut this.notice, opener, &path);
                                this.menu = None;
                                cx.notify();
                            }))
                            .into_any_element(),
                    );
                }
                items.push(div().my_1().child(divider(cx)).into_any_element());
                items.push(
                    menu_item("tab-close", "Close", false, cx)
                        .text_color(t.error)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.menu = None;
                            this.close_project(i);
                            cx.notify();
                        }))
                        .into_any_element(),
                );
            }
        }
        Some(deferred(
            anchored().position(at).anchor(anchor).snap_to_window_with_margin(px(8.)).child(
                popover(cx)
                    .min_w(px(200.))
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        this.menu = None;
                        cx.notify();
                    }))
                    .children(items),
            ),
        ))
    }

    fn render_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let view = self.settings.as_ref()?;
        let t = theme(cx).clone();
        const SECTIONS: [&str; 2] = ["Permissions", "Model"];
        let section = view.section.min(SECTIONS.len() - 1);
        let path = view.path.clone();
        let vp = window.viewport_size();

        let nav = div().flex().flex_col().gap_0p5().w(px(170.)).flex_none().p_2().border_r_1().border_color(t.border_variant).children(
            SECTIONS.iter().enumerate().map(|(i, name)| {
                menu_item(("settings-section", i), *name, false, cx)
                    .when(i == section, |d| d.bg(t.active))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(s) = &mut this.settings {
                            s.section = i;
                        }
                        cx.notify();
                    }))
            }),
        );

        let content = match section {
            0 => {
                let modes = Mode::ALL.into_iter().enumerate().map(|(n, m)| {
                    let selected = view.claude.mode == m;
                    let hover = t.hover;
                    div()
                        .id(("default-mode", n))
                        .flex()
                        .items_start()
                        .gap_2()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .cursor_pointer()
                        .hover(move |s| s.bg(hover))
                        .child(radio(selected, &t))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .child(div().text_sm().child(m.label()))
                                .child(div().text_xs().text_color(t.text_muted).child(m.help())),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| this.edit_settings(cx, |v| v.claude.mode = m)))
                });
                let rule_list = |deny: bool, rules: &[String], cx: &mut Context<Self>| {
                    let title = if deny { "Deny" } else { "Allow" };
                    div()
                        .flex()
                        .flex_col()
                        .gap_0p5()
                        .child(div().text_xs().text_color(t.text_muted).child(title))
                        .when(rules.is_empty(), |d| d.child(div().pl_2().text_sm().text_color(t.text_placeholder).child("None")))
                        .children(rules.iter().enumerate().map(|(i, rule)| {
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .pl_2()
                                .rounded_md()
                                .hover(|s| s.bg(t.hover))
                                .child(div().flex_1().min_w_0().truncate().font_family(mono()).text_sm().child(rule.clone()))
                                .child(
                                    icon_button(SharedString::from(format!("rm-{title}-{i}")), Icon::Close, cx)
                                        .tooltip(Tooltip::text("Remove this rule"))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.edit_settings(cx, |v| {
                                                let list = if deny { &mut v.perms.deny } else { &mut v.perms.allow };
                                                if i < list.len() {
                                                    list.remove(i);
                                                }
                                            })
                                        })),
                                )
                        }))
                };
                let (allow, deny, dirs) = (view.perms.allow.clone(), view.perms.deny.clone(), view.claude.add_dirs.clone());
                div()
                    .flex()
                    .flex_col()
                    .gap_5()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(heading("Default mode", "What new sessions start in. Switch a running session from the composer.", &t))
                            .children(modes),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(heading(
                                "Rules",
                                "In Claude Code's syntax: Bash(cargo test:*), Edit, WebFetch(domain:docs.rs). They add to your own Claude settings; a deny anywhere wins.",
                                &t,
                            ))
                            .child(rule_list(false, &allow, cx))
                            .child(rule_list(true, &deny, cx))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .flex_1()
                                            .px_2()
                                            .py_1()
                                            .rounded_md()
                                            .border_1()
                                            .border_color(t.border)
                                            .bg(t.editor)
                                            .text_sm()
                                            .child(self.new_rule.clone()),
                                    )
                                    .child(outline_button("add-allow", "Allow", cx).on_click(cx.listener(|this, _, _, cx| this.add_rule(false, cx))))
                                    .child(outline_button("add-deny", "Deny", cx).on_click(cx.listener(|this, _, _, cx| this.add_rule(true, cx)))),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(heading("Additional directories", "Folders outside the project Claude may read and edit (--add-dir).", &t))
                            .children(dirs.iter().enumerate().map(|(i, d)| {
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .pl_2()
                                    .rounded_md()
                                    .hover(|s| s.bg(t.hover))
                                    .child(div().flex_1().min_w_0().truncate().font_family(mono()).text_sm().child(d.display().to_string()))
                                    .child(
                                        icon_button(("rm-dir", i), Icon::Close, cx).tooltip(Tooltip::text("Remove")).on_click(cx.listener(
                                            move |this, _, _, cx| {
                                                this.edit_settings(cx, |v| {
                                                    if i < v.claude.add_dirs.len() {
                                                        v.claude.add_dirs.remove(i);
                                                    }
                                                })
                                            },
                                        )),
                                    )
                            }))
                            .child(
                                div().pt_1().child(
                                    outline_button("add-dir", "Add folder…", cx).on_click(cx.listener(|this, _, _, cx| this.add_dir(cx))),
                                ),
                            ),
                    )
            }
            _ => {
                let effort = project::EFFORTS.iter().find(|e| e.0 == view.claude.effort).map_or("Default", |e| e.1);
                div().flex().flex_col().gap_2().child(heading("Effort", "How hard Claude thinks (--effort). Default uses your Claude settings.", &t)).child(
                    div().flex().child(
                        outline_button("effort", effort, cx)
                            .gap_1()
                            .child(icon(Icon::ChevronDown, t.text_placeholder).size_3())
                            .on_click(cx.listener(|this, ev: &ClickEvent, _, cx| {
                                this.open_menu(Menu::Effort, ev.position(), Anchor::TopLeft, cx)
                            })),
                    ),
                )
                .child(div().pt_2().text_xs().text_color(t.text_muted).child("The model and effort are also in the composer. The model applies to every tab."))
            }
        };

        let card = div()
            .id("settings-card")
            .occlude()
            .flex()
            .flex_col()
            .w(px(760.).min(vp.width - px(80.)))
            .h(px(560.).min(vp.height - px(100.)))
            .rounded_lg()
            .border_1()
            .border_color(t.border)
            .bg(t.elevated)
            .shadow_lg()
            .overflow_hidden()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap_2()
                    .h(px(44.))
                    .pl_4()
                    .pr_2()
                    .border_b_1()
                    .border_color(t.border_variant)
                    .child(icon(Icon::Settings, t.text_muted))
                    .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child("Claude settings"))
                    .child(div().text_sm().text_color(t.text_muted).child(project_name(&path)))
                    .child(div().flex_1())
                    .child(
                        icon_button("close-settings", Icon::Close, cx)
                            .tooltip(Tooltip::with_meta("Close", "Esc"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.settings = None;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div().flex().flex_1().min_h_0().child(nav).child(
                    div()
                        .id("settings-body")
                        .flex_1()
                        .overflow_y_scroll()
                        .p_5()
                        .child(content)
                        .child(
                            div()
                                .pt_6()
                                .text_xs()
                                .text_color(t.text_placeholder)
                                .child(format!("Stored in {}", project::dir(&path).display())),
                        ),
                ),
            );

        Some(
            div()
                .id("modal-backdrop")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::black().opacity(0.45))
                .occlude()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.settings = None;
                        cx.notify();
                    }),
                )
                .child(card),
        )
    }

    /// The Claude Code update strip above the composer: an offer, progress, or the outcome.
    fn render_cli_update(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let t = theme(cx).clone();
        let (lead, text, color): (AnyElement, String, _) = match &self.cli_update {
            UpdateState::Idle => return None,
            UpdateState::Available { installed, latest } => (
                icon(Icon::ArrowUp, t.info).size_3p5().into_any_element(),
                format!("Claude Code {latest} is available (you have {installed})"),
                t.text,
            ),
            UpdateState::Updating { latest } => (
                spinner("cli-update-spin", t.accent, px(14.)).into_any_element(),
                format!("Updating Claude Code to {latest}…"),
                t.text_muted,
            ),
            UpdateState::Updated { version } => (
                icon(Icon::Check, t.success).size_3p5().into_any_element(),
                format!("Claude Code updated to {version}. Sessions pick it up with their next message."),
                t.text,
            ),
            UpdateState::Failed(e) => (icon(Icon::Dot, t.error).size_3p5().into_any_element(), e.clone(), t.text),
        };
        let dismiss = || {
            icon_button("cli-update-dismiss", Icon::Close, cx).on_click(cx.listener(|this, _, _, cx| {
                this.cli_update = UpdateState::Idle;
                cx.notify();
            }))
        };
        let actions = match &self.cli_update {
            UpdateState::Available { .. } => div()
                .flex()
                .items_center()
                .gap_1()
                .child(
                    ghost_button("cli-skip", cx)
                        .text_xs()
                        .text_color(t.text_muted)
                        .child("Skip this version")
                        .on_click(cx.listener(|this, _, _, cx| this.skip_cli_version(cx))),
                )
                .child(
                    outline_button("cli-update", "Update", cx)
                        .tooltip(Tooltip::text("Runs `claude update`; idle sessions restart on the new version"))
                        .on_click(cx.listener(|this, _, _, cx| this.update_cli(cx))),
                )
                .into_any_element(),
            UpdateState::Updating { .. } => div().into_any_element(),
            _ => dismiss().into_any_element(),
        };
        Some(
            div()
                .flex()
                .items_center()
                .gap_2()
                .pl_2()
                .pr_1()
                .py_1()
                .rounded_md()
                .border_1()
                .border_color(t.border_variant)
                .bg(t.panel)
                .child(lead)
                .child(div().flex_1().min_w_0().text_xs().text_color(color).child(text))
                .child(actions)
                .into_any_element(),
        )
    }

    fn render_notice(&mut self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let n = self.notice.clone()?;
        let t = theme(cx).clone();
        Some(
            div().absolute().bottom_4().right_4().max_w(px(480.)).child(
                popover(cx)
                    .flex_row()
                    .items_start()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .child(icon(Icon::Dot, t.warning).mt_0p5())
                    .child(div().flex_1().text_sm().child(n))
                    .child(icon_button("dismiss-notice", Icon::Close, cx).on_click(cx.listener(|this, _, _, cx| {
                        this.notice = None;
                        cx.notify();
                    }))),
            ),
        )
    }
}

impl Focusable for KynexApp {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for KynexApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx).clone();
        if let Active::Project(i) = self.saved.active {
            if let Some(p) = self.saved.projects.get_mut(i).filter(|p| p.view == View::Settings) {
                p.view = View::Code;
            }
        }
        let view = self.active_project().map(|p| p.view);
        // Create (or resume) the project's session as soon as its tab is shown,
        // so a restored transcript is visible before the first message.
        if view != Some(View::Git) {
            let _ = self.session_mut();
        }
        self.sync_prompt(window, cx);
        // Shortcuts (Esc, Ctrl+B…) only reach the workspace through the focused
        // element, so never leave the window with nothing focused.
        if window.focused(cx).is_none() {
            self.focus.focus(window, cx);
        }

        let main = if view == Some(View::Git) {
            let path = self.active_project().map(|p| p.path.clone()).unwrap();
            let git = self.git.entry(path.clone()).or_insert_with(|| cx.new(|cx| GitView::new(path, cx))).clone();
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .bg(t.editor)
                .child(self.render_toolbar(cx))
                .child(div().flex_1().min_h_0().child(git))
                .into_any_element()
        } else {
            let width = px(self.saved.sessions_width);
            let panel = (!self.saved.sessions_collapsed).then(|| {
                div()
                    .relative()
                    .flex_none()
                    .w(width)
                    .h_full()
                    .bg(t.panel)
                    .border_r_1()
                    .border_color(t.border)
                    .child(self.render_sessions_panel(cx))
                    .child(
                        // Drag the panel's right edge to resize it.
                        div()
                            .id("sessions-resize")
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .right(px(-3.))
                            .w(px(6.))
                            .cursor_col_resize()
                            .hover(|s| s.bg(t.border_focused.opacity(0.6)))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.resizing_sessions = true;
                                    cx.stop_propagation();
                                }),
                            ),
                    )
            });
            div()
                .flex()
                .flex_1()
                .min_h_0()
                .children(panel)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .bg(t.editor)
                        .child(self.render_toolbar(cx))
                        .child(self.render_thread(cx))
                        .child(self.render_composer(window, cx)),
                )
                .into_any_element()
        };

        let menu = self.render_menu(cx);
        let settings = self.render_settings(window, cx);
        let notice = self.render_notice(cx);
        div()
            .id("workspace")
            .key_context("Workspace")
            .track_focus(&self.focus)
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .bg(t.editor)
            .text_color(t.text)
            .text_sm()
            .on_action(cx.listener(Self::open_project))
            .on_action(cx.listener(Self::next_tab))
            .on_action(cx.listener(Self::prev_tab))
            .on_action(cx.listener(Self::toggle_sessions))
            .on_action(cx.listener(Self::cycle_mode))
            .on_action(cx.listener(Self::interrupt_action))
            .on_key_down(cx.listener(Self::key_down))
            .on_drop(cx.listener(Self::on_drop))
            .when(self.resizing_sessions, |d| {
                d.cursor_col_resize()
                    .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                        this.saved.sessions_width = f32::from(ev.position.x).clamp(180., 480.);
                        cx.notify();
                    }))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.resizing_sessions = false;
                            this.save();
                            cx.notify();
                        }),
                    )
            })
            .child(self.render_title_bar(window, cx))
            .child(main)
            .children(notice)
            .children(settings)
            .children(menu)
    }
}

fn spinner(id: impl Into<gpui::ElementId>, color: gpui::Hsla, size: Pixels) -> gpui::AnimationElement<gpui::Svg> {
    icon(Icon::Loader, color).size(size).with_animation(
        id,
        Animation::new(Duration::from_millis(900)).repeat(),
        |svg, delta| svg.with_transformation(Transformation::rotate(percentage(delta))),
    )
}

/// The models the account can use, fetched once from the CLI. Kept in a static
/// rather than on `KynexApp` so it doesn't change the app's layout.
static MODEL_LIST: std::sync::OnceLock<Vec<session::ModelInfo>> = std::sync::OnceLock::new();
static MODELS_FETCHING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn model_list() -> Option<Vec<session::ModelInfo>> {
    MODEL_LIST.get().cloned()
}

/// The entry for a saved model choice (`""` is the CLI's default).
fn current_model(saved: &str) -> Option<session::ModelInfo> {
    let value = if saved.is_empty() { "default" } else { saved };
    MODEL_LIST.get()?.iter().find(|m| m.value == value).cloned()
}

/// Starts the one background fetch of the model list; repaints when it lands.
/// If it fails the built-in short list stays.
fn fetch_models(cx: &mut Context<KynexApp>) {
    if MODELS_FETCHING.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    cx.spawn(async move |this, cx| {
        let result = cx.background_spawn(async { session::list_models() }).await;
        match result {
            Ok(list) if !list.is_empty() => {
                let _ = MODEL_LIST.set(list);
            }
            Ok(_) => eprintln!("kynex: claude reported no models"),
            Err(e) => eprintln!("kynex: couldn't list models: {e}"),
        }
        this.update(cx, |_, cx| cx.notify()).ok();
    })
    .detach();
}

/// The strip above the composer: a label and a track with a sliding accent
/// while Claude works, or a steady warning-coloured one while it waits for you.
fn working_bar(working: bool, t: &theme::Palette) -> AnyElement {
    let color = if working { t.accent } else { t.warning };
    let label = div()
        .flex()
        .items_center()
        .gap_1p5()
        .text_xs()
        .text_color(color)
        .child(if working { spinner("working-spin", color, px(12.)).into_any_element() } else { icon(Icon::Dot, color).size_2().into_any_element() })
        .child(if working { "Working…" } else { "Waiting for you" });
    let track = div().relative().w_full().h(px(2.)).rounded_full().overflow_hidden().bg(t.border_variant);
    let track = if working {
        track.child(
            div()
                .absolute()
                .top_0()
                .h_full()
                .w(gpui::relative(0.3))
                .rounded_full()
                .bg(color)
                .with_animation(
                    "working-slide",
                    Animation::new(Duration::from_millis(1400)).repeat().with_easing(gpui::ease_in_out),
                    |bar, delta| bar.left(gpui::relative(delta * 1.3 - 0.3)),
                ),
        )
    } else {
        track.child(div().size_full().bg(color.opacity(0.6)))
    };
    div().flex().flex_col().gap_1().px_1().child(label).child(track).into_any_element()
}

fn cost_label(cost: f64, t: &theme::Palette) -> impl IntoElement + use<> {
    div().px_1().text_xs().font_family(mono()).text_color(t.text_placeholder).child(format!("${cost:.3}"))
}

fn session_row(
    id: impl Into<gpui::ElementId>,
    title: SharedString,
    when: Option<String>,
    current: bool,
    t: &theme::Palette,
) -> gpui::Stateful<gpui::Div> {
    let hover = t.hover;
    div()
        .id(id)
        .flex()
        .items_center()
        .gap_1()
        .h(px(30.))
        .pl_2()
        .pr_1()
        .rounded_md()
        .cursor_pointer()
        .text_sm()
        .when(current, |d| d.bg(t.active))
        .when(!current, |d| d.hover(move |s| s.bg(hover)))
        .child(div().flex_1().min_w_0().truncate().text_color(if current { t.text } else { t.text_muted }).child(title))
        .when_some(when, |d, w| d.child(div().flex_none().text_xs().text_color(t.text_placeholder).child(w)))
}

fn disclosure_row(id: impl Into<gpui::ElementId>, expanded: bool, t: &theme::Palette) -> gpui::Stateful<gpui::Div> {
    let hover = t.hover;
    div()
        .id(id)
        .flex()
        .items_center()
        .gap_1p5()
        .h(px(22.))
        .px_1()
        .rounded_md()
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .child(icon(if expanded { Icon::ChevronDown } else { Icon::ChevronRight }, t.text_placeholder).size_3())
}

fn code_box(id: gpui::ElementId, text: String, t: &theme::Palette) -> impl IntoElement + use<> {
    div()
        .id(id)
        .ml_6()
        .mt_1()
        .max_h(px(320.))
        .overflow_y_scroll()
        .rounded_md()
        .border_1()
        .border_color(t.border_variant)
        .bg(t.panel)
        .px_3()
        .py_2()
        .font_family(mono())
        .text_xs()
        .text_color(t.text_muted)
        .child(text)
}

fn heading(title: &'static str, help: &'static str, t: &theme::Palette) -> impl IntoElement + use<> {
    div()
        .flex()
        .flex_col()
        .gap_0p5()
        .pb_1()
        .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(title))
        .child(div().text_xs().text_color(t.text_muted).child(help))
}

fn radio(selected: bool, t: &theme::Palette) -> impl IntoElement + use<> {
    div()
        .mt_0p5()
        .size(px(14.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .border_1()
        .border_color(if selected { t.accent } else { t.border })
        .when(selected, |d| d.child(div().size(px(6.)).rounded_full().bg(t.accent)))
}

/// "just now", "5m ago", "3h ago", "2d ago".
fn ago(t: std::time::SystemTime) -> String {
    let secs = t.elapsed().map_or(0, |d| d.as_secs());
    match secs {
        0..60 => "just now".into(),
        60..3600 => format!("{}m ago", secs / 60),
        3600..86400 => format!("{}h ago", secs / 3600),
        _ => format!("{}d ago", secs / 86400),
    }
}

fn is_image(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp"))
}

#[derive(Clone, Copy)]
enum Opener {
    Code,
    Explorer,
    Terminal,
}

/// Opens a project folder in another app. WSL projects open VS Code over
/// Remote - WSL and the terminal inside the distro.
fn open_project(notice: &mut Option<String>, opener: Opener, path: &Path) {
    if let Some(w) = wsl::parse(path) {
        match opener {
            Opener::Code => {
                let remote = format!("wsl+{}", w.distro);
                open_with(notice, "code", &["--remote".as_ref(), remote.as_ref(), w.path.as_ref()]);
            }
            Opener::Explorer => open_with(notice, "explorer", &[path.as_os_str()]),
            Opener::Terminal => {
                open_with(notice, "wt", &["wsl.exe".as_ref(), "-d".as_ref(), w.distro.as_ref(), "--cd".as_ref(), w.path.as_ref()]);
            }
        }
        return;
    }
    match opener {
        Opener::Code => open_with(notice, "code", &[path.as_os_str()]),
        Opener::Explorer if cfg!(target_os = "windows") => open_with(notice, "explorer", &[path.as_os_str()]),
        Opener::Explorer if cfg!(target_os = "macos") => open_with(notice, "open", &[path.as_os_str()]),
        Opener::Explorer => open_with(notice, "xdg-open", &[path.as_os_str()]),
        Opener::Terminal if cfg!(target_os = "windows") => open_with(notice, "wt", &["-d".as_ref(), path.as_os_str()]),
        Opener::Terminal if cfg!(target_os = "macos") => {
            open_with(notice, "open", &["-a".as_ref(), "Terminal".as_ref(), path.as_os_str()])
        }
        Opener::Terminal => open_with(notice, "x-terminal-emulator", &["--working-directory".as_ref(), path.as_os_str()]),
    }
}

fn open_with(notice: &mut Option<String>, program: &str, args: &[&std::ffi::OsStr]) {
    // `code` is a .cmd shim on Windows, so go through cmd to resolve it.
    let result = if cfg!(windows) && program == "code" {
        Command::new("cmd").arg("/c").arg(program).args(args).spawn()
    } else {
        Command::new(program).args(args).spawn()
    };
    if let Err(e) = result {
        *notice = Some(format!("Couldn't launch {program}: {e}"));
    }
}
