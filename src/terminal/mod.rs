//! The terminal panel under the conversation: tabs of shells running in the
//! project folder, a "new terminal" menu, and an Administrator badge when
//! Kynex (and so every shell it starts) runs elevated.
//!
//! | file         | what                                                     |
//! |--------------|----------------------------------------------------------|
//! | `mod.rs`     | `TerminalPanel`: tabs, running a command in the right one |
//! | `view.rs`    | `TerminalView`: one shell, keyboard, mouse, clipboard     |
//! | `element.rs` | drawing the grid on the GPU, sizing it, colours          |
//!
//! The terminal itself (PTY, parser, shells) is `kynex_core::terminal`.

mod element;
mod view;

use std::path::PathBuf;

use crate::ui::px;
use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, FontWeight, SharedString, Subscription,
    Window, anchored, deferred, div, prelude::*,
};
use kynex_core::terminal::{self as core, ShellKind};

pub use self::view::bind_keys;
use self::view::{TerminalEvent, TerminalView};
use crate::theme::theme;
use crate::ui::{Icon, Tooltip, icon, icon_button, menu_item, popover};

pub enum PanelEvent {
    /// The user closed the panel.
    Hide,
    /// Every shell exited, or one came back: the dock changes height.
    Collapsed,
}

/// The dock's height while every shell has exited.
pub const STRIP_HEIGHT: f32 = 28.;

impl EventEmitter<PanelEvent> for TerminalPanel {}

struct Tab {
    view: Entity<TerminalView>,
    _sub: Subscription,
}

pub struct TerminalPanel {
    cwd: PathBuf,
    tabs: Vec<Tab>,
    active: usize,
    new_menu: bool,
    /// Whether shells run as administrator; `None` until asked.
    elevated: Option<bool>,
    error: Option<String>,
    focus: FocusHandle,
    /// Last said through `PanelEvent::Collapsed`.
    was_collapsed: bool,
}

impl TerminalPanel {
    pub fn new(cwd: PathBuf, cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this, cx| {
            let elevated = cx.background_spawn(async { core::elevated() }).await;
            this.update(cx, |this, cx| {
                this.elevated = Some(elevated);
                cx.notify();
            })
            .ok();
        })
        .detach();
        Self {
            cwd,
            tabs: Vec::new(),
            active: 0,
            new_menu: false,
            elevated: None,
            error: None,
            focus: cx.focus_handle(),
            was_collapsed: false,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }

    /// Every shell has exited: the dock is just a strip until one restarts.
    pub fn collapsed(&self, cx: &App) -> bool {
        !self.tabs.is_empty() && self.tabs.iter().all(|t| t.view.read(cx).exited())
    }

    /// Tells the app when `collapsed` changes, so the dock takes its new height.
    fn check_collapsed(&mut self, cx: &mut Context<Self>) {
        let now = self.collapsed(cx);
        if now != self.was_collapsed {
            self.was_collapsed = now;
            cx.emit(PanelEvent::Collapsed);
        }
    }

    /// Starts the shown tab's shell again, at the dock's full height.
    fn restart_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(t) = self.tabs.get(self.active) {
            t.view.update(cx, |v, cx| v.restart(cx));
            self.focus_active(window, cx);
        }
        self.check_collapsed(cx);
        cx.notify();
    }

    /// Opens a new shell of `kind` and shows it.
    pub fn open(
        &mut self,
        kind: ShellKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<TerminalView>> {
        self.new_menu = false;
        let shell = match core::resolve_shell(kind) {
            Ok(s) => s,
            Err(e) => {
                self.error = Some(e);
                cx.notify();
                return None;
            }
        };
        self.error = None;
        let cwd = self.cwd.clone();
        let view = cx.new(|cx| TerminalView::new(shell, cwd, cx));
        let sub = cx.subscribe(&view, |this, _, ev: &TerminalEvent, cx| match ev {
            TerminalEvent::Changed => {
                this.check_collapsed(cx);
                cx.notify();
            }
        });
        self.tabs.push(Tab { view: view.clone(), _sub: sub });
        self.active = self.tabs.len() - 1;
        view.read(cx).focus_handle(cx).focus(window, cx);
        self.check_collapsed(cx);
        cx.notify();
        Some(view)
    }

    /// Makes sure there's a shell to look at.
    pub fn ensure_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.tabs.is_empty() {
            self.open(ShellKind::default_shell(), window, cx);
        }
    }

    /// Runs `command` in a shell of `kind`: the active tab if it is one, else
    /// another open one, else a new one.
    pub fn run(&mut self, kind: ShellKind, command: &str, window: &mut Window, cx: &mut Context<Self>) {
        let is_kind = |t: &Tab, cx: &App| t.view.read(cx).shell.kind == kind;
        let target = if self.tabs.get(self.active).is_some_and(|t| is_kind(t, cx)) {
            Some(self.active)
        } else {
            self.tabs.iter().rposition(|t| is_kind(t, cx))
        };
        let view = match target {
            Some(i) => {
                self.active = i;
                self.tabs[i].view.clone()
            }
            None => match self.open(kind, window, cx) {
                Some(v) => v,
                None => return,
            },
        };
        view.update(cx, |v, cx| v.run(command, cx));
        view.read(cx).focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    pub fn focus_active(&self, window: &mut Window, cx: &mut App) {
        if let Some(t) = self.tabs.get(self.active) {
            t.view.read(cx).focus_handle(cx).focus(window, cx);
        }
    }

    fn close(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        if i >= self.tabs.len() {
            return;
        }
        self.tabs.remove(i);
        // A tab closed to the left shifts the shown one down; keep showing it.
        if i < self.active {
            self.active -= 1;
        }
        self.active = self.active.min(self.tabs.len().saturating_sub(1));
        if self.tabs.is_empty() {
            cx.emit(PanelEvent::Hide);
        } else {
            self.focus_active(window, cx);
        }
        self.check_collapsed(cx);
        cx.notify();
    }

    /// The dock with every shell exited: one line to restart from, or to open
    /// another. Enter or a click restarts the shown one.
    fn render_strip(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx).clone();
        let view = self.tabs[self.active].view.read(cx);
        let (label, focus) = (view.label(), view.focus_handle(cx));
        let hover = t.hover;
        let focused = focus.contains_focused(window, cx);
        div()
            .id("terminal-strip")
            .track_focus(&focus)
            .flex()
            .items_center()
            .gap_2()
            .size_full()
            .px_2()
            .bg(element::background(&t))
            .text_xs()
            .text_color(t.text_muted)
            .cursor_pointer()
            .hover(move |s| s.bg(hover))
            .child(icon(Icon::Terminal, if focused { t.accent } else { t.text_muted }).size_3())
            .child(div().text_color(t.text).child(label))
            .child(div().text_color(t.text_placeholder).child("exited · Enter to restart"))
            .child(div().flex_1())
            .child(
                div()
                    .relative()
                    .child(
                        icon_button("strip-new-terminal", Icon::Plus, cx)
                            .tooltip(Tooltip::text("New terminal"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                this.new_menu = !this.new_menu;
                                cx.notify();
                            })),
                    )
                    .when(self.new_menu, |d| d.child(self.new_menu(cx))),
            )
            .child(
                icon_button("strip-hide-terminal", Icon::ChevronDown, cx)
                    .tooltip(Tooltip::text("Hide the terminal"))
                    .on_click(cx.listener(|_, _, _, cx| {
                        cx.stop_propagation();
                        cx.emit(PanelEvent::Hide);
                    })),
            )
            .tooltip(Tooltip::text("Restart the shell"))
            .on_click(cx.listener(|this, _, window, cx| this.restart_active(window, cx)))
            .on_key_down(cx.listener(|this, ev: &gpui::KeyDownEvent, window, cx| {
                if ev.keystroke.key == "enter" {
                    cx.stop_propagation();
                    this.restart_active(window, cx);
                }
            }))
            .into_any_element()
    }

    fn new_menu(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let kinds = [ShellKind::PowerShell, ShellKind::Bash];
        div()
            .absolute()
            .top(px(28.))
            .right_0()
            .child(deferred(
                anchored().snap_to_window_with_margin(gpui::px(8.)).child(
                    popover(cx)
                        .min_w(px(180.))
                        .occlude()
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                            this.new_menu = false;
                            cx.notify();
                        }))
                        .children(kinds.into_iter().enumerate().map(|(n, kind)| {
                            menu_item(("new-terminal", n), kind.label(), false, cx).on_click(cx.listener(
                                move |this, _, window, cx| {
                                    this.open(kind, window, cx);
                                },
                            ))
                        })),
                ),
            ))
            .into_any_element()
    }
}

impl Focusable for TerminalPanel {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.tabs.get(self.active).map_or_else(|| self.focus.clone(), |t| t.view.read(cx).focus_handle(cx))
    }
}

impl Render for TerminalPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.collapsed(cx) {
            return self.render_strip(window, cx);
        }
        let t = theme(cx).clone();
        let tabs: Vec<AnyElement> = self
            .tabs
            .iter()
            .enumerate()
            .map(|(i, tab)| {
                let v = tab.view.read(cx);
                let active = i == self.active;
                let exited = v.exited();
                let hover = t.hover;
                let focused = active && v.focus_handle(cx).contains_focused(window, cx);
                div()
                    .id(("terminal-tab", i))
                    .group("terminal-tab")
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .h(px(26.))
                    .pl_2()
                    .pr_1()
                    .rounded_md()
                    .text_xs()
                    .cursor_pointer()
                    .text_color(if active { t.text } else { t.text_muted })
                    .when(active, |d| d.bg(t.active))
                    .when(!active, |d| d.hover(move |s| s.bg(hover)))
                    .child(icon(Icon::Terminal, if focused { t.accent } else { t.text_muted }).size_3())
                    .child(div().max_w(px(160.)).truncate().child(v.label()))
                    .when(exited, |d| d.child(div().text_color(t.text_placeholder).child("exited")))
                    .child(
                        icon_button(("close-terminal", i), Icon::Close, cx)
                            .size(px(18.))
                            .invisible()
                            .group_hover("terminal-tab", |s| s.visible())
                            .when(active, |d| d.visible())
                            .tooltip(Tooltip::text("Close (ends the shell)"))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.close(i, window, cx);
                            })),
                    )
                    .when_some(v.title(), |d, title| d.tooltip(Tooltip::text(title)))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.active = i;
                        this.focus_active(window, cx);
                        cx.notify();
                    }))
                    .into_any_element()
            })
            .collect();

        let admin = self.elevated.unwrap_or(false).then(|| {
            div()
                .id("terminal-admin")
                .flex()
                .items_center()
                .gap_1()
                .px_1p5()
                .h(px(20.))
                .rounded_sm()
                .border_1()
                .border_color(t.warning.opacity(0.5))
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .text_color(t.warning)
                .child(icon(Icon::Shield, t.warning).size_3())
                .child("Administrator")
                .tooltip(Tooltip::text("Kynex runs as administrator, so these shells do too"))
        });

        let header = div()
            .flex()
            .flex_none()
            .items_center()
            .gap_1()
            .h(px(34.))
            .px_2()
            .border_b_1()
            .border_color(t.border_variant)
            .child(div().flex().items_center().gap_0p5().min_w_0().overflow_hidden().children(tabs))
            .child(
                div()
                    .relative()
                    .child(icon_button("new-terminal", Icon::Plus, cx).tooltip(Tooltip::text("New terminal")).on_click(
                        cx.listener(|this, _, _, cx| {
                            this.new_menu = !this.new_menu;
                            cx.notify();
                        }),
                    ))
                    .when(self.new_menu, |d| d.child(self.new_menu(cx))),
            )
            .child(div().flex_1())
            .when_some(self.error.clone(), |d, e| {
                d.child(div().min_w_0().truncate().text_xs().text_color(t.error).child(SharedString::from(e)))
            })
            .children(admin)
            .child(
                icon_button("hide-terminal", Icon::ChevronDown, cx)
                    .tooltip(Tooltip::for_action(
                        "Hide the terminal",
                        crate::app::ToggleTerminal,
                        "shells keep running",
                    ))
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(PanelEvent::Hide))),
            );

        let body = match self.tabs.get(self.active) {
            Some(tab) => tab.view.clone().into_any_element(),
            None => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(t.text_placeholder)
                .child("No terminal open")
                .into_any_element(),
        };

        div()
            .track_focus(&self.focus)
            .flex()
            .flex_col()
            .size_full()
            .bg(element::background(&t))
            .child(header)
            .child(div().flex_1().min_h_0().child(body))
            .into_any_element()
    }
}
