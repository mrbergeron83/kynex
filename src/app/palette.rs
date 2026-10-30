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
use kynex_core::fmt::ago;
use kynex_core::palette::{Entry, Kind, Match, Palette};
use kynex_core::paths::project_name;
use kynex_core::session::{self, SessionInfo};

use super::KynexApp;
use super::keys;
use super::state::Active;
use crate::theme::{THEMES, Theme, mono, theme};
use crate::ui::input::{self, TextInput};
use crate::ui::motion::{self, fade, pop};
use crate::ui::wash::wash;
use crate::ui::{Icon, icon};

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
    /// Each project's past sessions, as they come in.
    sessions: Rc<HashMap<PathBuf, Vec<SessionInfo>>>,
    /// What `found` is for, and the selection in it.
    query: String,
    found: Rc<Vec<Match>>,
    selected: usize,
    /// The entries changed (sessions came in): match again, keeping the selection.
    stale: bool,
}

/// What a result is besides its title: where it goes and what's shown with it.
struct Row {
    target: Target,
    detail: SharedString,
    /// A shortcut, or a session's state ("working", "needs you").
    hint: SharedString,
}

#[derive(Clone)]
enum Target {
    Session { path: PathBuf, id: String },
    Tab(Active),
    Command(Cmd),
}

/// What an action in the palette does.
#[derive(Clone, Copy, PartialEq)]
enum Cmd {
    NewSession,
    OpenProject,
    CloseTab,
    NextTab,
    PrevTab,
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
    About,
    CheckForUpdates,
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
        (Cmd::Settings(1), "Settings: Appearance", "theme font motion toasts footer"),
        (Cmd::Settings(2), "Settings: Keyboard", "shortcuts keymap bindings"),
        (Cmd::ProjectSettings, "Project settings", "rules permissions effort model directories"),
        (Cmd::Extensions, "Extensions", "mcp servers plugins"),
        (Cmd::About, "About Kynex", "version"),
        (Cmd::CheckForUpdates, "Check for updates", "kynex claude cli version"),
    ]
    .into_iter()
    .map(|(c, label, words)| (c, label.to_string(), words))
    .collect();
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
        let input = cx.new(|cx| TextInput::new(cx).placeholder("Search sessions, projects and actions…"));
        input.read(cx).focus_handle(cx).focus(window, cx);
        let view = PaletteView {
            input,
            rows: Rc::default(),
            palette: Rc::default(),
            sessions: Rc::default(),
            query: String::new(),
            found: Rc::default(),
            selected: 0,
            stale: true,
        };
        self.palette = Some(view);
        self.rebuild_palette(cx);
        for p in &self.saved.projects {
            let path = p.path.clone();
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

    /// Lists every entry again: after sessions come in.
    fn rebuild_palette(&mut self, cx: &Context<Self>) {
        let Some(view) = &self.palette else { return };
        let mut rows = Vec::new();
        let mut entries = Vec::new();
        for p in &self.saved.projects {
            let name = project_name(&p.path);
            let live = self.code.get(&p.path);
            for s in view.sessions.get(&p.path).into_iter().flatten() {
                let open = live.and_then(|l| l.sessions.iter().find(|o| o.session_id.as_deref() == Some(&s.id)));
                let hint = match open {
                    Some(o) if o.wants_you() => "needs you",
                    Some(o) if o.busy => "working",
                    _ => "",
                };
                rows.push(Row {
                    target: Target::Session { path: p.path.clone(), id: s.id.clone() },
                    detail: format!("{name} · {}", ago(s.modified)).into(),
                    hint: hint.into(),
                });
                entries.push(Entry {
                    kind: Kind::Session,
                    title: s.title.clone(),
                    keywords: name.clone(),
                    when: Some(s.modified),
                });
            }
        }
        let tabs = std::iter::once((Active::Chat, "Chat".to_string(), String::new())).chain(
            self.saved
                .projects
                .iter()
                .enumerate()
                .map(|(i, p)| (Active::Project(i), project_name(&p.path), p.path.display().to_string())),
        );
        for (tab, name, path) in tabs {
            rows.push(Row { target: Target::Tab(tab), detail: path.clone().into(), hint: "".into() });
            entries.push(Entry { kind: Kind::Project, title: name, keywords: path, when: None });
        }
        for (cmd, label, words) in commands() {
            let hint = cmd.action().and_then(|a| crate::ui::shortcut(a.as_ref(), cx)).unwrap_or_default();
            rows.push(Row { target: Target::Command(cmd), detail: "".into(), hint: hint.into() });
            entries.push(Entry { kind: Kind::Action, title: label, keywords: words.into(), when: None });
        }
        let view = self.palette.as_mut().unwrap();
        view.rows = Rc::new(rows);
        view.palette = Rc::new(Palette::new(entries));
        view.stale = true;
    }

    /// The results for what's typed, matched once per change.
    fn palette_found(&mut self, cx: &Context<Self>) -> Option<Rc<Vec<Match>>> {
        let view = self.palette.as_mut()?;
        let query = view.input.read(cx).text().to_string();
        if view.stale || view.query != query {
            let recent: Vec<&str> = self.palette_recent.iter().map(String::as_str).collect();
            view.found = Rc::new(view.palette.matching(&query, &recent, LISTED));
            if view.query != query {
                view.selected = 0;
            }
            view.query = query;
            view.stale = false;
            view.selected = view.selected.min(view.found.len().saturating_sub(1));
        }
        Some(view.found.clone())
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
            Target::Session { path, id } => {
                if let Some(i) = self.saved.projects.iter().position(|p| p.path == path) {
                    self.select(Active::Project(i), window, cx);
                    self.show_code_view();
                    self.switch_session(session::Kind::Code, path, id);
                }
                self.focus_input(window, cx);
            }
            Target::Tab(tab) => {
                self.select(tab, window, cx);
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
        // Keys that only work in the message box (Ctrl+R, Ctrl+S…) find it focused.
        self.focus_input(window, cx);
        if let Some(action) = cmd.action() {
            return window.dispatch_action(action, cx);
        }
        match cmd {
            Cmd::Compact => self.compact(cx),
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
            Cmd::About => self.open_about(cx),
            Cmd::CheckForUpdates => {
                self.check_for_update(cx);
                self.check_cli_now(cx);
            }
            Cmd::Theme(t) => self.set_theme(t, cx),
            _ => {}
        }
    }

    pub(super) fn render_palette(
        &mut self,
        leaving: Option<f32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let found = self.palette_found(cx)?;
        let view = self.palette.clone()?;
        let t = theme(cx).clone();
        let vp = window.viewport_size();
        let bold = HighlightStyle { font_weight: Some(FontWeight::BOLD), color: Some(t.text), ..Default::default() };
        let start = (view.selected + 1).saturating_sub(ROWS);
        let rows = found.iter().enumerate().skip(start).take(ROWS).map(|(i, m)| {
            let row = &view.rows[m.index];
            let entry = &view.palette.entries[m.index];
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
                _ => t.text_placeholder,
            };
            let mono_hint = matches!(row.target, Target::Command(_));
            div()
                .id(("palette-row", i))
                .relative()
                .flex()
                .items_center()
                .gap_2()
                .h(px(30.))
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
                            .text_xs()
                            .text_color(hint_colour)
                            .when(mono_hint, |d| d.font_family(mono()))
                            .child(row.hint.clone()),
                    )
                })
                .on_click(cx.listener(move |this, _, window, cx| this.take_palette(i, window, cx)))
        });
        let more = found.len().saturating_sub(start + ROWS);
        let empty = found.is_empty();

        let card = div()
            .id("palette")
            .occlude()
            .flex()
            .flex_col()
            .w_full()
            .max_w(px(640.))
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
                    .h(px(44.))
                    .px_3()
                    .border_b_1()
                    .border_color(t.border_variant)
                    .child(icon(Icon::Search, t.text_muted).flex_none())
                    .child(div().flex_1().min_w_0().child(view.input.clone())),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .p_1()
                    .children(rows)
                    .when(empty, |d| {
                        d.child(div().px_2().py_2().text_sm().text_color(t.text_muted).child("Nothing matches"))
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
                .pt(vp.height * 0.12)
                .bg(gpui::black().opacity(0.2))
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
