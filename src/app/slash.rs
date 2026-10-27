//! Slash commands in the composer: a popup of the session's commands while a
//! `/name` is typed (Up / Down to move, Tab or Enter to take one, Esc to close),
//! and the built-ins Kynex runs itself instead of sending.
//!
//! A folder's commands come from its sessions: each reports them as its process
//! starts, and the last list is saved with the project. So the popup shows the
//! saved list at once, and a fresh one replaces it when a session starts. Only a
//! folder never seen before asks the CLI with a process of its own.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui::{Anchor, AnyElement, Bounds, Context, Focusable, Pixels, Window, point, prelude::*, px};
use kynex_core::commands::{self, Action, Commands, SlashCommand};
use kynex_core::project;
use kynex_core::session::{Kind, Session};

use super::menus::Menu;
use super::{KynexApp, models};
use crate::ui::completions::{Completion, completions};

/// The popup's state, and each folder's commands.
#[derive(Default)]
pub(super) struct Slash {
    commands: HashMap<PathBuf, Known>,
    /// The last matches, so the popup filters once per keystroke rather than on
    /// every render and key handler.
    matches: Option<Matches>,
    selected: usize,
    /// What the selection belongs to; a new query starts at the top again.
    query: String,
    /// The composer's text when Esc closed the popup; it stays closed until that changes.
    pub(super) dismissed: Option<String>,
    /// The composer's bounds as last drawn, for menus a command opens.
    pub(super) composer: Option<Bounds<Pixels>>,
}

/// A folder's commands as far as Kynex knows them.
struct Known {
    commands: Rc<Commands>,
    /// The saved list (or the CLI's) is on its way.
    loading: bool,
    /// From a session that started this run, so a saved list mustn't replace it.
    fresh: bool,
}

struct Matches {
    cwd: PathBuf,
    commands: Rc<Commands>,
    query: String,
    found: Rc<Vec<SlashCommand>>,
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
    fn slash_matches(&mut self, cx: &mut Context<Self>) -> Option<Rc<Vec<SlashCommand>>> {
        let query = self.slash_query(cx)?;
        let cwd = self.active_session()?.cwd.clone();
        self.load_commands(&cwd, cx);
        let commands = self.slash.commands.get(&cwd).map(|k| k.commands.clone()).unwrap_or_default();
        if let Some(m) = &self.slash.matches
            && m.cwd == cwd
            && m.query == query
            && Rc::ptr_eq(&m.commands, &commands)
        {
            return Some(m.found.clone());
        }
        let found = Rc::new(commands.matching(&query).into_iter().map(|i| commands.list[i].clone()).collect());
        self.slash.matches = Some(Matches { cwd, commands, query, found: Rc::clone(&found) });
        Some(found)
    }

    /// Loads a folder's saved commands the first time it's shown, or for a
    /// folder never seen, asks the CLI.
    fn load_commands(&mut self, cwd: &Path, cx: &mut Context<Self>) {
        if self.slash.commands.contains_key(cwd) {
            return;
        }
        let known = Known { commands: Rc::default(), loading: true, fresh: false };
        self.slash.commands.insert(cwd.to_path_buf(), known);
        let cwd = cwd.to_path_buf();
        cx.spawn(async move |this, cx| {
            let dir = cwd.clone();
            let result = cx
                .background_spawn(async move {
                    match commands::saved(&dir) {
                        Some(list) => Ok((list, false)),
                        None => commands::list(&dir).map(|list| (list, true)),
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                match result {
                    Ok((list, fresh)) if fresh => this.commands_reported(cwd.clone(), list, cx),
                    Ok((list, _)) => {
                        if let Some(k) = this.slash.commands.get_mut(&cwd).filter(|k| !k.fresh) {
                            k.commands = Rc::new(Commands::new(list));
                        }
                    }
                    Err(e) => eprintln!("kynex: couldn't list slash commands: {e}"),
                }
                if let Some(k) = this.slash.commands.get_mut(&cwd) {
                    k.loading = false;
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// A folder's fresh list, from a session that started there: shown from now
    /// on, and saved for the next start if it changed.
    pub(super) fn commands_reported(&mut self, cwd: PathBuf, list: Vec<SlashCommand>, cx: &mut Context<Self>) {
        if list.is_empty() {
            return;
        }
        let same = self.slash.commands.get(&cwd).is_some_and(|k| k.commands.list == list);
        if !same {
            let dir = cwd.clone();
            let to_save = list.clone();
            cx.background_spawn(async move {
                if let Err(e) = commands::save(&dir, &to_save) {
                    eprintln!("kynex: {e}");
                }
            })
            .detach();
        }
        let k =
            self.slash.commands.entry(cwd).or_insert(Known { commands: Rc::default(), loading: false, fresh: true });
        k.fresh = true;
        if !same {
            k.commands = Rc::new(Commands::new(list));
        }
    }

    /// The popup above the composer, while a `/name` is being typed.
    pub(super) fn render_slash(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        // Load the shown folder's commands before the first `/`, so it opens full.
        if let Some(cwd) = self.active_session().map(|s| s.cwd.clone()).filter(|c| !self.slash.commands.contains_key(c))
        {
            self.load_commands(&cwd, cx);
        }
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
        let Some(c) = self.slash_matches(cx).and_then(|m| m.get(i).cloned()) else { return };
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
