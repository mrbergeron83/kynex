//! What you do to a pane besides typing in it: its header's lamp, zoom and close, its
//! right-click menu (split, zoom, move to a tab, close), the tabs' menu, and putting an
//! existing session beside a pane ("Open to the side", or a session row dropped on a pane's
//! edge; dropped in the middle it takes the pane's place).

use gpui::{Anchor, AnyElement, Context, MouseButton, MouseDownEvent, Window, canvas, div, prelude::*};
use kynex_core::layout::{Axis, Direction, PaneId, Rect};
use kynex_core::live::Key;
use kynex_core::session::Session;
use kynex_core::tiling::Content;
use kynex_core::toasts::Toast;

use super::KynexApp;
use super::menus::Menu;
use super::session_groups::DraggedSession;
use crate::theme::{mono, theme};
use crate::ui::{Icon, Lamp, Tooltip, divider, icon_button, lamp, menu_item, px};

type Action = Box<dyn Fn(&mut KynexApp, &mut Window, &mut Context<KynexApp>)>;

impl KynexApp {
    /// Shows the past session `id` of the workspace you're in at `side` of pane `target`
    /// (the focused one if `None`), or in that pane's place with no side. One that is on
    /// show already gets the focus instead.
    pub(super) fn open_beside(
        &mut self,
        id: String,
        target: Option<PaneId>,
        side: Option<Direction>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((project, ws, _)) = self.tiling_scope() else { return };
        let Some((kind, cwd)) = self.panel_folder() else { return };
        self.edit_groups(&cwd, |g| {
            g.unarchive(&id);
        });
        let Some(target) = target.or_else(|| self.tiles.get(&ws).and_then(|t| t.focus())) else { return };
        let open = self
            .code
            .get(&project)
            .and_then(|l| l.sessions.iter().find(|s| s.session_id.as_deref() == Some(id.as_str())))
            .map(|s| s.uid);
        if let Some(key) = open
            && let Some(pane) = self.tiles.get(&ws).and_then(|t| t.pane_of(key))
        {
            self.focus_pane(pane, window, cx);
            self.focus_input(window, cx);
            return;
        }
        let fresh = match open {
            Some(_) => None,
            None => {
                let mut s = Session::resume(kind, cwd, id, self.wake.clone());
                self.adopt_project_settings(&mut s);
                self.start_model(&mut s);
                s.wake = self.wake.clone();
                Some(s)
            }
        };
        let Some(key) = open.or(fresh.as_ref().map(|s| s.uid)) else { return };
        let pane = self.next_pane;
        let bounds = self.tile_bounds();
        let Some(t) = self.tiles.get_mut(&ws) else { return };
        let placed = match side {
            Some(side) => t.split_at(target, side, pane, key, bounds),
            None => t.show_in(target, key) && t.focus_pane(target),
        };
        if !placed {
            self.toast(Toast::error("No room to split here: the panes would get too small.".to_string()));
            cx.notify();
            return;
        }
        if side.is_some() {
            self.next_pane += 1;
        }
        if let Some(live) = self.code.get_mut(&project) {
            if let Some(s) = fresh {
                live.sessions.push(s);
            }
            live.focus(key);
        }
        self.swap_to(if side.is_some() { pane } else { target }, window, cx);
        self.focus_input(window, cx);
        cx.notify();
    }

    /// A session row let go over a pane: near an edge it splits there, in the middle it
    /// takes the pane's place. `rect` is the pane's place in the panes' area (all of it
    /// for a single pane).
    pub(super) fn drop_session_on_pane(
        &mut self,
        d: &DraggedSession,
        pane: PaneId,
        rect: Option<Rect>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(area) = self.tile_area.get() else { return };
        let Some(id) = d.ids.first().cloned() else { return };
        let z = self.saved.zoom;
        let pos = window.mouse_position();
        let (x, y) = (f32::from(pos.x - area.origin.x) / z, f32::from(pos.y - area.origin.y) / z);
        let rect = rect.unwrap_or_else(|| {
            let b = self.tile_bounds();
            Rect::new(0.0, 0.0, b.w, b.h)
        });
        self.open_beside(id, Some(pane), kynex_core::tiling::drop_side(rect, x, y), window, cx);
    }

    /// Closes pane `id` (the last session pane stays). A session that never got a message
    /// goes with it; a terminal is hidden.
    pub(super) fn close_pane_id(&mut self, id: PaneId, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, ws, _)) = self.tiling_scope() else { return };
        if self.tiles.get(&ws).is_some_and(|t| t.is_terminal(id)) {
            self.hide_terminal(id, window, cx);
            return;
        }
        let Some(Content::Session(key)) = self.tiles.get_mut(&ws).and_then(|t| t.close_pane(id)) else { return };
        self.show_focused_pane(window, cx);
        if self.session_by_key(key).is_some_and(Session::is_new) {
            self.close_open(key);
        }
        self.focus_input(window, cx);
    }

    /// Zooms the focused pane to fill its tab, or puts the others back.
    pub(super) fn toggle_pane_zoom(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, ws, _)) = self.tiling_scope() else { return };
        if let Some(t) = self.tiles.get_mut(&ws) {
            t.toggle_zoom();
        }
        self.focus_input(window, cx);
        cx.notify();
    }

    fn move_pane(&mut self, pane: PaneId, tab: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, ws, _)) = self.tiling_scope() else { return };
        let bounds = self.tile_bounds();
        let Some(t) = self.tiles.get_mut(&ws) else { return };
        let moved = match tab {
            Some(tab) => t.move_to_tab(pane, tab, bounds),
            None => t.move_to_new_tab(pane),
        };
        if !moved {
            self.toast(Toast::error("No room for the pane in that tab.".to_string()));
        }
        self.show_focused_pane(window, cx);
    }

    /// A pane's header: the session's, with the pane's lamp, model, zoom and close when
    /// there are several (`tiled`). Right-click opens the pane's menu.
    pub(super) fn render_tile_header(
        &mut self,
        pane: PaneId,
        key: Option<Key>,
        tiled: bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let key = key?;
        let t = theme(cx).clone();
        let s = self.session_by_key(key)?;
        let state = Lamp::of(s.busy, s.waiting(), s.finished);
        let model = s.model.clone();
        let zoomed =
            self.tiling_scope().and_then(|(_, ws, _)| self.tiles.get(&ws)).is_some_and(|t| t.zoomed().is_some());
        let n = pane as usize;
        let lead = tiled.then(|| lamp(("pane-lamp", n), state, cx));
        let mut tail: Vec<AnyElement> = Vec::new();
        if tiled {
            if !model.is_empty() {
                tail.push(
                    div()
                        .flex_none()
                        .max_w(px(120.))
                        .truncate()
                        .text_xs()
                        .font_family(mono())
                        .text_color(t.text_placeholder)
                        .child(model)
                        .into_any_element(),
                );
            }
            tail.push(
                icon_button(("pane-zoom", n), if zoomed { Icon::Restore } else { Icon::Maximize }, cx)
                    .tooltip(Tooltip::text(if zoomed { "Show the other panes again" } else { "Zoom: fill the tab" }))
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_pane_zoom(window, cx)))
                    .into_any_element(),
            );
            tail.push(
                icon_button(("pane-close", n), Icon::Close, cx)
                    .tooltip(Tooltip::text("Close pane"))
                    .on_click(cx.listener(move |this, _, window, cx| this.close_pane_id(pane, window, cx)))
                    .into_any_element(),
            );
        }
        let header = self.render_session_header_with(key, lead, tail, cx)?;
        Some(
            div()
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                        this.open_menu(Menu::Pane(pane), ev.position, Anchor::TopLeft, cx)
                    }),
                )
                .child(header)
                .into_any_element(),
        )
    }

    /// The single pane's conversation as a place to drop a session row: on an edge it
    /// splits, in the middle it opens there. Outside a project's Code view, just `body`.
    pub(super) fn single_pane_drop(&mut self, body: AnyElement, cx: &mut Context<Self>) -> AnyElement {
        if self.tiling_scope().is_none() {
            return body;
        }
        let pane = self.pane_id;
        let area = self.tile_area.clone();
        let probe = canvas(move |bounds, _, _| area.set(Some(bounds)), |_, _, _, _| {}).absolute().inset_0();
        div()
            .relative()
            .flex()
            .flex_1()
            .min_h_0()
            .child(probe)
            .child(body)
            .on_drop(cx.listener(move |this, d: &DraggedSession, window, cx| {
                this.drop_session_on_pane(d, pane, None, window, cx)
            }))
            .into_any_element()
    }

    fn menu_action(n: usize, label: String, cx: &mut Context<Self>, f: Action) -> AnyElement {
        menu_item(("pane-menu", n), label, false, cx)
            .on_click(cx.listener(move |this, _, window, cx| {
                this.menu = None;
                f(this, window, cx);
                cx.notify();
            }))
            .into_any_element()
    }

    /// A pane's right-click menu: split, zoom, move to a tab, close.
    pub(super) fn pane_menu_items(&mut self, pane: PaneId, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some((_, ws, _)) = self.tiling_scope() else { return Vec::new() };
        let Some(t) = self.tiles.get(&ws) else { return Vec::new() };
        let tabs = t.tabs();
        let zoomed = t.zoomed().is_some();
        let several = t.visible_panes().len() > 1 || zoomed;
        let here = tabs.iter().position(|t| t.active).unwrap_or(0);
        let terminal = t.is_terminal(pane);
        let closable = terminal || t.panes().len() > 1 && t.sessions().len() > 1;
        // A terminal splits into another terminal; a session into a new session.
        let split = move |axis: Axis| -> Action {
            Box::new(move |this: &mut KynexApp, window: &mut Window, cx: &mut Context<KynexApp>| {
                if terminal {
                    let side = if axis == Axis::Row { Direction::Right } else { Direction::Down };
                    this.split_terminal(pane, side, window, cx);
                } else {
                    this.focus_pane(pane, window, cx);
                    this.split_pane(axis, window, cx);
                }
            })
        };
        let mut items = vec![
            Self::menu_action(0, "Split right".into(), cx, split(Axis::Row)),
            Self::menu_action(1, "Split down".into(), cx, split(Axis::Column)),
        ];
        if several {
            items.push(Self::menu_action(
                2,
                if zoomed { "Unzoom".into() } else { "Zoom".into() },
                cx,
                Box::new(move |this, window, cx| {
                    this.focus_pane(pane, window, cx);
                    this.toggle_pane_zoom(window, cx);
                }),
            ));
            items.push(div().my_1().child(divider(cx)).into_any_element());
            items.push(Self::menu_action(
                3,
                "Move to a new tab".into(),
                cx,
                Box::new(move |this, window, cx| this.move_pane(pane, None, window, cx)),
            ));
        }
        for (i, tab) in tabs.iter().enumerate().filter(|(i, _)| *i != here) {
            items.push(Self::menu_action(
                100 + i,
                format!("Move to tab {}", tab.name),
                cx,
                Box::new(move |this, window, cx| this.move_pane(pane, Some(i), window, cx)),
            ));
        }
        if closable {
            items.push(div().my_1().child(divider(cx)).into_any_element());
            items.push(Self::menu_action(
                4,
                if terminal { "Hide terminal".into() } else { "Close pane".into() },
                cx,
                Box::new(move |this, window, cx| this.close_pane_id(pane, window, cx)),
            ));
        }
        items
    }

    /// A pane tab's right-click menu.
    pub(super) fn pane_tab_menu_items(&mut self, tab: usize, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some((_, ws, _)) = self.tiling_scope() else { return Vec::new() };
        let Some(tabs) = self.tiles.get(&ws).map(|t| t.tabs()) else { return Vec::new() };
        let Some(name) = tabs.get(tab).map(|t| t.name.clone()) else { return Vec::new() };
        let mut items = vec![
            Self::menu_action(
                0,
                "Rename".into(),
                cx,
                Box::new(move |this, window, cx| this.start_tab_rename(tab, name.clone(), window, cx)),
            ),
            Self::menu_action(1, "New tab".into(), cx, Box::new(|this, window, cx| this.new_pane_tab(window, cx))),
        ];
        if tabs.len() > 1 {
            items.push(div().my_1().child(divider(cx)).into_any_element());
            items.push(Self::menu_action(
                2,
                "Close tab".into(),
                cx,
                Box::new(move |this, window, cx| this.close_pane_tab(tab, window, cx)),
            ));
        }
        items
    }
}
