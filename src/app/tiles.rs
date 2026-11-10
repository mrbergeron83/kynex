//! The split panes of a context: each context (a folder) has a [`Tiling`], its tabs of
//! panes and the session in each. The focused pane's [`SessionPane`] is `self.pane`, so
//! everything that talks to "the composer" keeps working; the others are parked in
//! `self.parked` and swapped in to be drawn or focused.
//!
//! A tiling follows the sessions: whichever session the window talks to
//! ([`Live::current`]) is in the focused pane, so opening one from the sessions panel
//! replaces that pane, and picking one that's on show focuses its pane.

use std::path::PathBuf;

use gpui::{AnyElement, Context, Entity, Focusable, Window, canvas, div, prelude::*};
use kynex_core::layout::{Axis, PaneId, Rect};
use kynex_core::live::Key;
use kynex_core::session::Session;
use kynex_core::tiling::Tiling;

use super::KynexApp;
use super::state::{Active, View};
use crate::pane::SessionPane;
use crate::theme::theme;
use crate::ui::input::{InputEvent, TextInput};
use crate::ui::px;

/// Room for a split when the area hasn't been measured yet.
const UNMEASURED: Rect = Rect { x: 0.0, y: 0.0, w: 4000.0, h: 4000.0 };

impl KynexApp {
    /// A message box for pane `id`: sending, pasting and focusing it act on that pane.
    pub(super) fn new_composer(id: PaneId, window: &mut Window, cx: &mut Context<Self>) -> Entity<TextInput> {
        let composer = cx.new(|cx| TextInput::new(cx).multiline());
        cx.subscribe_in(&composer, window, move |this, _, ev, window, cx| match ev {
            InputEvent::Submit => {
                this.focus_pane(id, window, cx);
                this.send(window, cx);
            }
            InputEvent::PasteImages(images) => {
                this.focus_pane(id, window, cx);
                for i in images {
                    this.attach_bytes(&i.bytes, cx);
                }
            }
            InputEvent::Changed => {
                if this.pane_id == id {
                    this.warm_shown();
                }
            }
        })
        .detach();
        // Focusing the message box means a message is coming: that pane is the one, and
        // Claude starts now.
        let focus = composer.read(cx).focus_handle(cx);
        cx.on_focus(&focus, window, move |this, window, cx| {
            this.focus_pane(id, window, cx);
            this.warm_shown();
        })
        .detach();
        composer
    }

    /// The code project's context you're in: (project folder, context folder, shown session).
    /// `None` outside the Code view and in chat and KNX Ops, which stay a single pane.
    fn tiling_scope(&self) -> Option<(PathBuf, PathBuf, Key)> {
        if self.in_chief() {
            return None;
        }
        let Active::Project(i) = self.saved.active else { return None };
        let p = self.saved.projects.get(i).filter(|p| p.view == View::Code)?;
        let s = self.code.get(&p.path)?.current()?;
        Some((p.path.clone(), s.cwd.clone(), s.uid))
    }

    /// The context's tiling when it has several panes on screen.
    fn tiled(&self) -> Option<&Tiling> {
        let (_, cwd, _) = self.tiling_scope()?;
        self.tiles.get(&cwd).filter(|t| t.visible().len() > 1)
    }

    /// Runs `f` with pane `id`'s state as `self.pane`.
    pub(super) fn with_pane<R>(&mut self, id: PaneId, f: impl FnOnce(&mut Self) -> R) -> R {
        if id == self.pane_id {
            return f(self);
        }
        let Some(other) = self.parked.remove(&id) else { return f(self) };
        let mine = std::mem::replace(&mut self.pane, other);
        let out = f(self);
        let other = std::mem::replace(&mut self.pane, mine);
        self.parked.insert(id, other);
        out
    }

    /// Makes `id`'s state `self.pane`, parking the one that was. If the keyboard was in the
    /// old message box, it goes to the new one.
    fn swap_to(&mut self, id: PaneId, window: &mut Window, cx: &mut Context<Self>) {
        if id == self.pane_id {
            return;
        }
        let next = match self.parked.remove(&id) {
            Some(p) => p,
            None => SessionPane::new(Self::new_composer(id, window, cx)),
        };
        let had_keys = self.pane.composer.read(cx).is_focused(window);
        let old = std::mem::replace(&mut self.pane, next);
        self.parked.insert(self.pane_id, old);
        self.pane_id = id;
        if had_keys {
            self.pane.composer.read(cx).focus_handle(cx).focus(window, cx);
        }
    }

    /// Focuses pane `id` of the context you're in, and the session in it.
    pub(super) fn focus_pane(&mut self, id: PaneId, window: &mut Window, cx: &mut Context<Self>) {
        if id == self.pane_id {
            return;
        }
        let Some((project, cwd, _)) = self.tiling_scope() else { return };
        let Some(t) = self.tiles.get_mut(&cwd) else { return };
        if !t.focus_pane(id) {
            return;
        }
        if let Some(key) = t.key_of(id)
            && let Some(live) = self.code.get_mut(&project)
        {
            live.focus(key);
        }
        self.swap_to(id, window, cx);
        cx.notify();
    }

    /// Lets the focused pane's session be the shown one, after the layout changed it.
    fn show_focused_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((project, cwd, _)) = self.tiling_scope() else { return };
        let Some(t) = self.tiles.get(&cwd) else { return };
        let Some(focus) = t.focus() else { return };
        if let Some(key) = t.key_of(focus)
            && let Some(live) = self.code.get_mut(&project)
        {
            live.focus(key);
        }
        self.swap_to(focus, window, cx);
        cx.notify();
    }

    /// The area the panes fill, in logical pixels (what the layout works in).
    fn tile_bounds(&self) -> Rect {
        match self.tile_area.get() {
            Some(b) => {
                let z = self.saved.zoom;
                Rect::new(0.0, 0.0, f32::from(b.size.width) / z, f32::from(b.size.height) / z)
            }
            None => UNMEASURED,
        }
    }

    /// Splits the focused pane: a new session in the new pane beside (`Row`) or under it.
    pub(super) fn split_pane(&mut self, axis: Axis, window: &mut Window, cx: &mut Context<Self>) {
        let Some((project, cwd, _)) = self.tiling_scope() else { return };
        let Some(live) = self.code.get(&project) else { return };
        if live.visible().len() >= kynex_core::live::MAX_RUNNING {
            self.toast(kynex_core::toasts::Toast::error(format!(
                "{} panes are up already. Close one to open another.",
                kynex_core::live::MAX_RUNNING
            )));
            cx.notify();
            return;
        }
        let Some(kind) = live.current().map(|s| s.kind) else { return };
        let id = self.next_pane;
        let mut s = Session::new(kind, cwd.clone());
        self.adopt_project_settings(&mut s);
        self.start_model(&mut s);
        s.wake = self.wake.clone();
        let key = s.uid;
        let bounds = self.tile_bounds();
        let Some(t) = self.tiles.get_mut(&cwd) else { return };
        if !t.split(axis, id, key, bounds) {
            self.toast(kynex_core::toasts::Toast::error(
                "No room to split here: the panes would get too small.".to_string(),
            ));
            cx.notify();
            return;
        }
        self.next_pane += 1;
        if let Some(live) = self.code.get_mut(&project) {
            live.sessions.push(s);
            live.focus(key);
        }
        self.swap_to(id, window, cx);
        self.pane.composer.read(cx).focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    /// Closes the focused pane; its session stays open until it's idle and not recent.
    pub(super) fn close_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, cwd, key)) = self.tiling_scope() else { return };
        if self.tiles.get_mut(&cwd).and_then(Tiling::close_focused).is_none() {
            return;
        }
        // A session that never got a message goes with its pane.
        if self.session_by_key(key).is_some_and(Session::is_new) {
            self.close_open(key);
        }
        self.show_focused_pane(window, cx);
        self.focus_input(window, cx);
    }

    /// Focuses the next (`1`) or previous (`-1`) pane of the tab.
    pub(super) fn step_pane(&mut self, by: isize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(t) = self.tiled() else { return };
        let panes: Vec<PaneId> = t.visible().iter().map(|(p, _)| *p).collect();
        let at = t.focus().and_then(|f| panes.iter().position(|p| *p == f)).unwrap_or(0) as isize;
        let next = panes[(at + by).rem_euclid(panes.len() as isize) as usize];
        self.focus_pane(next, window, cx);
        self.focus_input(window, cx);
    }

    /// What each frame settles for the panes: the context's tiling follows the sessions,
    /// the sessions on screen are kept open, prompts and drafts are kept for every pane.
    pub(super) fn sync_tiles(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((project, cwd, current)) = self.tiling_scope() else { return };
        if !self.tiles.contains_key(&cwd) {
            // The first pane of a context takes the state that's up, if no context has it.
            let orphan = !self.tiles.values().any(|t| t.has(self.pane_id));
            let id = if orphan {
                self.pane_id
            } else {
                self.next_pane += 1;
                self.next_pane - 1
            };
            self.tiles.insert(cwd.clone(), Tiling::new(id, current));
        }
        let open: Vec<Key> =
            self.code.get(&project).map(|l| l.sessions.iter().map(|s| s.uid).collect()).unwrap_or_default();
        let Some(t) = self.tiles.get_mut(&cwd) else { return };
        t.reconcile(&open, current);
        let visible = t.visible();
        let focus = t.focus();
        if let Some(live) = self.code.get_mut(&project) {
            let keys: Vec<Key> = visible.iter().map(|(_, k)| *k).collect();
            live.set_visible(&keys);
        }
        if let Some(focus) = focus {
            self.swap_to(focus, window, cx);
        }
        // The other panes keep their prompts and drafts too.
        for (id, key) in visible {
            if id != self.pane_id {
                self.with_pane(id, |this| {
                    this.sync_draft(Some(key), cx);
                    this.sync_prompt(Some(key), window, cx);
                });
            }
        }
        let live: Vec<PaneId> = self.tiles.values().flat_map(|t| t.panes()).collect();
        let me = self.pane_id;
        self.parked.retain(|id, _| *id != me && live.contains(id));
    }

    /// Every pane's half-written message, put aside (for quitting).
    pub(super) fn stash_all_drafts(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<PaneId> = self.parked.keys().copied().collect();
        for id in ids {
            self.with_pane(id, |this| this.stash_draft(cx));
        }
        self.stash_draft(cx);
    }

    /// The panes of the context, when it has several on screen: each a session view in its
    /// rectangle, with a line between. `None` for one pane, which `render_code_main` draws
    /// as it always has.
    pub(super) fn render_panes(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let t = theme(cx).clone();
        let tiling = self.tiled()?;
        let tab = tiling.layout().tab()?;
        let rects = tab.rects(self.tile_bounds());
        let measured = self.tile_area.get().is_some();
        let panes: Vec<(PaneId, Option<Key>, Rect)> =
            rects.panes.iter().map(|(id, r)| (*id, tiling.key_of(*id), *r)).collect();
        let focus = tab.focus();
        let area = self.tile_area.clone();
        let probe = canvas(
            move |bounds, window, _| {
                if area.get() != Some(bounds) {
                    area.set(Some(bounds));
                    window.refresh();
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0();
        let mut children: Vec<AnyElement> = Vec::new();
        if measured {
            for (id, key, r) in panes {
                let focused = id == focus;
                let body = self.with_pane(id, |this| {
                    div()
                        .size_full()
                        .flex()
                        .flex_col()
                        .children(key.and_then(|k| this.render_session_header(k, cx)))
                        .child(this.render_pane_body(key, focused, window, cx))
                });
                children.push(
                    div()
                        .absolute()
                        .left(px(r.x))
                        .top(px(r.y))
                        .w(px(r.w))
                        .h(px(r.h))
                        .overflow_hidden()
                        // The click that focuses a pane comes before what it lands on.
                        .capture_any_mouse_down(cx.listener(move |this, _, window, cx| {
                            this.focus_pane(id, window, cx);
                        }))
                        .child(body)
                        .into_any_element(),
                );
            }
            for d in &rects.dividers {
                let r = d.rect;
                children.push(
                    div().absolute().left(px(r.x)).top(px(r.y)).w(px(r.w)).h(px(r.h)).bg(t.border).into_any_element(),
                );
            }
        }
        Some(div().relative().flex_1().min_h_0().min_w_0().child(probe).children(children).into_any_element())
    }
}
