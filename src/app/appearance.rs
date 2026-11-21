//! Each project's colour, to tell at a glance which one you're in: it tints the
//! title bar while the project is shown and marks its tab. Picked (or turned off)
//! from the project tab's right-click menu; automatic otherwise.

use std::path::{Path, PathBuf};

use gpui::{AnyElement, Context, Div, Hsla, Pixels, div, prelude::*};
use kynex_core::project::{Appearance, COLOURS, Colour};
use kynex_core::toasts::Toast;

use super::KynexApp;
use crate::theme::{Palette, theme};
use crate::ui::{Tooltip, menu_item};

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

impl KynexApp {
    /// Reads the appearance of projects opened since the last render; once each.
    pub(super) fn load_project_settings(&mut self) {
        for p in &self.saved.projects {
            if !self.appearances.contains_key(&p.path) {
                self.appearances.insert(p.path.clone(), Appearance::load(&p.path));
            }
        }
    }

    /// A project's colour on this palette, or `None` when it has none.
    pub(super) fn project_colour(&self, path: &Path, t: &Palette) -> Option<Hsla> {
        let default = Appearance::default();
        let name = self.appearances.get(path).unwrap_or(&default).colour(path);
        name.map(|n| t.project_colour(n))
    }

    /// The shown project's colour.
    pub(super) fn active_colour(&self, t: &Palette) -> Option<Hsla> {
        self.active_project().and_then(|p| self.project_colour(&p.path, t))
    }

    fn colour_swatches(&self, id: &'static str, path: &Path, size: Pixels, t: &Palette, cx: &mut Context<Self>) -> Div {
        let current = self.appearances.get(path).map(|a| a.colour.clone()).unwrap_or_default();
        div().flex().gap_0p5().children(COLOURS.iter().enumerate().map(|(n, name)| {
            let selected = current == Colour::Named(name.to_string());
            let path = path.to_path_buf();
            div()
                .id((id, n))
                .size(size)
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .border_2()
                .border_color(if selected { t.text } else { gpui::transparent_black() })
                .cursor_pointer()
                .child(div().size(size * 0.7).rounded_full().bg(t.project_colour(name)))
                .tooltip(Tooltip::text(capitalize(name)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.set_project_colour(path.clone(), Colour::Named(name.to_string()), cx)
                }))
        }))
    }

    /// The colour part of a project tab's right-click menu.
    pub(super) fn tab_colour_items(&self, path: &Path, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let t = theme(cx).clone();
        let current = self.appearances.get(path).map(|a| a.colour.clone()).unwrap_or_default();
        let mut items = vec![
            div().px_2().pt_1().text_xs().text_color(t.text_muted).child("Colour").into_any_element(),
            div()
                .px_1()
                .py_1()
                .child(self.colour_swatches("tab-colour", path, gpui::px(22.), &t, cx))
                .into_any_element(),
        ];
        for (n, (label, choice)) in [("Automatic", Colour::Auto), ("No colour", Colour::Off)].into_iter().enumerate() {
            let path = path.to_path_buf();
            items.push(
                menu_item(("tab-colour-mode", n), label, current == choice, cx)
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.set_project_colour(path.clone(), choice.clone(), cx)),
                    )
                    .into_any_element(),
            );
        }
        items
    }

    pub(super) fn set_project_colour(&mut self, path: PathBuf, colour: Colour, cx: &mut Context<Self>) {
        let appearance = Appearance { colour };
        if let Err(e) = appearance.save(&path) {
            self.toast(Toast::error(e));
        }
        self.appearances.insert(path, appearance);
        cx.notify();
    }
}
