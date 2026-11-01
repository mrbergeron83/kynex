//! The settings modal's "Setup" section, which a project opens on the first
//! time: is the folder trusted, what mode do sessions start in, which ask
//! rules will prompt even in Auto mode, and starter rules for what's in the
//! folder (git, Cargo, npm...) to allow in one click.

use gpui::{AnyElement, Context, FontWeight, SharedString, div, prelude::*};
use kynex_core::rules::{self, Kind};
use kynex_core::starters;

use super::KynexApp;
use super::settings::{SettingsView, heading};
use crate::theme::{Palette, mono};
use crate::ui::{Icon, Tooltip, icon, outline_button};

/// One line of the checklist: a mark, what it says, a note under it, and
/// maybe a button.
fn row(
    mark: Icon,
    colour: gpui::Hsla,
    title: String,
    notes: Vec<String>,
    action: Option<AnyElement>,
    t: &Palette,
) -> gpui::Div {
    div()
        .flex()
        .items_start()
        .gap_2()
        .child(div().flex_none().pt(crate::ui::px(3.)).child(icon(mark, colour).size_3p5()))
        .child(
            div().flex_1().min_w_0().flex().flex_col().gap_0p5().child(div().text_sm().child(title)).children(
                notes.into_iter().map(|n| div().text_xs().text_color(t.text_muted).font_family(mono()).child(n)),
            ),
        )
        .children(action)
}

impl KynexApp {
    pub(super) fn render_project_setup(&self, view: &SettingsView, t: &Palette, cx: &mut Context<Self>) -> gpui::Div {
        let mut all = view.files.clone();
        all.extend(rules::kynex_rules(&view.perms));
        let mut list = div().flex().flex_col().gap_3();

        // Trust: the CLI ignores a folder's own settings until it's trusted.
        list = list.child(match self.untrusted_files(&view.path) {
            Some(files) if !files.is_empty() => {
                let dir = view.path.clone();
                let button = outline_button("setup-trust", "Trust folder", cx)
                    .tooltip(Tooltip::text(
                        "Only if you trust its files: its settings and MCP servers can run commands",
                    ))
                    .on_click(cx.listener(move |this, _, _, cx| this.trust_folder(dir.clone(), cx)))
                    .into_any_element();
                row(
                    Icon::Shield,
                    t.warning,
                    format!("Claude Code ignores this folder's {} until you trust it", files.join(", ")),
                    Vec::new(),
                    Some(button),
                    t,
                )
            }
            Some(_) => row(
                Icon::Check,
                t.success,
                "Trusted, or nothing in this folder needs trust".into(),
                Vec::new(),
                None,
                t,
            ),
            None => row(
                Icon::Dot,
                t.text_muted,
                "Checking whether Claude Code trusts this folder".into(),
                Vec::new(),
                None,
                t,
            ),
        });

        // The mode new sessions start in.
        list = list.child(row(
            Icon::Check,
            t.success,
            format!("New sessions start in {} mode", view.claude.mode.label()),
            vec![format!("{} Change it under Permissions.", view.claude.mode.help())],
            None,
            t,
        ));

        // Ask rules prompt even in Auto mode.
        let asks: Vec<&rules::Rule> = all.iter().filter(|r| r.kind == Kind::Ask).collect();
        list = list.child(if asks.is_empty() {
            row(
                Icon::Check,
                t.success,
                "No ask rules, so nothing prompts that a mode or allow rule covers".into(),
                Vec::new(),
                None,
                t,
            )
        } else {
            row(
                Icon::Shield,
                t.warning,
                format!(
                    "{} ask rule{} will prompt, even in Auto mode",
                    asks.len(),
                    if asks.len() == 1 { "" } else { "s" }
                ),
                asks.iter().map(|r| format!("{}  ({})", r.text, r.source.file())).collect(),
                None,
                t,
            )
        });

        // Starter rules for what's in the folder.
        let starters: Vec<(usize, &starters::Starter, Vec<String>)> =
            view.starters.iter().enumerate().map(|(i, s)| (i, s, starters::missing(s, &all))).collect();
        let offers = starters.iter().any(|(_, _, m)| !m.is_empty());
        let mut starter_rows = div().flex().flex_col().gap_3();
        for (i, starter, missing) in starters {
            starter_rows = starter_rows.child(if missing.is_empty() {
                row(Icon::Check, t.success, format!("{}: already allowed", starter.label), Vec::new(), None, t)
            } else {
                let n = missing.len();
                let button = outline_button(("add-starter", i), format!("Allow {n}"), cx)
                    .tooltip(Tooltip::text("Adds these to Kynex's rules for this project"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let rules = missing.clone();
                        this.edit_settings(cx, move |v| {
                            v.perms.allow(rules);
                        })
                    }))
                    .into_any_element();
                let shown = starter.rules.iter().filter(|r| r.starts_with("Bash(")).cloned().collect();
                row(
                    Icon::Dot,
                    t.text_muted,
                    format!("{} (found {})", starter.label, starter.found),
                    shown,
                    Some(button),
                    t,
                )
            });
        }

        div()
            .flex()
            .flex_col()
            .gap_5()
            .child(
                div().flex().flex_col().gap_2().child(heading(
                    "Is this project ready?",
                    "What to check so Claude works here without surprises.",
                    t,
                ))
                .child(list),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(heading(
                        "Starter rules",
                        "Everyday commands Claude would otherwise ask about, allowed for this project. Never a push, a publish or a delete.",
                        t,
                    ))
                    .child(starter_rows)
                    .when(view.starters.is_empty(), |d| {
                        d.child(
                            div()
                                .text_sm()
                                .text_color(t.text_placeholder)
                                .child("Nothing recognised in this folder (no git, Cargo, npm, Python, Go or .NET)."),
                        )
                    })
                    .when(!view.starters.is_empty() && !offers, |d| {
                        d.child(div().text_xs().text_color(t.text_muted).child("Every starter rule is already allowed."))
                    }),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(t.text_placeholder)
                    .font_weight(FontWeight::NORMAL)
                    .child(SharedString::from("Try any command against the rules under Rules in effect.")),
            )
    }
}
