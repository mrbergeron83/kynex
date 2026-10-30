//! The Extensions modal's read-only tabs: skills and slash commands, agents,
//! and hooks, each grouped by where it's defined (you, the project, a plugin).

use gpui::{AnyElement, FontWeight, div, prelude::*};
use kynex_core::catalog::{Hook, Item, Kind, Origin};

use super::KynexApp;
use super::extensions::{card, chip};
use super::settings::heading;
use crate::theme::{Palette, mono};

impl KynexApp {
    pub(super) fn render_skills(&self, t: &Palette) -> gpui::Div {
        let items = self.extensions.iter().flat_map(|v| v.extra.iter()).flat_map(|e| &e.items);
        section(
            "Skills & commands",
            "Skills Claude uses when they fit the task, and slash commands you type. Built-in commands aren't listed.",
            "No skills or commands of your own, the project's or from plugins.",
            groups(items.collect(), |i| &i.origin)
                .into_iter()
                .map(|(o, items)| group(&o, items.into_iter().map(|i| item_row(i, t)), t)),
            t,
        )
    }

    pub(super) fn render_agents(&self, t: &Palette) -> gpui::Div {
        let items = self.extensions.iter().flat_map(|v| v.extra.iter()).flat_map(|e| &e.agents);
        section(
            "Agents",
            "Subagents Claude can hand work to, each with its own instructions and tools.",
            "No agents of your own, the project's or from plugins.",
            groups(items.collect(), |i| &i.origin)
                .into_iter()
                .map(|(o, items)| group(&o, items.into_iter().map(|i| item_row(i, t)), t)),
            t,
        )
    }

    pub(super) fn render_hooks(&self, t: &Palette) -> gpui::Div {
        let hooks = self.extensions.iter().flat_map(|v| v.extra.iter()).flat_map(|e| &e.hooks);
        section(
            "Hooks",
            "Commands Claude Code runs itself on events: before or after a tool, when a turn ends, and so on.",
            "No hooks are set for this folder.",
            groups(hooks.collect(), |h| &h.origin)
                .into_iter()
                .map(|(o, hooks)| group(&o, hooks.into_iter().map(|h| hook_row(h, t)), t)),
            t,
        )
    }
}

fn section(
    title: &'static str,
    help: &'static str,
    empty: &'static str,
    groups: impl Iterator<Item = AnyElement>,
    t: &Palette,
) -> gpui::Div {
    let groups: Vec<AnyElement> = groups.collect();
    div()
        .flex()
        .flex_col()
        .gap_4()
        .child(heading(title, help, t))
        .when(groups.is_empty(), |d| d.child(div().text_sm().text_color(t.text_muted).child(empty)))
        .children(groups)
}

/// `items` by origin, in the order origins first appear.
fn groups<T>(items: Vec<&T>, origin: fn(&T) -> &Origin) -> Vec<(Origin, Vec<&T>)> {
    let mut out: Vec<(Origin, Vec<&T>)> = Vec::new();
    for i in items {
        match out.iter_mut().find(|(o, _)| o == origin(i)) {
            Some((_, list)) => list.push(i),
            None => out.push((origin(i).clone(), vec![i])),
        }
    }
    out
}

fn group(origin: &Origin, rows: impl Iterator<Item = gpui::Div>, t: &Palette) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(div().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(t.text_muted).child(origin.label()))
        .children(rows)
        .into_any_element()
}

fn item_row(i: &Item, t: &Palette) -> gpui::Div {
    let (name, kind) = match i.kind {
        Kind::Command => (format!("/{}", i.name), "command"),
        Kind::Skill => (i.name.clone(), "skill"),
        Kind::Agent => (i.name.clone(), "agent"),
    };
    card(t)
        .items_baseline()
        .child(div().flex_none().text_sm().font_family(mono()).child(name))
        .when(i.kind != Kind::Agent, |d| d.child(div().text_xs().child(chip(kind, t))))
        .child(div().flex_1().min_w_0().text_xs().text_color(t.text_muted).truncate().child(i.description.clone()))
}

fn hook_row(h: &Hook, t: &Palette) -> gpui::Div {
    card(t)
        .flex_col()
        .gap_0p5()
        .child(
            div()
                .flex()
                .items_baseline()
                .gap_2()
                .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(h.event.clone()))
                .when(!h.matcher.is_empty(), |d| {
                    d.child(div().text_xs().text_color(t.text_muted).child(format!("for {}", h.matcher)))
                }),
        )
        .child(
            div().text_xs().font_family(mono()).text_color(t.text_muted).whitespace_normal().child(h.command.clone()),
        )
}
