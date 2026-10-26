use std::collections::{HashMap, HashSet};
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::Command;

use base64::Engine;
use eframe::egui::{self, RichText};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use serde::{Deserialize, Serialize};

use crate::chrome;
use crate::git::GitView;
use crate::project::{self, ClaudeSettings, Mode, Permissions};
use crate::prompt;
use crate::session::{self, Attachment, Item, Kind, PermState, Session, SessionInfo};
use crate::theme::{self, THEMES, Theme};
use crate::wsl;

const MODELS: [(&str, &str); 4] = [("", "Default"), ("opus", "Opus"), ("sonnet", "Sonnet"), ("haiku", "Haiku")];
const MAX_IMAGE_EDGE: u32 = 1568;
const COMPOSER_ID: &str = "composer-input";
const SETTINGS_MODAL: &str = "claude-settings";

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
    /// Old Settings tab, now the ⚙ Claude settings modal. Kept so saved state
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

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Saved {
    projects: Vec<ProjectState>,
    active: Active,
    model: String,
    chat_session_id: Option<String>,
    theme: Theme,
    sessions_collapsed: bool,
}

pub struct KynexApp {
    saved: Saved,
    code: HashMap<PathBuf, Session>,
    chat: Session,
    git: HashMap<PathBuf, GitView>,
    input: String,
    attachments: Vec<Attachment>,
    md: CommonMarkCache,
    notice: Option<String>,
    last_active: Active,
    switched_at: f64,
    sessions: Option<SessionList>,
    /// Session id whose delete button was clicked once and is awaiting confirmation.
    confirm_delete: Option<String>,
    /// The project settings being edited, loaded when the settings modal opens.
    settings: Option<SettingsView>,
    /// Projects whose rules changed mid-reply; their process restarts once it's idle.
    restart_when_idle: HashSet<PathBuf>,
}

struct SettingsView {
    path: PathBuf,
    perms: Permissions,
    claude: ClaudeSettings,
    new_rule: String,
}

/// Past sessions for the active conversation's folder, re-read when the folder
/// or the current session changes, or a reply starts or finishes.
struct SessionList {
    cwd: PathBuf,
    current: Option<String>,
    busy: bool,
    items: Vec<SessionInfo>,
}

enum SessionAction {
    Switch(String),
    New,
    Delete(String),
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
    pub fn new(cc: &eframe::CreationContext) -> Self {
        let dir = data_dir();
        let _ = std::fs::create_dir_all(dir.join("chat"));
        let mut saved: Saved = std::fs::read_to_string(dir.join("state.json"))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        if matches!(saved.active, Active::Project(i) if i >= saved.projects.len()) {
            saved.active = Active::Chat;
        }
        theme::apply(&cc.egui_ctx, saved.theme);
        let mut chat = match saved.chat_session_id.clone() {
            Some(id) => Session::resume(Kind::Chat, dir.join("chat"), id),
            None => Session::new(Kind::Chat, dir.join("chat")),
        };
        chat.model = saved.model.clone();
        Self {
            saved,
            code: HashMap::new(),
            chat,
            git: HashMap::new(),
            input: String::new(),
            attachments: Vec::new(),
            md: CommonMarkCache::default(),
            notice: None,
            last_active: Active::Chat,
            switched_at: f64::NEG_INFINITY,
            sessions: None,
            confirm_delete: None,
            settings: None,
            restart_when_idle: HashSet::new(),
        }
    }

    fn save(&self) {
        if let Ok(s) = serde_json::to_string_pretty(&self.saved) {
            let _ = std::fs::write(data_dir().join("state.json"), s);
        }
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
        let model = self.saved.model.clone();
        Some(self.code.entry(path.clone()).or_insert_with(|| {
            let mut s = match resume {
                Some(id) => Session::resume(Kind::Code, path, id),
                None => Session::new(Kind::Code, path),
            };
            s.model = model;
            s
        }))
    }

    /// Replaces the active conversation (the old process, if any, is stopped).
    fn set_session(&mut self, mut s: Session) {
        s.model = self.saved.model.clone();
        match self.saved.active {
            Active::Chat => self.chat = s,
            Active::Project(i) => {
                if let Some(p) = self.saved.projects.get(i) {
                    self.code.insert(p.path.clone(), s);
                }
            }
        }
    }

    /// Left panel: past sessions for the active conversation's folder, to switch
    /// to, start, or delete.
    fn sessions_panel(&mut self, ui: &mut egui::Ui) {
        let Some(s) = self.session_mut() else { return };
        let (kind, cwd, current, busy) = (s.kind, s.cwd.clone(), s.session_id.clone(), s.busy);
        // Re-read when the folder or session changes, and when a reply finishes (titles and times move).
        if self.sessions.as_ref().is_none_or(|l| l.cwd != cwd || l.current != current || l.busy != busy) {
            self.sessions = Some(SessionList { items: session::list_sessions(&cwd), cwd: cwd.clone(), current: current.clone(), busy });
        }
        let mut action = None;
        ui.horizontal(|ui| {
            if ui.add(egui::Button::new("«").frame(false)).on_hover_text("Hide sessions (Ctrl+B)").clicked() {
                self.saved.sessions_collapsed = true;
            }
            ui.label(RichText::new("Sessions").strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let r = ui
                    .add_enabled(!busy && current.is_some(), egui::Button::new("+ New"))
                    .on_hover_text("Start a new session")
                    .on_disabled_hover_text(if busy { "Stop the current reply first" } else { "Already a new session" });
                if r.clicked() {
                    action = Some(SessionAction::New);
                }
            });
        });
        ui.separator();

        // Project sessions have Claude settings (permissions so far); chat has no tools.
        if kind == Kind::Code {
            egui::Panel::bottom("sessions-footer").frame(egui::Frame::NONE).show(ui, |ui| {
                ui.separator();
                let r = ui.add(egui::Button::new("⚙  Claude settings").frame(false)).on_hover_text("Permissions for Claude in this project");
                if r.clicked() {
                    ui.data_mut(|d| d.insert_temp(egui::Id::unique(SETTINGS_MODAL), Some(cwd.clone())));
                }
            });
        }

        let Some(list) = &self.sessions else { return };
        let confirm_delete = &mut self.confirm_delete;
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            if current.is_none() {
                ui.add(egui::Button::selectable(true, "New session").min_size(egui::vec2(ui.available_width(), 0.0)));
            }
            if list.items.is_empty() {
                ui.label(RichText::new("No past sessions yet").weak());
            }
            for info in &list.items {
                let is_current = current.as_deref() == Some(info.id.as_str());
                let confirming = confirm_delete.as_deref() == Some(info.id.as_str());
                let (open, delete) = egui::Sides::new().shrink_left().show(
                    ui,
                    |ui| {
                        let width = ui.available_width();
                        ui.add_enabled(!busy || is_current, egui::Button::selectable(is_current, truncate(&info.title, 60)).truncate().min_size(egui::vec2(width, 0.0)))
                            .on_hover_text(format!("{}\n{}", info.title, ago(info.modified)))
                            .on_disabled_hover_text("Stop the current reply first")
                            .clicked()
                    },
                    |ui| {
                        let button = if confirming {
                            egui::Button::new(RichText::new("Delete?").color(theme::p().red))
                        } else {
                            egui::Button::new("✖").small().frame(false)
                        };
                        ui.add_enabled(!(busy && is_current), button)
                            .on_hover_text("Delete this session's transcript")
                            .on_disabled_hover_text("Stop the current reply first")
                            .clicked()
                    },
                );
                if open && !is_current {
                    action = Some(SessionAction::Switch(info.id.clone()));
                }
                if delete && confirming {
                    action = Some(SessionAction::Delete(info.id.clone()));
                    *confirm_delete = None;
                } else if delete {
                    *confirm_delete = Some(info.id.clone());
                } else if open {
                    *confirm_delete = None;
                }
            }
        });

        match action {
            Some(SessionAction::Switch(id)) => self.set_session(Session::resume(kind, cwd, id)),
            Some(SessionAction::New) => self.set_session(Session::new(kind, cwd)),
            Some(SessionAction::Delete(id)) => {
                if current.as_deref() == Some(id.as_str()) {
                    self.set_session(Session::new(kind, cwd.clone()));
                }
                if let Err(e) = session::delete_session(&cwd, &id) {
                    self.notice = Some(e);
                }
                self.sessions = None;
            }
            None => {}
        }
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

    fn pick_project(&mut self) {
        if let Some(dir) = rfd::FileDialog::new().pick_folder() {
            self.add_project(dir);
        }
    }

    fn tab_bar(&mut self, ui: &mut egui::Ui) {
        let mut close = None;
        let mut select = None;
        ui.horizontal(|ui| {
            ui.label(RichText::new("kynex").strong().color(theme::p().border_a));
            ui.add_space(8.0);

            let badge = self.chat.busy.then_some(theme::p().blue);
            if theme::workspace_pill(ui, 1, "chat", self.saved.active == Active::Chat, badge).clicked() {
                select = Some(Active::Chat);
            }

            for (i, p) in self.saved.projects.iter().enumerate() {
                let session = self.code.get(&p.path);
                let waiting = session.is_some_and(|s| {
                    s.items.iter().any(|it| matches!(it, Item::Permission { state: PermState::Pending, .. }))
                });
                let badge = if waiting {
                    Some(theme::p().peach)
                } else {
                    session.is_some_and(|s| s.busy).then_some(theme::p().blue)
                };
                let active = self.saved.active == Active::Project(i);
                let r = theme::workspace_pill(ui, i + 2, &project_name(&p.path), active, badge)
                    .on_hover_text(format!("{}\nMiddle-click or right-click to close", p.path.display()));
                if r.clicked() {
                    select = Some(Active::Project(i));
                }
                if r.middle_clicked() {
                    close = Some(i);
                }
                r.context_menu(|ui| {
                    if ui.button("Open in VS Code").clicked() {
                        open_project(&mut self.notice, Opener::Code, &p.path);
                        ui.close();
                    }
                    if ui.button("Open in Explorer").clicked() {
                        open_project(&mut self.notice, Opener::Explorer, &p.path);
                        ui.close();
                    }
                    if ui.button("Open terminal").clicked() {
                        open_project(&mut self.notice, Opener::Terminal, &p.path);
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("Close").clicked() {
                        close = Some(i);
                        ui.close();
                    }
                });
            }

            if ui.button("+").on_hover_text("Open a project folder (Ctrl+O)").clicked() {
                self.pick_project();
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                chrome::window_controls(ui);
                ui.add_space(8.0);
                let current = MODELS.iter().find(|m| m.0 == self.saved.model).map_or("Default", |m| m.1);
                let before = self.saved.model.clone();
                egui::ComboBox::from_id_salt("model").selected_text(current).show_ui(ui, |ui| {
                    for (id, name) in MODELS {
                        ui.selectable_value(&mut self.saved.model, id.to_string(), name);
                    }
                });
                if before != self.saved.model {
                    self.save();
                }
                ui.label(RichText::new("model").color(theme::p().overlay));
                ui.add_space(8.0);
                let current = THEMES.iter().find(|t| t.0 == self.saved.theme).map_or("Hyprland", |t| t.1);
                let before = self.saved.theme;
                egui::ComboBox::from_id_salt("theme").selected_text(current).show_ui(ui, |ui| {
                    for (t, name) in THEMES {
                        ui.selectable_value(&mut self.saved.theme, t, name);
                    }
                });
                if before != self.saved.theme {
                    theme::apply(ui.ctx(), self.saved.theme);
                    self.save();
                }
                ui.label(RichText::new("theme").color(theme::p().overlay));
                // The empty middle of the bar is the window's title bar.
                chrome::drag_region(ui);
            });
        });
        if let Some(a) = select {
            self.saved.active = a;
            self.save();
        }
        if let Some(i) = close {
            self.close_project(i);
        }
    }

    fn project_toolbar(&mut self, ui: &mut egui::Ui) {
        let Active::Project(i) = self.saved.active else { return };
        let Some(p) = self.saved.projects.get_mut(i) else { return };
        let path = p.path.clone();
        let mut changed = false;
        ui.horizontal(|ui| {
            changed |= ui.selectable_value(&mut p.view, View::Code, "Code").changed();
            changed |= ui.selectable_value(&mut p.view, View::Git, "Git").changed();
            ui.add_space(8.0);
            ui.label(RichText::new(path.display().to_string()).weak().small());

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Terminal").clicked() {
                    open_project(&mut self.notice, Opener::Terminal, &path);
                }
                if ui.button("Explorer").clicked() {
                    open_project(&mut self.notice, Opener::Explorer, &path);
                }
                if ui.button("VS Code").clicked() {
                    open_project(&mut self.notice, Opener::Code, &path);
                }
                if let Some(s) = self.code.get(&path) {
                    if s.total_cost > 0.0 {
                        ui.label(RichText::new(format!("${:.3}", s.total_cost)).weak());
                    }
                }
            });
        });
        if changed {
            self.save();
        }
    }

    /// Claude's oldest unanswered prompt in the active session, shown in the
    /// composer's place like the VS Code extension. Returns whether one is up.
    fn permission_prompt(&mut self, ui: &mut egui::Ui) -> bool {
        let session = match self.saved.active {
            Active::Chat => Some(&mut self.chat),
            Active::Project(i) => self.saved.projects.get(i).and_then(|p| self.code.get_mut(&p.path)),
        };
        let Some(session) = session else { return false };
        let mut pending = session.items.iter().enumerate().filter_map(|(i, it)| match it {
            Item::Permission { request_id, tool, input, suggestions, description, state: PermState::Pending } => {
                Some((i, request_id, tool, input, suggestions, description))
            }
            _ => None,
        });
        let Some((i, id, tool, input, suggestions, description)) = pending.next() else { return false };
        let req = prompt::Request { id, tool, input, suggestions, description, kind: session.kind, queued: pending.count() };
        if let Some(answer) = prompt::show(ui, &mut self.md, &req) {
            let rules = session.answer_permission(i, answer);
            let cwd = session.cwd.clone();
            self.save_rules(&cwd, rules);
        }
        true
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
            self.settings = None;
        }
    }

    fn composer(&mut self, ui: &mut egui::Ui) {
        if self.permission_prompt(ui) {
            return;
        }
        let ctx = ui.ctx().clone();
        let busy = self.session_mut().is_some_and(|s| s.busy);

        if !self.attachments.is_empty() {
            let mut remove = None;
            ui.horizontal(|ui| {
                for (i, a) in self.attachments.iter().enumerate() {
                    let r = ui.add(egui::Image::new(&a.texture).max_height(64.0).corner_radius(4.0).sense(egui::Sense::click()));
                    if r.on_hover_text("Click to remove").clicked() {
                        remove = Some(i);
                    }
                }
            });
            if let Some(i) = remove {
                self.attachments.remove(i);
            }
        }

        let hint = match self.saved.active {
            Active::Chat => "Message Claude…  (Enter to send, Shift+Enter for newline, Ctrl+V to paste images)",
            Active::Project(_) => "Ask Claude Code to do something in this project…  (Ctrl+V or drop images)",
        };
        let edit = egui::TextEdit::multiline(&mut self.input)
            .id(egui::Id::unique(COMPOSER_ID))
            .hint_text(hint)
            .desired_rows(3)
            .desired_width(f32::INFINITY)
            .return_key(egui::KeyboardShortcut::new(egui::Modifiers::SHIFT, egui::Key::Enter))
            .lock_focus(true);
        let resp = ui.add(edit);

        let mut send = resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift);
        ui.horizontal(|ui| {
            if busy {
                if ui.button("Stop").clicked() {
                    if let Some(s) = self.session_mut() {
                        s.interrupt();
                    }
                }
                ui.spinner();
            } else if ui.button("Send").clicked() {
                send = true;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                self.mode_picker(ui);
                if ui.button("Attach image").clicked() {
                    if let Some(files) = rfd::FileDialog::new().add_filter("Images", &["png", "jpg", "jpeg", "gif", "webp"]).pick_files() {
                        for f in files {
                            self.attach_file(&ctx, &f);
                        }
                    }
                }
            });
        });

        if send && !busy && (!self.input.trim().is_empty() || !self.attachments.is_empty()) {
            let text = std::mem::take(&mut self.input).trim().to_string();
            let atts = std::mem::take(&mut self.attachments);
            let model = self.saved.model.clone();
            if let Some(s) = self.session_mut() {
                if !s.is_running() {
                    s.model = model;
                }
                s.send(&ctx, text, atts);
            }
        }
    }

    /// The live permission mode of the active project session, like Shift+Tab in
    /// the CLI. Applies right away; the project's default lives in Claude settings.
    fn mode_picker(&mut self, ui: &mut egui::Ui) {
        let Some(s) = self.session_mut().filter(|s| s.kind == Kind::Code) else { return };
        let mut mode = s.mode;
        egui::ComboBox::from_id_salt("permission-mode")
            .selected_text(mode.label())
            .show_ui(ui, |ui| {
                for m in Mode::ALL {
                    ui.selectable_value(&mut mode, m, m.label()).on_hover_text(m.help());
                }
            })
            .response
            .on_hover_text(format!("{}  (Shift+Tab to cycle)", mode.help()));
        if ui.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, egui::Key::Tab)) {
            let next = Mode::ALL.iter().position(|m| *m == mode).map_or(0, |i| (i + 1) % Mode::ALL.len());
            mode = Mode::ALL[next];
        }
        if mode != s.mode {
            s.set_mode(mode);
        }
    }

    fn handle_input_events(&mut self, ctx: &egui::Context) {
        let (pasted, dropped, open, cycle, jump) = ctx.input(|i| {
            let pasted = i.events.iter().any(|e| {
                matches!(e, egui::Event::Paste(_))
                    || matches!(e, egui::Event::Key { key: egui::Key::V, pressed: true, modifiers, .. } if modifiers.command)
            });
            let dropped: Vec<PathBuf> = i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect();
            let open = i.modifiers.command && i.key_pressed(egui::Key::O);
            let cycle = (i.modifiers.ctrl && i.key_pressed(egui::Key::Tab)).then_some(i.modifiers.shift);
            const DIGITS: [egui::Key; 9] = [
                egui::Key::Num1, egui::Key::Num2, egui::Key::Num3, egui::Key::Num4, egui::Key::Num5,
                egui::Key::Num6, egui::Key::Num7, egui::Key::Num8, egui::Key::Num9,
            ];
            let jump = if i.modifiers.command { DIGITS.iter().position(|k| i.key_pressed(*k)) } else { None };
            (pasted, dropped, open, cycle, jump)
        });

        if open {
            self.pick_project();
        }
        // Tab order: Chat, then each project. Ctrl+Tab / Ctrl+Shift+Tab cycle, Ctrl+1 is Chat.
        let count = self.saved.projects.len() + 1;
        let pos = match self.saved.active {
            Active::Chat => 0,
            Active::Project(i) => i + 1,
        };
        let target = match (cycle, jump) {
            (Some(back), _) => Some(if back { (pos + count - 1) % count } else { (pos + 1) % count }),
            (_, Some(n)) if n < count => Some(n),
            _ => None,
        };
        if let Some(t) = target {
            self.saved.active = if t == 0 { Active::Chat } else { Active::Project(t - 1) };
            self.save();
        }

        let in_code = self.active_project().is_none_or(|p| p.view == View::Code);
        if pasted && in_code {
            if let Ok(img) = arboard::Clipboard::new().and_then(|mut c| c.get_image()) {
                if let Some(rgba) = image::RgbaImage::from_raw(img.width as u32, img.height as u32, img.bytes.into_owned()) {
                    self.attach_image(ctx, image::DynamicImage::ImageRgba8(rgba));
                }
            }
        }

        for path in dropped {
            if path.is_dir() {
                self.add_project(path);
            } else if is_image(&path) {
                self.attach_file(ctx, &path);
            } else {
                if !self.input.is_empty() && !self.input.ends_with(' ') {
                    self.input.push(' ');
                }
                self.input.push_str(&path.display().to_string());
            }
        }
    }

    fn attach_file(&mut self, ctx: &egui::Context, path: &Path) {
        match image::open(path) {
            Ok(img) => self.attach_image(ctx, img),
            Err(e) => self.notice = Some(format!("Couldn't read image {}: {e}", path.display())),
        }
    }

    fn attach_image(&mut self, ctx: &egui::Context, img: image::DynamicImage) {
        let img = if img.width().max(img.height()) > MAX_IMAGE_EDGE {
            img.resize(MAX_IMAGE_EDGE, MAX_IMAGE_EDGE, image::imageops::FilterType::Triangle)
        } else {
            img
        };
        let rgba = img.to_rgba8();
        let mut png = Vec::new();
        if let Err(e) = rgba.write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png) {
            self.notice = Some(format!("Couldn't encode image: {e}"));
            return;
        }
        let size = [rgba.width() as usize, rgba.height() as usize];
        let color = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
        let n = self.attachments.len();
        let texture = ctx.load_texture(format!("attachment-{n}-{}", png.len()), color, Default::default());
        self.attachments.push(Attachment {
            media_type: "image/png".into(),
            base64: base64::engine::general_purpose::STANDARD.encode(&png),
            texture,
        });
    }

    fn transcript(&mut self, ui: &mut egui::Ui) {
        let session = match self.saved.active {
            Active::Chat => Some(&mut self.chat),
            Active::Project(i) => self.saved.projects.get(i).and_then(|p| self.code.get_mut(&p.path)),
        };
        let Some(session) = session.filter(|s| !s.items.is_empty()) else {
            let msg = match self.saved.active {
                Active::Chat => "Start a conversation. Uses your Claude Code login, no API key needed.",
                Active::Project(_) => "Ask Claude Code to work on this project.",
            };
            ui.centered_and_justified(|ui| ui.label(RichText::new(msg).weak()));
            return;
        };

        egui::ScrollArea::vertical().stick_to_bottom(true).auto_shrink(false).show(ui, |ui| {
            ui.set_max_width(ui.available_width().min(900.0));
            for (i, item) in session.items.iter().enumerate() {
                ui.add_space(6.0);
                match item {
                    Item::User { text, images } => {
                        egui::Frame::new()
                            .fill(ui.visuals().faint_bg_color)
                            .corner_radius(6.0)
                            .inner_margin(8.0)
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                if !images.is_empty() {
                                    ui.horizontal(|ui| {
                                        for t in images {
                                            ui.add(egui::Image::new(t).max_height(120.0).corner_radius(4.0));
                                        }
                                    });
                                }
                                if !text.is_empty() {
                                    ui.label(text);
                                }
                            });
                    }
                    Item::Assistant(text) => {
                        CommonMarkViewer::new().show(ui, &mut self.md, text);
                    }
                    Item::Thinking => {
                        ui.label(RichText::new("thinking…").italics().weak());
                    }
                    Item::ToolUse { name, summary, input } => {
                        egui::CollapsingHeader::new(RichText::new(format!("⚙ {name}  {summary}")).monospace())
                            .id_salt(("tool", i))
                            .show(ui, |ui| {
                                ui.label(RichText::new(input).monospace().small());
                            });
                    }
                    Item::ToolResult { content, is_error } => {
                        let first = content.lines().next().unwrap_or("").chars().take(140).collect::<String>();
                        let color = if *is_error { theme::p().red } else { ui.visuals().weak_text_color() };
                        egui::CollapsingHeader::new(RichText::new(format!("  {first}")).monospace().small().color(color))
                            .id_salt(("result", i))
                            .show(ui, |ui| {
                                egui::ScrollArea::vertical().max_height(300.0).id_salt(("result-scroll", i)).show(ui, |ui| {
                                    ui.label(RichText::new(content).monospace().small());
                                });
                            });
                    }
                    // Answered in the prompt that takes the composer's place; the
                    // transcript keeps a one-line record.
                    Item::Permission { tool, input, state, .. } => {
                        let p = theme::p();
                        let (mark, note, color) = match state {
                            PermState::Pending => ("●", "waiting for you below", p.peach),
                            PermState::Allowed(n) => ("✔", n.as_str(), p.green),
                            PermState::Denied(n) => ("✖", n.as_str(), p.red),
                        };
                        ui.horizontal_wrapped(|ui| {
                            ui.label(RichText::new(mark).color(color));
                            ui.label(RichText::new(prompt::title(tool, input)).small());
                            ui.label(RichText::new(note).small().color(color));
                        });
                    }
                    Item::Result { cost, duration_ms, is_error } => {
                        let text = format!("— done in {:.1}s · ${cost:.4}{}", *duration_ms as f64 / 1000.0, if *is_error { " · error" } else { "" });
                        ui.label(RichText::new(text).small().weak());
                    }
                    Item::Error(e) => {
                        ui.label(RichText::new(e).color(theme::p().red).small());
                    }
                }
            }
        });
    }

    /// The ⚙ Claude settings modal for a project, open while egui's temp memory
    /// holds its path. Sections down the left.
    fn claude_settings_modal(&mut self, ctx: &egui::Context) {
        let id = egui::Id::unique(SETTINGS_MODAL);
        let Some(path) = ctx.data(|d| d.get_temp::<Option<PathBuf>>(id)).flatten() else { return };
        const SECTIONS: [&str; 2] = ["Permissions", "Model"];
        let section_id = id.with("section");
        let mut section = ctx.data(|d| d.get_temp::<usize>(section_id)).unwrap_or(0).min(SECTIONS.len() - 1);
        let mut close = false;

        let modal = egui::Modal::new(id).show(ctx, |ui| {
            ui.set_width(760.0_f32.min(ctx.content_rect().width() - 80.0));
            ui.set_height(560.0_f32.min(ctx.content_rect().height() - 120.0));
            ui.horizontal(|ui| {
                ui.heading("⚙  Claude settings");
                ui.label(RichText::new(project_name(&path)).weak());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    close |= ui.add(egui::Button::new("✖").frame(false)).on_hover_text("Close (Esc)").clicked();
                });
            });
            ui.separator();
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(150.0);
                    for (i, name) in SECTIONS.iter().enumerate() {
                        if ui.add(egui::Button::selectable(section == i, *name).min_size(egui::vec2(150.0, 0.0))).clicked() {
                            section = i;
                        }
                    }
                });
                ui.separator();
                ui.vertical(|ui| {
                    egui::ScrollArea::vertical().id_salt("claude-settings-scroll").auto_shrink(false).show(ui, |ui| {
                        self.settings_section(ui, &path, SECTIONS[section]);
                        ui.add_space(16.0);
                        ui.label(RichText::new(format!("Stored in {}", project::dir(&path).display())).small().weak());
                    });
                });
            });
        });

        ctx.data_mut(|d| d.insert_temp(section_id, section));
        if close || modal.should_close() {
            ctx.data_mut(|d| d.remove::<Option<PathBuf>>(id));
        }
    }

    /// One section of the Claude settings modal. Edits save immediately: the mode
    /// applies to the live session, everything else from the next process start.
    fn settings_section(&mut self, ui: &mut egui::Ui, path: &Path, section: &str) {
        if self.settings.as_ref().is_none_or(|s| s.path != path) {
            self.settings = Some(SettingsView {
                path: path.to_path_buf(),
                perms: Permissions::load(path),
                claude: ClaudeSettings::load(path),
                new_rule: String::new(),
            });
        }
        let Some(view) = &mut self.settings else { return };
        let (perms_before, claude_before) = (view.perms.clone(), view.claude.clone());

        match section {
            "Permissions" => {
                ui.label(RichText::new("Default mode").strong());
                ui.label(RichText::new("What new sessions start in. Switch a running session from the composer.").weak());
                for m in Mode::ALL {
                    ui.radio_value(&mut view.claude.mode, m, m.label()).on_hover_text(m.help());
                }
                ui.add_space(12.0);

                ui.label(RichText::new("Rules").strong());
                ui.label(
                    RichText::new(
                        "In Claude Code's syntax: Bash(cargo test:*), Edit, WebFetch(domain:docs.rs). \
                         They add to your own Claude settings; a deny anywhere wins.",
                    )
                    .weak(),
                );
                ui.add_space(4.0);
                rules_editor(ui, &mut view.perms, &mut view.new_rule);
                ui.add_space(12.0);

                ui.label(RichText::new("Additional directories").strong());
                ui.label(RichText::new("Folders outside the project Claude may read and edit (--add-dir).").weak());
                let mut remove = None;
                for (i, dir) in view.claude.add_dirs.iter().enumerate() {
                    ui.horizontal(|ui| {
                        if ui.small_button("✖").on_hover_text("Remove").clicked() {
                            remove = Some(i);
                        }
                        ui.label(RichText::new(dir.display().to_string()).monospace());
                    });
                }
                if let Some(i) = remove {
                    view.claude.add_dirs.remove(i);
                }
                if ui.button("Add folder…").clicked() {
                    if let Some(dir) = rfd::FileDialog::new().set_directory(path).pick_folder() {
                        if !view.claude.add_dirs.contains(&dir) {
                            view.claude.add_dirs.push(dir);
                        }
                    }
                }
            }
            _ => {
                ui.label(RichText::new("Effort").strong());
                ui.label(RichText::new("How hard Claude thinks (--effort). Default uses your Claude settings.").weak());
                let current = project::EFFORTS.iter().find(|e| e.0 == view.claude.effort).map_or("Default", |e| e.1);
                egui::ComboBox::from_id_salt("effort").selected_text(current).show_ui(ui, |ui| {
                    for (id, name) in project::EFFORTS {
                        ui.selectable_value(&mut view.claude.effort, id.to_string(), name);
                    }
                });
                ui.add_space(12.0);
                ui.label(RichText::new("The model is picked in the top bar and applies to every tab.").weak());
            }
        }

        let path = path.to_path_buf();
        if view.perms != perms_before {
            if let Err(e) = view.perms.save(&path) {
                self.notice = Some(e);
            }
        }
        if view.claude != claude_before {
            if let Err(e) = view.claude.save(&path) {
                self.notice = Some(e);
            }
            if view.claude.mode != claude_before.mode {
                if let Some(s) = self.code.get_mut(&path) {
                    s.set_mode(view.claude.mode);
                }
            }
        }
        let claude_restart = view.claude.effort != claude_before.effort || view.claude.add_dirs != claude_before.add_dirs;
        if view.perms != perms_before || claude_restart {
            // The CLI reads these at startup: restart the project's process so it picks them up.
            // `--resume` keeps the conversation going; rules allowed "this session" are dropped.
            self.restart_when_idle.insert(path);
        }
    }
}

/// Allow and deny lists with remove buttons, and a field to add a rule.
fn rules_editor(ui: &mut egui::Ui, perms: &mut Permissions, new_rule: &mut String) {
    for (title, rules) in [("Allow", &mut perms.allow), ("Deny", &mut perms.deny)] {
        ui.label(title);
        if rules.is_empty() {
            ui.label(RichText::new("  none").weak());
        }
        let mut remove = None;
        for (i, rule) in rules.iter().enumerate() {
            ui.horizontal(|ui| {
                if ui.small_button("✖").on_hover_text("Remove this rule").clicked() {
                    remove = Some(i);
                }
                ui.add(egui::Label::new(RichText::new(rule).monospace()).truncate()).on_hover_text(rule);
            });
        }
        if let Some(i) = remove {
            rules.remove(i);
        }
        ui.add_space(4.0);
    }

    let rule = new_rule.trim().to_string();
    let (enter, allow, deny) = ui
        .horizontal(|ui| {
            let field = ui.add(egui::TextEdit::singleline(new_rule).hint_text("Bash(npm test:*)").desired_width(320.0));
            let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let allow = ui.add_enabled(!rule.is_empty(), egui::Button::new("Allow")).clicked();
            let deny = ui.add_enabled(!rule.is_empty(), egui::Button::new("Deny")).clicked();
            (enter, allow, deny)
        })
        .inner;
    let list = if deny { Some(&mut perms.deny) } else if allow || enter { Some(&mut perms.allow) } else { None };
    if let Some(list) = list.filter(|_| !rule.is_empty()) {
        if !list.contains(&rule) {
            list.push(rule);
        }
        new_rule.clear();
    }
}

impl eframe::App for KynexApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        chrome::CLEAR
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // With `--features hotpatch`, `dx serve --hot-patch` swaps in edited code for `render` live.
        #[cfg(feature = "hotpatch")]
        subsecond::call(|| self.render(ui));
        #[cfg(not(feature = "hotpatch"))]
        self.render(ui);
    }
}

impl KynexApp {
    fn render(&mut self, ui: &mut egui::Ui) {
        let ctx = &ui.ctx().clone();
        self.chat.poll();
        for s in self.code.values_mut() {
            s.poll();
        }
        self.restart_when_idle.retain(|path| match self.code.get_mut(path) {
            Some(s) if s.busy => true,
            Some(s) => {
                s.kill();
                false
            }
            None => false,
        });
        self.sync_session_ids();
        self.handle_input_events(ctx);

        // Like Hyprland, one tile always has focus: the composer while typing, otherwise the main view.
        let composer_focused = ctx.memory(|m| m.has_focus(egui::Id::unique(COMPOSER_ID)));
        if let Active::Project(i) = self.saved.active {
            if let Some(p) = self.saved.projects.get_mut(i).filter(|p| p.view == View::Settings) {
                p.view = View::Code;
            }
        }
        let view = self.active_project().map(|p| p.view);

        // Switching workspace fades the content in, like Hyprland's workspace animation.
        if self.last_active != self.saved.active {
            self.last_active = self.saved.active;
            self.switched_at = ctx.input(|i| i.time);
        }
        let fade = ((ctx.input(|i| i.time) - self.switched_at) / 0.12).clamp(0.0, 1.0) as f32;
        if fade < 1.0 {
            ctx.request_repaint();
        }

        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::B)) {
            self.saved.sessions_collapsed = !self.saved.sessions_collapsed;
            self.save();
        }
        let sessions_collapsed = self.saved.sessions_collapsed;

        chrome::background(ui);
        let gap = theme::p().gap;
        let gaps = egui::Frame::new().inner_margin(gap);
        egui::CentralPanel::default().frame(gaps).show(ui, |ui| {
            egui::Panel::top("bar").frame(egui::Frame::NONE).show(ui, |ui| {
                theme::tile(ui, "bar", "", false, false, |ui| self.tab_bar(ui));
                ui.add_space(gap);
            });
            ui.multiply_opacity(1.0 - (1.0 - fade).powi(3));

            if let Some(n) = self.notice.clone() {
                egui::Panel::bottom("notice").frame(egui::Frame::NONE).show(ui, |ui| {
                    ui.add_space(gap);
                    theme::tile(ui, "notice", "", false, false, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(n).color(theme::p().yellow));
                            if ui.small_button("x").clicked() {
                                self.notice = None;
                            }
                        });
                    });
                });
            }

            if view == Some(View::Git) {
                let path = self.active_project().map(|p| p.path.clone()).unwrap();
                theme::tile(ui, "main", "", true, true, |ui| {
                    self.project_toolbar(ui);
                    ui.separator();
                    self.git.entry(path.clone()).or_insert_with(|| GitView::new(path)).ui(ui);
                });
                return;
            }
            // Create (or resume) the project's session as soon as its tab is shown,
            // so a restored transcript is visible before the first message.
            let _ = self.session_mut();

            let side = egui::Frame::NONE.inner_margin(egui::Margin { right: gap as i8, ..Default::default() });
            if self.saved.sessions_collapsed {
                egui::Panel::left("sessions-collapsed").frame(side).resizable(false).show(ui, |ui| {
                    theme::tile(ui, "sessions-collapsed", "", false, true, |ui| {
                        if ui.add(egui::Button::new("»").frame(false)).on_hover_text("Show sessions (Ctrl+B)").clicked() {
                            self.saved.sessions_collapsed = false;
                        }
                    });
                });
            } else {
                egui::Panel::left("sessions")
                    .frame(side)
                    .resizable(true)
                    .default_size(260.0)
                    .size_range(160.0..=480.0)
                    .show(ui, |ui| {
                        theme::tile(ui, "sessions", "", false, true, |ui| self.sessions_panel(ui));
                    });
            }
            if self.saved.sessions_collapsed != sessions_collapsed {
                self.save();
            }
            egui::Panel::bottom("composer").frame(egui::Frame::NONE).show(ui, |ui| {
                ui.add_space(gap);
                theme::tile(ui, "composer", "", composer_focused, false, |ui| self.composer(ui));
            });
            theme::tile(ui, "main", "", !composer_focused, true, |ui| {
                if view.is_some() {
                    self.project_toolbar(ui);
                    ui.separator();
                }
                self.transcript(ui);
            });
        });
        self.claude_settings_modal(ctx);
        chrome::resize_edges(ui);
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() > max { format!("{}…", s.chars().take(max).collect::<String>()) } else { s.to_string() }
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
    let Some(w) = wsl::parse(path) else {
        match opener {
            Opener::Code => open_with(notice, "code", &[path.as_os_str()]),
            Opener::Explorer => open_with(notice, "explorer", &[path.as_os_str()]),
            Opener::Terminal => open_with(notice, "wt", &["-d".as_ref(), path.as_os_str()]),
        }
        return;
    };
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
