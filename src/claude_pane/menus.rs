//! The composer's popup menus: model (`models.rs` draws its list), effort,
//! permission mode, and where a `#` note goes. One is open at a time, anchored
//! where it was clicked (or above the composer, from a key or a command).

use gpui::{Anchor, AnyElement, Context, Pixels, Point, SharedString, anchored, deferred, div, prelude::*};
use kynex_core::project::{self, Mode};

use super::{ClaudePane, models};
use crate::theme::theme;
use crate::ui::motion::pop;
use crate::ui::{Tooltip, menu_item, popover, px};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Menu {
    Model,
    Mode,
    Effort,
    /// Where the composer's `#` note goes: a CLAUDE.md file.
    Memory,
}

#[derive(Clone)]
pub struct OpenMenu {
    which: Menu,
    at: Point<Pixels>,
}

impl ClaudePane {
    /// Opens `which` at `at`, or closes it if it's already open.
    pub(super) fn open_menu(&mut self, which: Menu, at: Point<Pixels>, cx: &mut Context<Self>) {
        self.menu =
            if self.menu.as_ref().is_some_and(|m| m.which == which) { None } else { Some(OpenMenu { which, at }) };
        cx.notify();
    }

    /// Closes the open menu; false if none was.
    pub(super) fn close_menu(&mut self, cx: &mut Context<Self>) -> bool {
        let open = self.menu.take().is_some();
        if open {
            cx.notify();
        }
        open
    }

    /// The effort levels: only those the selected model takes, once the CLI has said which.
    fn effort_items(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let current = self.claude.effort.clone();
        let allowed = models::current_model(&self.active_model()).map(|m| m.efforts);
        let levels = project::EFFORTS
            .iter()
            .filter(|(id, _)| id.is_empty() || allowed.as_ref().is_none_or(|a| a.iter().any(|e| e == id)));
        levels
            .enumerate()
            .map(|(n, (id, name))| {
                let id = id.to_string();
                menu_item(("effort", n), *name, current == id, cx)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.menu = None;
                        this.set_effort(id.clone(), cx);
                        cx.notify();
                    }))
                    .into_any_element()
            })
            .collect()
    }

    /// Whether picks apply to the project's sessions or only this one (#136).
    /// Switching keeps the menu open; "This session" starts from the model in use.
    fn model_scope(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx).clone();
        let own = self.session_model().is_some();
        let current = self.active_model();
        let hover = t.hover;
        let scope = |n: usize, label: &'static str, on: bool, cx: &mut Context<Self>| {
            let pin = (n == 1).then(|| current.clone());
            div()
                .id(("model-scope", n))
                .flex_1()
                .flex()
                .justify_center()
                .py_0p5()
                .rounded_md()
                .text_xs()
                .cursor_pointer()
                .text_color(if on { t.text } else { t.text_muted })
                .when(on, |d| d.bg(t.active))
                .when(!on, |d| d.hover(move |s| s.bg(hover)))
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.set_session_model(pin.clone(), cx);
                }))
        };
        div()
            .flex()
            .gap_1()
            .p_0p5()
            .mb_1()
            .rounded_md()
            .border_1()
            .border_color(t.border_variant)
            .child(scope(0, "This project", !own, cx))
            .child(scope(1, "This session", own, cx))
            .into_any_element()
    }

    fn mode_items(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let current = self.session.mode;
        let bypass = self.session.can_bypass();
        Mode::choices(bypass)
            .into_iter()
            .enumerate()
            .map(|(n, m)| {
                menu_item(("mode", n), m.label(), current == m, cx)
                    .h_auto()
                    .py_1()
                    .child(div())
                    .tooltip(Tooltip::text(m.help()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.session.set_mode(m);
                        this.menu = None;
                        cx.notify();
                    }))
                    .into_any_element()
            })
            .collect()
    }

    /// The open menu.
    pub(super) fn render_menu(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let open = self.menu.as_ref()?;
        let (which, at) = (open.which, open.at);
        let items: Vec<AnyElement> = match which {
            Menu::Model => {
                let mut items = vec![self.model_scope(cx)];
                if self.session.provider().provider().is_openrouter() {
                    items.extend(self.openrouter_model_items(cx));
                } else {
                    items.extend(self.model_items(cx));
                }
                items
            }
            Menu::Mode => self.mode_items(cx),
            Menu::Effort => self.effort_items(cx),
            Menu::Memory => self.memory_items(cx),
        };
        Some(
            deferred(
                anchored().position(at).anchor(Anchor::BottomLeft).snap_to_window_with_margin(gpui::px(8.)).child(pop(
                    SharedString::from(format!("composer-menu-in-{which:?}")),
                    popover(cx)
                        .min_w(px(if which == Menu::Model { 300. } else { 200. }))
                        // Clicks stop here: the row under it mustn't take them too.
                        .occlude()
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                            this.menu = None;
                            cx.notify();
                        }))
                        .children(items),
                    // Drops down into place.
                    gpui::px(-4.),
                    None,
                )),
            )
            .into_any_element(),
        )
    }
}
