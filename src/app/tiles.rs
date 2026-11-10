//! The split panes of a workspace: each workspace (a folder) has a [`Tiling`], its tabs of
//! panes and the session in each. The focused pane's [`SessionPane`] is `self.pane`, so
//! everything that talks to "the composer" keeps working; the others are parked in
//! `self.parked` and swapped in to be drawn or focused.
//!
//! A tiling follows the sessions: whichever session the window talks to
//! ([`Live::current`]) is in the focused pane, so opening one from the sessions panel
//! replaces that pane, and picking one that's on show focuses its pane.

use std::path::PathBuf;

use gpui::{AnyElement, ClickEvent, Context, Entity, Focusable, MouseButton, Window, canvas, div, prelude::*};
use kynex_core::layout::{Axis, DIVIDER, Divider, PaneId, Rect};
use kynex_core::live::Key;
use kynex_core::session::Session;
use kynex_core::tile_style::TileStyle;
use kynex_core::tiling::Tiling;

use super::KynexApp;
use super::session_groups::DraggedSession;
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

    /// The code project's workspace you're in: (project folder, workspace folder, shown session).
    /// `None` outside the Code view and in chat, which stays a single pane.
    pub(super) fn tiling_scope(&self) -> Option<(PathBuf, PathBuf, Key)> {
        let Active::Project(i) = self.saved.active else { return None };
        let p = self.saved.projects.get(i).filter(|p| p.view == View::Code)?;
        let s = self.code.get(&p.path)?.current()?;
        Some((p.path.clone(), s.cwd.clone(), s.uid))
    }

    /// The workspace's tiling when it has several panes on screen.
    fn tiled(&self) -> Option<&Tiling> {
        let (_, cwd, _) = self.tiling_scope()?;
        self.tiles.get(&cwd).filter(|t| t.visible().len() > 1 || t.zoomed().is_some())
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
    pub(super) fn swap_to(&mut self, id: PaneId, window: &mut Window, cx: &mut Context<Self>) {
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

    /// Focuses pane `id` of the workspace you're in, and the session in it.
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
    pub(super) fn show_focused_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
    pub(super) fn tile_bounds(&self) -> Rect {
        match self.tile_area.get() {
            Some(b) => {
                let z = self.saved.zoom;
                Rect::new(0.0, 0.0, f32::from(b.size.width) / z, f32::from(b.size.height) / z)
            }
            None => UNMEASURED,
        }
    }

    /// A new session for a new pane in `cwd`, under the project's settings.
    pub(super) fn pane_session(&mut self, kind: kynex_core::session::Kind, cwd: PathBuf) -> Session {
        let mut s = Session::new(kind, cwd);
        self.adopt_project_settings(&mut s);
        self.start_model(&mut s);
        s.wake = self.wake.clone();
        s
    }

    /// Splits the focused pane: a new session in the new pane beside (`Row`) or under it.
    pub(super) fn split_pane(&mut self, axis: Axis, window: &mut Window, cx: &mut Context<Self>) {
        let Some((project, cwd, _)) = self.tiling_scope() else { return };
        let Some(live) = self.code.get(&project) else { return };
        if self.tiles.get(&cwd).is_some_and(|t| t.visible().len() >= kynex_core::live::MAX_RUNNING) {
            self.toast(kynex_core::toasts::Toast::error(format!(
                "{} panes are up already. Close one to open another.",
                kynex_core::live::MAX_RUNNING
            )));
            cx.notify();
            return;
        }
        let Some(kind) = live.current().map(|s| s.kind) else { return };
        let id = self.next_pane;
        let s = self.pane_session(kind, cwd.clone());
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

    /// What each frame settles for the panes: the workspace's tiling follows the sessions,
    /// the sessions on screen are kept open, prompts and drafts are kept for every pane.
    pub(super) fn sync_tiles(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((project, cwd, current)) = self.tiling_scope() else { return };
        if !self.tiles.contains_key(&cwd) {
            // The first pane of a workspace takes the state that's up, if no workspace has it.
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
        // Every pane's session stays open: those of the other tabs and workspaces too, which
        // come back as they were.
        let held: Vec<Key> = self.tiles.values().flat_map(Tiling::sessions).collect();
        if let Some(live) = self.code.get_mut(&project) {
            live.set_visible(&held);
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

    /// The area the tiles are laid out in, in the container's own logical pixels: the
    /// container less half a gap on each side (each tile is inset by half a gap too, which
    /// makes the margin round the outside a whole gap).
    fn core_bounds(&self, gap: f32) -> Rect {
        let b = self.tile_bounds();
        let half = gap / 2.0;
        Rect::new(half, half, (b.w - gap).max(0.0), (b.h - gap).max(0.0))
    }

    /// The tiles' look: the theme's, with what Appearance changed.
    fn tile_style(&self, cx: &Context<Self>) -> TileStyle {
        self.saved.tiles.resolve(theme(cx).tile)
    }

    /// A divider is grabbed by its handle: a strip as wide as the gap (at least a few
    /// pixels) along it.
    fn handle_of(d: &Divider, gap: f32) -> Rect {
        let grow = (gap / 2.0).max(3.0);
        let r = d.rect;
        match d.axis {
            Axis::Row => Rect::new(r.x - grow, r.y, r.w + 2.0 * grow, r.h),
            Axis::Column => Rect::new(r.x, r.y - grow, r.w, r.h + 2.0 * grow),
        }
    }

    /// Starts dragging the dividers `splits` (one, or two where a row and a column meet).
    fn start_tile_drag(&mut self, splits: Vec<Resizing>, cx: &mut Context<Self>) {
        self.tile_drag = Some(splits);
        cx.notify();
    }

    /// The pointer moved with dividers held: they follow it, live, and the panes inside
    /// keep their proportions (the ratios are fractions).
    pub(super) fn drag_tiles(&mut self, pos: gpui::Point<gpui::Pixels>, cx: &mut Context<Self>) {
        let Some(drag) = self.tile_drag.clone() else { return };
        let Some(area) = self.tile_area.get() else { return };
        let Some((_, cwd, _)) = self.tiling_scope() else { return };
        let z = self.saved.zoom;
        let (x, y) = (f32::from(pos.x - area.origin.x) / z, f32::from(pos.y - area.origin.y) / z);
        let bounds = self.core_bounds(self.tile_style(cx).gap);
        let Some(t) = self.tiles.get_mut(&cwd) else { return };
        for Resizing { path, axis } in drag {
            // Looked up afresh: moving one divider moves the area of those inside it.
            let rects = t.rects(bounds);
            let Some(d) = rects.dividers.iter().find(|d| d.path == path) else { continue };
            let (at, from, len) = match axis {
                Axis::Row => (x, d.area.x, d.area.w),
                Axis::Column => (y, d.area.y, d.area.h),
            };
            let avail = len - DIVIDER;
            if avail > 0.0 {
                t.set_ratio(&path, (at - from - DIVIDER / 2.0) / avail, bounds);
            }
        }
        cx.notify();
    }

    /// The divider `path` back to the middle of its split (double-click).
    fn even_out(&mut self, path: &[bool], cx: &mut Context<Self>) {
        let Some((_, cwd, _)) = self.tiling_scope() else { return };
        let bounds = self.core_bounds(self.tile_style(cx).gap);
        if let Some(t) = self.tiles.get_mut(&cwd) {
            t.set_ratio(path, 0.5, bounds);
        }
        cx.notify();
    }

    /// The panes of the workspace, when it has several on screen: each a session view in a
    /// tile (a rounded, bordered surface with a gap round it; the focused tile's border is
    /// in the project's colour and a pixel thicker), with a handle on every divider to drag.
    /// With no gap the panes are flush, split by 1px dividers. `None` for one pane, which
    /// `render_code_main` draws as it always has.
    pub(super) fn render_panes(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let t = theme(cx).clone();
        let style = self.tile_style(cx);
        let bounds = self.core_bounds(style.gap);
        let tiling = self.tiled()?;
        let rects = tiling.rects(bounds);
        let measured = self.tile_area.get().is_some();
        let panes: Vec<(PaneId, Option<Key>, Rect)> =
            rects.panes.iter().map(|(id, r)| (*id, tiling.key_of(*id), *r)).collect();
        let focus = tiling.focus();
        let focus_colour = self.active_colour(&t).unwrap_or(t.border_focused);
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
        let flush = style.gap == 0.0;
        let half = style.gap / 2.0;
        // The focused tile's border is a pixel thicker; the others make up the pixel as padding.
        let (bw, fw) = if flush { (0, 0) } else { (style.border as u32, (style.border as u32 + 1).min(4)) };
        let radius = if flush { 0.0 } else { style.radius };
        let mut children: Vec<AnyElement> = Vec::new();
        if measured {
            for (id, key, r) in panes {
                let focused = Some(id) == focus;
                let body = self.with_pane(id, |this| {
                    div()
                        .size_full()
                        .flex()
                        .flex_col()
                        .children(this.render_tile_header(id, key, true, cx))
                        .child(this.render_pane_body(key, focused, window, cx))
                });
                let width = if focused { fw } else { bw };
                let surface = bordered(div().size_full().overflow_hidden().bg(t.editor), width)
                    .border_color(if focused { focus_colour } else { t.border })
                    .rounded(px(radius))
                    .child(body);
                let tile = Rect::new(r.x + half, r.y + half, (r.w - style.gap).max(0.0), (r.h - style.gap).max(0.0));
                children.push(
                    div()
                        .absolute()
                        .left(px(r.x + half))
                        .top(px(r.y + half))
                        .w(px((r.w - style.gap).max(0.0)))
                        .h(px((r.h - style.gap).max(0.0)))
                        .when(!focused && fw > bw, |d| d.p_px())
                        // The click that focuses a pane comes before what it lands on.
                        .capture_any_mouse_down(cx.listener(move |this, _, window, cx| {
                            this.focus_pane(id, window, cx);
                        }))
                        // A session row let go here: on an edge it splits, in the middle it opens.
                        .drag_over::<DraggedSession>(|style, _, _, _| style.opacity(0.7))
                        .on_drop(cx.listener(move |this, d: &DraggedSession, window, cx| {
                            this.drop_session_on_pane(d, id, Some(tile), window, cx)
                        }))
                        .child(surface)
                        .into_any_element(),
                );
            }
            if flush {
                for d in &rects.dividers {
                    let r = d.rect;
                    children.push(
                        div()
                            .absolute()
                            .left(px(r.x))
                            .top(px(r.y))
                            .w(px(r.w))
                            .h(px(r.h))
                            .bg(t.border)
                            .into_any_element(),
                    );
                }
            }
            // Handles: one per divider, and one where a row's meets a column's, which drags both.
            let hits: Vec<Rect> = rects.dividers.iter().map(|d| Self::handle_of(d, style.gap)).collect();
            let splits: Vec<Resizing> =
                rects.dividers.iter().map(|d| Resizing { path: d.path.clone(), axis: d.axis }).collect();
            let mut handles: Vec<(Rect, Vec<Resizing>, Option<Axis>)> = Vec::new();
            for (i, d) in rects.dividers.iter().enumerate() {
                handles.push((hits[i], vec![splits[i].clone()], Some(d.axis)));
            }
            for i in 0..hits.len() {
                for j in i + 1..hits.len() {
                    if rects.dividers[i].axis != rects.dividers[j].axis
                        && let Some(both) = overlap(hits[i], hits[j])
                    {
                        handles.push((both, vec![splits[i].clone(), splits[j].clone()], None));
                    }
                }
            }
            for (n, (r, drag, axis)) in handles.into_iter().enumerate() {
                let first = drag[0].path.clone();
                let el = div().id(("tile-handle", n)).absolute().left(px(r.x)).top(px(r.y)).w(px(r.w)).h(px(r.h));
                let el = match axis {
                    Some(Axis::Row) => el.cursor_col_resize(),
                    Some(Axis::Column) => el.cursor_row_resize(),
                    None => el.cursor_move(),
                };
                children.push(
                    el.on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| this.start_tile_drag(drag.clone(), cx)),
                    )
                    .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                        if ev.click_count() == 2 {
                            this.even_out(&first, cx);
                        }
                    }))
                    .into_any_element(),
                );
            }
        }
        Some(
            div()
                .relative()
                .flex_1()
                .min_h_0()
                .min_w_0()
                .bg(if flush { t.editor } else { t.panel })
                .child(probe)
                .children(children)
                .into_any_element(),
        )
    }
}

/// A divider being dragged: which split, and along which axis.
#[derive(Clone)]
pub(super) struct Resizing {
    path: Vec<bool>,
    axis: Axis,
}

/// `d` with its border `w` pixels wide (0 to 4).
fn bordered(d: gpui::Div, w: u32) -> gpui::Div {
    match w {
        0 => d.border_0(),
        1 => d.border_1(),
        2 => d.border_2(),
        3 => d.border_3(),
        _ => d.border_4(),
    }
}

/// Where two rectangles cross, if they do.
fn overlap(a: Rect, b: Rect) -> Option<Rect> {
    let (x, y) = (a.x.max(b.x), a.y.max(b.y));
    let (r, bottom) = ((a.x + a.w).min(b.x + b.w), (a.y + a.h).min(b.y + b.h));
    (r > x && bottom > y).then(|| Rect::new(x, y, r - x, bottom - y))
}
