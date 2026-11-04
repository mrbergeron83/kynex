//! The project settings' "Delegate to subagents": on, off or as in App
//! settings; which of Kynex's agents this project uses; and its own rule for
//! what goes to them. See `kynex_core::delegate`; the agents themselves are
//! edited in Settings → Agents (`agents_settings.rs`).

use gpui::{Context, div, prelude::*};
use kynex_core::delegate;

use super::KynexApp;
use super::settings::{SettingsView, heading, radio};
use crate::theme::{Palette, mono};
use crate::ui::outline_button;

impl KynexApp {
    pub(super) fn render_delegate_settings(
        &self,
        view: &SettingsView,
        t: &Palette,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let app = if delegate::default_on() { "On" } else { "Off" };
        let choices =
            [(None, format!("As in App settings ({app})")), (Some(true), "On".into()), (Some(false), "Off".into())];
        let on = delegate::enabled(view.claude.delegate, delegate::default_on());
        let own_rule = !view.claude.delegate_rule.is_empty();
        let hover = t.hover;
        let row = move |id: (&'static str, usize)| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap_2()
                .px_2()
                .py_1()
                .rounded_md()
                .cursor_pointer()
                .hover(move |s| s.bg(hover))
        };
        // The rule this project's sessions get when it has none of its own.
        let used: Vec<&delegate::Agent> =
            view.agents.iter().filter(|a| a.on && !view.claude.agents_off.contains(&a.name)).collect();
        let made_rule = (!used.is_empty()).then(|| delegate::rule(&used));
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(heading(
                "Delegate to subagents",
                "Whether sessions hand contained work, build and test runs and web lookups to cheaper subagents and keep design and debugging. From a session's next start; Anthropic only.",
                t,
            ))
            .children(choices.into_iter().enumerate().map(|(n, (choice, label))| {
                row(("delegate", n))
                    .child(radio(view.claude.delegate == choice, t))
                    .child(div().text_sm().child(label))
                    .on_click(cx.listener(move |this, _, _, cx| this.edit_settings(cx, |v| v.claude.delegate = choice)))
            }))
            .when(on, |d| {
                d.child(
                    div()
                        .pt_3()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(div().flex_1().text_xs().text_color(t.text_muted).child(
                            "Agents this project uses. Ones turned off in Settings → Agents aren't listed.",
                        ))
                        .child(outline_button("edit-agents", "Edit agents…", cx).on_click(cx.listener(
                            |this, _, _, cx| this.open_settings_on(super::app_settings::AGENTS_SECTION, cx),
                        ))),
                )
                .children(view.agents.iter().enumerate().filter(|(_, a)| a.on).map(|(n, a)| {
                    let name = a.name.clone();
                    let used = !view.claude.agents_off.contains(&a.name);
                    row(("project-agent", n))
                        .child(radio(used, t))
                        .child(div().flex_none().font_family(mono()).text_sm().child(a.name.clone()))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(t.text_placeholder)
                                .child(a.description.clone()),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let name = name.clone();
                            this.edit_settings(cx, |v| {
                                let off = &mut v.claude.agents_off;
                                match off.iter().position(|o| *o == name) {
                                    Some(i) => {
                                        off.remove(i);
                                    }
                                    None => off.push(name),
                                }
                            })
                        }))
                }))
                .child(
                    div()
                        .pt_3()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(div().flex_1().text_xs().text_color(t.text_muted).child(
                            "This project's own rule, instead of the one made from the agents (Shift+Enter for a new line):",
                        ))
                        .when(own_rule, |d| {
                            d.child(outline_button("delegate-reset", "Clear", cx).on_click(cx.listener(
                                |this, _, _, cx| {
                                    this.delegate_rule.update(cx, |i, cx| i.set_text("", cx));
                                },
                            )))
                        }),
                )
                .child(
                    div()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .border_1()
                        .border_color(t.border)
                        .bg(t.editor)
                        .text_xs()
                        .child(self.delegate_rule.clone()),
                )
                .children(made_rule.filter(|_| !own_rule).map(|rule| {
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .pt_1()
                        .child(div().text_xs().text_color(t.text_muted).child("What Claude is told:"))
                        .child(
                            div()
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .border_1()
                                .border_color(t.border_variant)
                                .text_xs()
                                .text_color(t.text_muted)
                                .child(rule),
                        )
                }))
            })
    }
}
