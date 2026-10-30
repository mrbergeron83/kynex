//! `@file` mentions in the composer: while an `@path` is typed in a project
//! session, a popup of the project's files and folders (Up / Down to move, Tab
//! or Enter to take one, Esc to close). Taking a folder lists what's in it.
//!
//! The file list is read in the background each time a mention starts; the
//! last one shows meanwhile, so a file made since turns up a moment later.

use std::collections::HashMap;
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;

use crate::ui::px;
use gpui::{AnyElement, Context, Focusable, Window, prelude::*};
use kynex_core::mentions::{self, Files, Match};
use kynex_core::session::Kind;

use super::KynexApp;
use crate::ui::completions::{Completion, completions};

/// Matches listed at most; the popup scrolls through them.
const LISTED: usize = 50;

/// The popup's state, and each folder's files.
#[derive(Default)]
pub(super) struct Mentions {
    files: HashMap<PathBuf, Known>,
    /// The last matches, so the popup filters once per keystroke.
    matches: Option<Matches>,
    selected: usize,
    /// What the selection belongs to; a new query starts at the top again.
    query: String,
    /// The composer's text when Esc closed the popup; it stays closed until that changes.
    pub(super) dismissed: Option<String>,
    /// A mention was being typed at the last render; a new one re-reads the files.
    typing: bool,
}

#[derive(Default)]
struct Known {
    files: Rc<Files>,
    loading: bool,
}

struct Matches {
    cwd: PathBuf,
    files: Rc<Files>,
    query: String,
    found: Rc<Vec<Match>>,
}

/// The mention being typed: its `@token` in the composer, what's after the
/// `@`, and the session's folder.
struct Typing {
    token: Range<usize>,
    query: String,
    cwd: PathBuf,
}

impl KynexApp {
    fn mention_typed(&self, cx: &Context<Self>) -> Option<Typing> {
        let session = self.active_session().filter(|s| s.kind == Kind::Code)?;
        let input = self.composer.read(cx);
        if self.prompt.is_some() || self.mentions.dismissed.as_deref() == Some(input.text()) {
            return None;
        }
        let (token, query) = mentions::at_cursor(input.text(), input.cursor()?)?;
        Some(Typing { token, query: query.to_string(), cwd: session.cwd.clone() })
    }

    /// The folder's files and the entries matching what's typed, or `None`
    /// when no mention is.
    fn mention_matches(&mut self, cx: &mut Context<Self>) -> Option<(Rc<Files>, Rc<Vec<Match>>)> {
        let typing = self.mention_typed(cx)?;
        let files = self.mentions.files.get(&typing.cwd).map(|k| k.files.clone()).unwrap_or_default();
        if let Some(m) = &self.mentions.matches
            && m.cwd == typing.cwd
            && m.query == typing.query
            && Rc::ptr_eq(&m.files, &files)
        {
            return Some((files, m.found.clone()));
        }
        let found = Rc::new(files.matching(&typing.query, LISTED));
        let Typing { cwd, query, .. } = typing;
        self.mentions.matches = Some(Matches { cwd, files: files.clone(), query, found: found.clone() });
        Some((files, found))
    }

    /// Whether the popup is up with something to take.
    fn mention_open(&mut self, cx: &mut Context<Self>) -> Option<usize> {
        self.mention_matches(cx).map(|(_, m)| m.len()).filter(|&n| n > 0)
    }

    /// Reads `cwd`'s files in the background, unless that's already under way.
    fn load_files(&mut self, cwd: PathBuf, cx: &mut Context<Self>) {
        let known = self.mentions.files.entry(cwd.clone()).or_default();
        if known.loading {
            return;
        }
        known.loading = true;
        cx.spawn(async move |this, cx| {
            let dir = cwd.clone();
            let files = cx.background_spawn(async move { Files::new(mentions::list(&dir)) }).await;
            this.update(cx, |this, cx| {
                let known = this.mentions.files.entry(cwd).or_default();
                known.files = Rc::new(files);
                known.loading = false;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The popup above the composer, while an `@path` is being typed.
    pub(super) fn render_mentions(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let typing = self.mention_typed(cx);
        let started = typing.is_some() && !self.mentions.typing;
        self.mentions.typing = typing.is_some();
        let typing = typing?;
        if started {
            self.load_files(typing.cwd.clone(), cx);
        }
        if self.mentions.query != typing.query {
            self.mentions.query = typing.query.clone();
            self.mentions.selected = 0;
        }
        let (files, found) = self.mention_matches(cx)?;
        let loading = self.mentions.files.get(&typing.cwd).is_some_and(|k| k.loading);
        if found.is_empty() && !loading {
            return None;
        }
        self.mentions.selected = self.mentions.selected.min(found.len().saturating_sub(1));
        let items: Rc<Vec<Completion>> = found
            .iter()
            .map(|m| Completion {
                label: files.entries[m.index].clone().into(),
                matched: m.positions.clone(),
                ..Default::default()
            })
            .collect::<Vec<_>>()
            .into();
        // Shares the slash list's slot, so it plays out the same way when it closes.
        self.slash.shown = Some((items.clone(), self.mentions.selected));
        let this = cx.entity().downgrade();
        let list = completions(&items, self.mentions.selected, "Listing files…", cx, move |i, window, cx| {
            this.update(cx, |this, cx| this.take_mention(i, window, cx)).ok();
        });
        Some(list.w_full().max_w(px(560.)).into_any_element())
    }

    /// Moves the selection; false if the popup is closed.
    pub(super) fn mention_move(&mut self, down: bool, cx: &mut Context<Self>) -> bool {
        let Some(n) = self.mention_open(cx) else { return false };
        let s = self.mentions.selected;
        self.mentions.selected = if down { (s + 1) % n } else { (s + n - 1) % n };
        cx.notify();
        true
    }

    /// Takes the selected entry (Tab or Enter); false if the popup is closed.
    pub(super) fn mention_accept(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.mention_open(cx).is_none() {
            return false;
        }
        self.take_mention(self.mentions.selected, window, cx);
        true
    }

    /// Esc: closes the popup; false if it wasn't open.
    pub(super) fn mention_dismiss(&mut self, cx: &mut Context<Self>) -> bool {
        if self.mention_open(cx).is_none() {
            return false;
        }
        self.mentions.dismissed = Some(self.composer.read(cx).text().to_string());
        cx.notify();
        true
    }

    /// Puts entry `i` in place of the `@token` being typed.
    fn take_mention(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(typing) = self.mention_typed(cx) else { return };
        let Some((files, found)) = self.mention_matches(cx) else { return };
        let Some(m) = found.get(i) else { return };
        let insert = mentions::insert_text(&files.entries[m.index]);
        let range = mentions::replace_range(self.composer.read(cx).text(), typing.token);
        self.composer.update(cx, |input, cx| input.replace(range, &insert, cx));
        self.composer.read(cx).focus_handle(cx).focus(window, cx);
        cx.notify();
    }
}
