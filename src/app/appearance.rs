//! Each project's colour, to tell at a glance which one you're in: it tints the
//! title bar while the project is shown, marks its tab, and edges the composer.
//! Picked (or turned off) in the project's settings; automatic otherwise.

use std::path::{Path, PathBuf};

use gpui::{Context, Div, Hsla, div, prelude::*, px};
use kynex_core::project::{self, Appearance, COLOURS, Colour};

use super::KynexApp;
use super::settings::heading;
use crate::theme::Palette;
use crate::ui::Tooltip;

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

impl KynexApp {
    /// Reads the appearance of projects opened since the last render; once each.
    pub(super) fn load_appearances(&mut self) {
        for p in &self.saved.projects {
            if !self.appearances.contains_key(&p.path) {
                self.appearances.insert(p.path.clone(), Appearance::load(&p.path));
            }
        }
    }

    /// A project's colour on this palette, or `None` when it has none.
    pub(super) fn project_colour(&self, path: &Path, t: &Palette) -> Option<Hsla> {
        let name = match self.appearances.get(path) {
            Some(a) => a.colour(path),
            None => Appearance::default().colour(path),
        };
        name.map(|n| t.project_colour(n))
    }

    /// The shown project's colour; `None` in Chat.
    pub(super) fn active_colour(&self, t: &Palette) -> Option<Hsla> {
        self.active_project().and_then(|p| self.project_colour(&p.path, t))
    }

    /// The Appearance section of project settings: automatic, a swatch, or none.
    pub(super) fn render_colour_picker(&self, path: &Path, t: &Palette, cx: &mut Context<Self>) -> Div {
        let current = self.appearances.get(path).map(|a| a.colour.clone()).unwrap_or_default();
        let auto = t.project_colour(project::auto_colour(path));
        let option =
            |id: &'static str, label: &'static str, swatch: Option<Hsla>, choice: Colour, cx: &mut Context<Self>| {
                let selected = current == choice;
                let (hover, path) = (t.hover, path.to_path_buf());
                div()
                    .id(id)
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .h(px(28.))
                    .px_2()
                    .rounded_md()
                    .border_1()
                    .border_color(if selected { t.accent } else { t.border })
                    .text_sm()
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover))
                    .child(match swatch {
                        Some(c) => div().size(px(12.)).rounded_full().bg(c),
                        None => div().size(px(12.)).rounded_full().border_1().border_color(t.text_placeholder),
                    })
                    .child(label)
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.set_project_colour(path.clone(), choice.clone(), cx)),
                    )
            };
        let row = div()
            .flex()
            .gap_2()
            .child(option("colour-auto", "Automatic", Some(auto), Colour::Auto, cx))
            .child(option("colour-off", "None", None, Colour::Off, cx));
        let swatches = COLOURS.iter().enumerate().map(|(n, name)| {
            let selected = current == Colour::Named(name.to_string());
            let path = path.to_path_buf();
            div()
                .id(("project-colour", n))
                .size(px(28.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .border_2()
                .border_color(if selected { t.text } else { gpui::transparent_black() })
                .cursor_pointer()
                .child(div().size(px(20.)).rounded_full().bg(t.project_colour(name)))
                .tooltip(Tooltip::text(capitalize(name)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.set_project_colour(path.clone(), Colour::Named(name.to_string()), cx)
                }))
        });
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(heading(
                "Project colour",
                "Tints the title bar and marks the tab and composer, so you can tell projects apart at a glance.",
                t,
            ))
            .child(row)
            .child(div().flex().gap_1().children(swatches))
    }

    /// Sets a project's colour and saves it.
    pub(super) fn set_project_colour(&mut self, path: PathBuf, colour: Colour, cx: &mut Context<Self>) {
        let appearance = Appearance { colour };
        if let Err(e) = appearance.save(&path) {
            self.notice = Some(e);
        }
        self.appearances.insert(path, appearance);
        cx.notify();
    }
}
