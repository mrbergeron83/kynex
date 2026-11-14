//! The selected workspace's pane tabs: a row above the panes with one tab per layout, each
//! with a state lamp rolled up from its sessions. Click shows a tab, double-click renames
//! it, the × or a middle-click closes it, dragging reorders, and + opens a new one with a
//! new session. With one tab the row is hidden (New pane tab in the palette makes a second).
//! A pane's header dropped on a tab moves the pane there; dropped on + it gets a tab of its own.

use gpui::{AnyElement, ClickEvent, Context, Focusable, MouseButton, SharedString, Window, div, prelude::*};
use kynex_core::live::Key;
use kynex_core::session::Session;

use super::KynexApp;
use crate::theme::{theme, ui_font};
use crate::ui::input::{InputEvent, TextInput};
use crate::ui::wash::row_wash;
use crate::ui::{Icon, Lamp, Tooltip, icon_button, lamp, popover, px};

/// What follows the pointer while a pane tab is dragged.
#[derive(Clone)]
struct DraggedPaneTab {
    from: usize,
    label: SharedString,
}

impl Render for DraggedPaneTab {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        // Drawn outside the window's root, so it doesn't inherit the font.
        popover(cx)
            .font_family(ui_font())
            .flex_row()
            .items_center()
            .h(px(26.))
            .px_2p5()
            .text_sm()
            .text_color(t.text)
            .child(self.label.clone())
    }
}

pub(super) use crate::ui::pane_drag::DraggedPane;

impl KynexApp {
    /// A new tab in the workspace you're in, with a new session in its one pane.
    pub(super) fn new_pane_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((project, cwd, _)) = self.tiling_scope() else { return };
        let Some(kind) = self.code.get(&project).and_then(|l| l.current()).map(|s| s.kind) else { return };
        let id = self.next_pane;
        let s = self.pane_session(kind, cwd.clone());
        let key = s.uid;
        if !self.tiles.get_mut(&cwd).is_some_and(|t| t.new_tab(id, key)) {
            return;
        }
        self.next_pane += 1;
        if let Some(live) = self.code.get_mut(&project) {
            live.sessions.push(s);
            live.focus(key);
        }
        self.swap_to(id, window, cx);
        // A new tab starts as the others do, with the tree.
        let (ratio, bounds) = (self.side_share(cx), self.layout_bounds(cx));
        let mut next = self.next_pane;
        if let Some(t) = self.tiles.get_mut(&cwd) {
            t.add_side_panes(
                || {
                    next += 1;
                    next - 1
                },
                ratio,
                bounds,
            );
        }
        self.next_pane = next;
        self.pane.composer.read(cx).focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    /// Shows tab `i` of the workspace you're in.
    pub(super) fn select_pane_tab(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, cwd, _)) = self.tiling_scope() else { return };
        if self.tiles.get_mut(&cwd).is_some_and(|t| t.select_tab(i)) {
            self.show_focused_pane(window, cx);
            self.focus_input(window, cx);
        }
    }

    /// The next (`1`) or previous (`-1`) tab, wrapping round.
    pub(super) fn step_pane_tab(&mut self, by: isize, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, cwd, _)) = self.tiling_scope() else { return };
        let Some(tabs) = self.tiles.get(&cwd).map(|t| t.tabs()) else { return };
        if tabs.len() < 2 {
            return;
        }
        let at = tabs.iter().position(|t| t.active).unwrap_or(0) as isize;
        self.select_pane_tab((at + by).rem_euclid(tabs.len() as isize) as usize, window, cx);
    }

    /// Closes tab `i` (the last one stays). Its sessions stay open off screen, except
    /// those that never got a message, which go with it.
    pub(super) fn close_pane_tab(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, cwd, _)) = self.tiling_scope() else { return };
        let Some(closed) = self.tiles.get_mut(&cwd).map(|t| t.close_tab(i)) else { return };
        if closed.is_empty() {
            return;
        }
        self.pane_tab_rename = None;
        // The tab that's left decides which session the window talks to, before any close.
        self.show_focused_pane(window, cx);
        for (_, key) in closed {
            if self.session_by_key(key).is_some_and(Session::is_new) {
                self.close_open(key);
            }
        }
        self.focus_input(window, cx);
    }

    /// Closes the tab that's showing.
    pub(super) fn close_active_pane_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, cwd, _)) = self.tiling_scope() else { return };
        let Some(at) = self.tiles.get(&cwd).and_then(|t| t.tabs().iter().position(|t| t.active)) else { return };
        self.close_pane_tab(at, window, cx);
    }

    fn move_pane_tab(&mut self, from: usize, to: usize, cx: &mut Context<Self>) {
        let Some((_, cwd, _)) = self.tiling_scope() else { return };
        if let Some(t) = self.tiles.get_mut(&cwd) {
            t.move_tab(from, to);
        }
        self.pane_tab_rename = None;
        cx.notify();
    }

    /// Double-click: the tab's name becomes a field. Enter or clicking away keeps what's
    /// typed, Esc doesn't.
    pub(super) fn start_tab_rename(&mut self, i: usize, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| TextInput::new(cx).placeholder("Name"));
        input.update(cx, |f, cx| f.set_text(name, cx));
        cx.subscribe(&input, |this, _, ev, cx| {
            if let InputEvent::Submit = ev {
                this.commit_tab_rename(cx);
            }
        })
        .detach();
        let handle = input.read(cx).focus_handle(cx);
        cx.on_blur(&handle, window, |this, _, cx| this.commit_tab_rename(cx)).detach();
        handle.focus(window, cx);
        self.pane_tab_rename = Some((i, input));
        cx.notify();
    }

    fn commit_tab_rename(&mut self, cx: &mut Context<Self>) {
        let Some((i, input)) = self.pane_tab_rename.take() else { return };
        let name = input.read(cx).text().to_string();
        if let Some((_, cwd, _)) = self.tiling_scope()
            && let Some(t) = self.tiles.get_mut(&cwd)
        {
            t.rename_tab(i, &name);
        }
        cx.notify();
    }

    /// Esc while a tab is being renamed: the name stays as it was. Whether one was.
    pub(super) fn cancel_tab_rename(&mut self, cx: &mut Context<Self>) -> bool {
        let was = self.pane_tab_rename.take().is_some();
        if was {
            cx.notify();
        }
        was
    }

    /// The tab row; `None` with a single tab, when the window looks as it always has.
    pub(super) fn render_pane_tabs(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (project, cwd, _) = self.tiling_scope()?;
        let tabs = self.tiles.get(&cwd)?.tabs();
        if tabs.len() < 2 {
            return None;
        }
        let t = theme(cx).clone();
        let colour = self.project_colour(&project, &t).unwrap_or(t.accent);
        let live = self.code.get(&project)?;
        let state_of = |keys: &[Key]| {
            Lamp::most_urgent(
                keys.iter()
                    .filter_map(|k| live.sessions.iter().find(|s| s.uid == *k))
                    .map(|s| Lamp::of(s.busy, s.waiting(), s.finished)),
            )
        };
        // Agent panes roll up with the sessions.
        let states: Vec<Lamp> = tabs
            .iter()
            .map(|tab| {
                Lamp::most_urgent(
                    [state_of(&tab.sessions)].into_iter().chain(tab.panes.iter().map(|p| self.agent_lamp(*p))),
                )
            })
            .collect();
        let renaming = self.pane_tab_rename.clone();
        let row: Vec<AnyElement> =
            tabs.iter()
                .enumerate()
                .map(|(n, tab)| {
                    let name = tab.name.clone();
                    let field = renaming.as_ref().filter(|(i, _)| *i == n).map(|(_, input)| input.clone());
                    let drag = DraggedPaneTab { from: n, label: name.clone().into() };
                    let rename_name = name.clone();
                    div()
                        .id(("pane-tab", n))
                        .group("pane-tab")
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap_1p5()
                        .h(px(28.))
                        .pl_2p5()
                        .pr_1()
                        .cursor_pointer()
                        .text_sm()
                        .relative()
                        .child(row_wash(tab.active, t.hover, t.selection))
                        .when(tab.active, |d| {
                            d.child(div().absolute().left_2().right_2().bottom_0().h(px(2.)).rounded_full().bg(colour))
                        })
                        .child(lamp(("pane-tab-lamp", n), states[n], cx))
                        .child(match field {
                            Some(input) => div().w(px(120.)).child(input).into_any_element(),
                            None => div()
                                .max_w(px(180.))
                                .truncate()
                                .text_color(if tab.active { t.text } else { t.text_muted })
                                .child(name)
                                .into_any_element(),
                        })
                        .child(
                            icon_button(("pane-tab-close", n), Icon::Close, cx)
                                .tooltip(Tooltip::text("Close tab"))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.close_pane_tab(n, window, cx);
                                })),
                        )
                        .tooltip(Tooltip::text("Double-click to rename · drag to reorder · middle-click to close"))
                        .on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
                            if ev.click_count() == 2 {
                                this.start_tab_rename(n, rename_name.clone(), window, cx);
                            } else if this.pane_tab_rename.is_none() {
                                this.select_pane_tab(n, window, cx);
                            }
                        }))
                        .on_mouse_down(
                            MouseButton::Middle,
                            cx.listener(move |this, _, window, cx| this.close_pane_tab(n, window, cx)),
                        )
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(move |this, ev: &gpui::MouseDownEvent, _, cx| {
                                this.open_menu(super::menus::Menu::PaneTab(n), ev.position, gpui::Anchor::TopLeft, cx)
                            }),
                        )
                        .on_drag(drag, |d, _, _, cx| cx.new(|_| d.clone()))
                        .drag_over::<DraggedPaneTab>(move |style, _, _, _| style.bg(t.hover))
                        .on_drop(cx.listener(move |this, d: &DraggedPaneTab, _, cx| this.move_pane_tab(d.from, n, cx)))
                        .drag_over::<DraggedPane>(move |style, _, _, _| style.bg(t.hover))
                        .on_drop(cx.listener(move |this, d: &DraggedPane, window, cx| {
                            this.move_pane(d.pane, Some(n), window, cx)
                        }))
                        .into_any_element()
                })
                .collect();
        Some(
            div()
                .flex()
                .flex_none()
                .items_center()
                .border_b_1()
                .border_color(t.border_variant)
                .child(div().id("pane-tabs").flex().min_w_0().overflow_x_scroll().children(row))
                .child(
                    div()
                        .flex_none()
                        .px_1()
                        .rounded(px(4.))
                        .drag_over::<DraggedPane>(move |style, _, _, _| style.bg(t.hover))
                        .on_drop(
                            cx.listener(|this, d: &DraggedPane, window, cx| this.move_pane(d.pane, None, window, cx)),
                        )
                        .child(
                            icon_button("new-pane-tab", Icon::Plus, cx)
                                .tooltip(Tooltip::text("New tab: another layout of panes in this workspace"))
                                .on_click(cx.listener(|this, _, window, cx| this.new_pane_tab(window, cx))),
                        ),
                )
                .into_any_element(),
        )
    }
}
