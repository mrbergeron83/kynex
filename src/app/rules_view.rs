//! The settings modal's "Rules in effect" section: every permission rule that
//! applies to the project, from every file, a box to try a command against
//! them, and where an ask or deny rule cuts into an allow. See
//! `kynex_core::rules` for how they're read.

use gpui::{Context, FontWeight, div, prelude::*};
use kynex_core::rules::{self, Kind, Source};

use super::KynexApp;
use super::settings::{SettingsView, heading};
use crate::theme::{Palette, mono};

impl KynexApp {
    pub(super) fn render_rules_in_effect(&self, view: &SettingsView, t: &Palette, cx: &mut Context<Self>) -> gpui::Div {
        let mut all = view.files.clone();
        all.extend(rules::kynex_rules(&view.perms));
        let command = self.rule_test.read(cx).text().to_string();
        let verdict = (!command.trim().is_empty()).then(|| rules::test(&all, &command));
        let by: Vec<usize> = verdict.as_ref().map(|v| v.by.clone()).unwrap_or_default();
        let conflicts = rules::conflicts(&all);

        let result = verdict.map(|v| {
            let named =
                v.by.iter()
                    .map(|&i| format!("{} ({})", all[i].text, all[i].source.label().to_lowercase()))
                    .collect::<Vec<_>>()
                    .join(", ");
            let (colour, text) = match v.decision {
                Some(Kind::Deny) => (t.error, format!("Denied by {named}.")),
                Some(Kind::Ask) => {
                    (t.warning, format!("Asks, because of {named}. An ask rule prompts even in Auto mode."))
                }
                Some(Kind::Allow) => (t.success, format!("Allowed by {named}.")),
                None => (
                    t.text_muted,
                    "No rule decides. The permission mode does; in Auto mode Claude's own check judges it.".into(),
                ),
            };
            div().text_sm().text_color(colour).child(text)
        });

        let by_source = Source::ALL.into_iter().filter_map(|source| {
            let mine: Vec<(usize, &rules::Rule)> = all.iter().enumerate().filter(|(_, r)| r.source == source).collect();
            if mine.is_empty() {
                return None;
            }
            Some(
                div()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .child(
                        div()
                            .flex()
                            .items_baseline()
                            .gap_2()
                            .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(source.label()))
                            .child(div().text_xs().text_color(t.text_placeholder).child(source.file())),
                    )
                    .children(mine.into_iter().map(|(i, r)| {
                        let colour = match r.kind {
                            Kind::Allow => t.success,
                            Kind::Ask => t.warning,
                            Kind::Deny => t.error,
                        };
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .pl_2()
                            .rounded_md()
                            .when(by.contains(&i), |d| d.bg(t.active))
                            .child(div().w(crate::ui::px(40.)).text_xs().text_color(colour).child(r.kind.label()))
                            .child(
                                div().flex_1().min_w_0().truncate().font_family(mono()).text_sm().child(r.text.clone()),
                            )
                    })),
            )
        });

        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(heading(
                        "Try a command",
                        "Type what Claude would run. It's judged one part at a time, so a chain with `git push` in it asks if any rule asks about a push.",
                        t,
                    ))
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .border_1()
                            .border_color(t.border)
                            .bg(t.editor)
                            .text_sm()
                            .child(self.rule_test.clone()),
                    )
                    .children(result),
            )
            .when(!conflicts.is_empty(), |d| {
                d.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(heading(
                            "Rules that cut into each other",
                            "An ask or deny wins over an allow for the commands they share.",
                            t,
                        ))
                        .children(
                            conflicts.into_iter().map(|c| div().pl_2().text_xs().text_color(t.warning).child(c)),
                        ),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(heading(
                        "Every rule that applies",
                        "From your user settings, the repo and Kynex. A deny beats an ask, which beats an allow. Kynex's own are edited under Permissions; the files are yours to edit.",
                        t,
                    ))
                    .children(by_source),
            )
            .children(view.file_errors.iter().map(|e| div().text_xs().text_color(t.error).child(e.clone())))
            .child(
                div().text_xs().text_color(t.text_placeholder).child(
                    "Kynex's reading of the rules, close to the CLI's but not its own matcher. Only shell rules (Bash, PowerShell) are tried here.",
                ),
            )
    }
}
