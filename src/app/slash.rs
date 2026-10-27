//! Slash commands in the composer: a popup of the session's commands while a
//! `/name` is typed (Up / Down to move, Tab or Enter to take one, Esc to close),
//! and the built-ins Kynex runs itself instead of sending.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use gpui::{Anchor, AnyElement, Bounds, Context, Focusable, Pixels, Window, point, prelude::*, px};
use kynex_core::commands::{self, Action, SlashCommand};
use kynex_core::project;
use kynex_core::session::{Kind, Session};

use super::menus::Menu;
use super::{KynexApp, models};
use crate::ui::completions::{Completion, completions};

/// How long a folder's command list is trusted before it's asked for again,
/// so newly added skills and commands show up.
const FRESH: Duration = Duration::from_secs(60);

/// The popup's state, and each folder's commands.
#[derive(Default)]
pub(super) struct Slash {
    commands: HashMap<PathBuf, Fetched>,
    selected: usize,
    /// What the selection belongs to; a new query starts at the top again.
    query: String,
    /// The composer's text when Esc closed the popup; it stays closed until that changes.
    pub(super) dismissed: Option<String>,
    /// The composer's bounds as last drawn, for menus a command opens.
    pub(super) composer: Option<Bounds<Pixels>>,
}

struct Fetched {
    list: Vec<SlashCommand>,
    at: Instant,
    loading: bool,
}

impl KynexApp {
    /// What's typed after the slash, while the composer holds just `/name`.
    fn slash_query(&self, cx: &Context<Self>) -> Option<String> {
        let text = self.composer.read(cx).text();
        let query = text.strip_prefix('/')?;
        let open = self.prompt.is_none()
            && !query.contains(char::is_whitespace)
            && self.slash.dismissed.as_deref() != Some(text)
            && self.active_session().is_some();
        open.then(|| query.to_string())
    }

    /// The commands matching what's typed, or `None` when the popup is closed.
    fn slash_matches(&mut self, cx: &mut Context<Self>) -> Option<Vec<SlashCommand>> {
        let query = self.slash_query(cx)?;
        let cwd = self.active_session()?.cwd.clone();
        self.fetch_commands(cwd.clone(), cx);
        let list = self.slash.commands.get(&cwd).map_or(&[][..], |f| &f.list[..]);
        Some(commands::matching(list, &query).into_iter().cloned().collect())
    }

    /// Asks the CLI for a folder's commands, the first time and once they're stale.
    fn fetch_commands(&mut self, cwd: PathBuf, cx: &mut Context<Self>) {
        if self.slash.commands.get(&cwd).is_some_and(|f| f.loading || f.at.elapsed() < FRESH) {
            return;
        }
        let entry = self.slash.commands.entry(cwd.clone()).or_insert(Fetched {
            list: Vec::new(),
            at: Instant::now(),
            loading: true,
        });
        entry.loading = true;
        cx.spawn(async move |this, cx| {
            let dir = cwd.clone();
            let result = cx.background_spawn(async move { commands::list(&dir) }).await;
            this.update(cx, |this, cx| {
                if let Some(f) = this.slash.commands.get_mut(&cwd) {
                    match result {
                        Ok(list) => f.list = list,
                        Err(e) => eprintln!("kynex: couldn't list slash commands: {e}"),
                    }
                    f.at = Instant::now();
                    f.loading = false;
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The popup above the composer, while a `/name` is being typed.
    pub(super) fn render_slash(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let matches = self.slash_matches(cx)?;
        let query = self.slash_query(cx).unwrap_or_default();
        if self.slash.query != query {
            self.slash.query = query;
            self.slash.selected = 0;
        }
        self.slash.selected = self.slash.selected.min(matches.len().saturating_sub(1));
        let loading = self.active_session().and_then(|s| self.slash.commands.get(&s.cwd)).is_some_and(|f| f.loading);
        if matches.is_empty() && !loading {
            return None;
        }
        let items: Vec<Completion> = matches
            .iter()
            .map(|c| Completion {
                label: format!("/{}", c.name).into(),
                hint: c.argument_hint.clone().into(),
                detail: c.description.lines().next().unwrap_or("").to_string().into(),
            })
            .collect();
        let this = cx.entity().downgrade();
        let list = completions(&items, self.slash.selected, "Loading commands…", cx, move |i, window, cx| {
            this.update(cx, |this, cx| this.take_slash(i, false, window, cx)).ok();
        });
        Some(list.w_full().max_w(px(560.)).into_any_element())
    }

    /// Moves the selection; false if the popup is closed.
    pub(super) fn slash_move(&mut self, down: bool, cx: &mut Context<Self>) -> bool {
        let Some(n) = self.slash_matches(cx).map(|m| m.len()).filter(|&n| n > 0) else { return false };
        self.slash.selected = if down { (self.slash.selected + 1) % n } else { (self.slash.selected + n - 1) % n };
        cx.notify();
        true
    }

    /// Takes the selected command (Tab, or Enter with `run`); false if the popup is closed.
    pub(super) fn slash_accept(&mut self, run: bool, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.slash_matches(cx).is_none_or(|m| m.is_empty()) {
            return false;
        }
        self.take_slash(self.slash.selected, run, window, cx);
        true
    }

    /// Esc: closes the popup; false if it wasn't open.
    pub(super) fn slash_dismiss(&mut self, cx: &mut Context<Self>) -> bool {
        if self.slash_matches(cx).is_none() {
            return false;
        }
        self.slash.dismissed = Some(self.composer.read(cx).text().to_string());
        cx.notify();
        true
    }

    /// Puts command `i` in the composer: ready for its arguments if it takes
    /// some, else sent right away with `run`.
    fn take_slash(&mut self, i: usize, run: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(c) = self.slash_matches(cx).and_then(|m| m.into_iter().nth(i)) else { return };
        let text = if c.argument_hint.is_empty() { format!("/{}", c.name) } else { format!("/{} ", c.name) };
        self.composer.update(cx, |input, cx| input.set_text(text.clone(), cx));
        // Typing a space closes it anyway; a complete `/name` would keep it open.
        self.slash.dismissed = Some(text);
        self.composer.read(cx).focus_handle(cx).focus(window, cx);
        if run && c.argument_hint.is_empty() {
            self.send(window, cx);
        }
        cx.notify();
    }

    /// Runs a built-in Kynex handles itself. Returns false for a message that
    /// isn't one, which is sent to Claude as usual.
    pub(super) fn run_command(&mut self, text: &str, cx: &mut Context<Self>) -> bool {
        let Some((name, args)) = commands::parse(text) else { return false };
        match commands::action(name, args) {
            Action::Send => return false,
            Action::NewSession => {
                if let Some(s) = self.session_mut().filter(|s| s.session_id.is_some()) {
                    let (kind, cwd) = (s.kind, s.cwd.clone());
                    self.open_session(Session::new(kind, cwd));
                }
            }
            Action::Model(m) if m.is_empty() => self.open_menu_at_composer(Menu::Model, cx),
            Action::Model(m) => {
                let found = models::models()
                    .into_iter()
                    .find(|i| i.value.eq_ignore_ascii_case(&m) || i.name.eq_ignore_ascii_case(&m));
                let value = found.map_or(m, |i| i.value);
                self.saved.model = if value == "default" { String::new() } else { value };
                self.save();
            }
            Action::Effort(e) => {
                let project = self.active_session().filter(|s| s.kind == Kind::Code).map(|s| s.cwd.clone());
                match project {
                    None => self.notice = Some("Effort is set per project; chat has none.".into()),
                    Some(_) if e.is_empty() => self.open_menu_at_composer(Menu::Effort, cx),
                    Some(path) => {
                        match project::EFFORTS.iter().find(|(id, _)| !id.is_empty() && id.eq_ignore_ascii_case(&e)) {
                            Some((id, _)) => self.set_effort(path, id.to_string(), cx),
                            None => {
                                let known: Vec<&str> =
                                    project::EFFORTS.iter().map(|e| e.0).filter(|e| !e.is_empty()).collect();
                                self.notice = Some(format!("Unknown effort \"{e}\"; try {}.", known.join(", ")));
                            }
                        }
                    }
                }
            }
            Action::Sessions => {
                self.saved.sessions_collapsed = false;
                self.save();
            }
            Action::TerminalOnly => {
                self.notice = Some(format!("/{name} only works in the terminal CLI; Kynex has nothing to run it with."))
            }
        }
        cx.notify();
        true
    }

    /// Opens a composer menu without a click to anchor it: above the composer's left edge.
    fn open_menu_at_composer(&mut self, which: Menu, cx: &mut Context<Self>) {
        let at = self.slash.composer.map_or(point(px(200.), px(400.)), |b| point(b.left() + px(12.), b.top() - px(4.)));
        self.open_menu(which, at, Anchor::BottomLeft, cx);
    }
}
