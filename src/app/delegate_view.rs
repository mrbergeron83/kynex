//! The project settings' Agents section (the toolbar's Agents button):
//! delegating on, off or as in App settings; which of Kynex's agents this
//! project uses; its own rule for what goes to them; and requests that have
//! Claude write the rule or new agents. See `kynex_core::delegate`; the agents
//! themselves are edited in Settings → Agents (`agents_settings.rs`).

use gpui::{ClipboardItem, Context, Window, div, prelude::*};
use kynex_core::delegate;
use kynex_core::toasts::Toast;

use super::KynexApp;
use super::settings::{SettingsView, heading, radio};
use crate::theme::{Palette, mono};
use crate::ui::outline_button;

/// A project rule, shown as an example of the kind of thing that helps.
const EXAMPLE: &str = "Every cargo build, test and clippy run goes to checker. Renames and the same edit across \
files go to haiku-worker, several at once when they're independent; steps marked [sonnet] in PLAN.md go to \
sonnet-worker. Keep for yourself: design, debugging, and anything touching the database migrations.";

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
        let note = |text: &'static str| div().text_xs().text_color(t.text_muted).child(text);
        // The rule this project's sessions get when it has none of its own.
        let used: Vec<&delegate::Agent> =
            view.agents.iter().filter(|a| a.on && !view.claude.agents_off.contains(&a.name)).collect();
        let made_rule = (!used.is_empty()).then(|| delegate::rule(&used));
        let (rule_ask, agents_ask) = (delegate::rule_request(&used), delegate::agents_request(&used));
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(heading(
                "Delegate to subagents",
                "Claude hands routine work to cheaper subagents and keeps design and debugging. Each subagent starts fresh, does one task and reports back in a few lines, so the main model's context stays small and your quota lasts longer. From a session's next start; Anthropic only.",
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
                        .child(div().flex_1().child(heading(
                            "Agents",
                            "The ones this project uses. Claude picks one by its description. Agents turned off in Settings → Agents aren't listed; add or change them there.",
                            t,
                        )))
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
                        .child(div().flex_1().child(heading(
                            "This project's own rule",
                            "Optional. Replaces the rule made from the agents' descriptions, so Claude knows this project's commands, files and habits. Name the commands and the kinds of work for each agent, and what Claude keeps. Shift+Enter for a new line.",
                            t,
                        )))
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
                .when(!own_rule, |d| {
                    d.child(note("For example:")).child(
                        div().px_2().text_xs().text_color(t.text_placeholder).child(EXAMPLE),
                    )
                })
                .child(
                    div().pt_3().child(heading(
                        "Let Claude write it",
                        "Claude can look around this project and draft the rule, or suggest new agents. The button puts the request in the message box (and copies it); send it, then paste the rule above, or each agent into Settings → Agents → Add agent.",
                        t,
                    )),
                )
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(outline_button("ask-rule", "Ask Claude for a rule", cx).on_click(cx.listener(
                            move |this, _, window, cx| this.ask_claude(rule_ask.clone(), window, cx),
                        )))
                        .child(outline_button("ask-agents", "Ask Claude for new agents", cx).on_click(cx.listener(
                            move |this, _, window, cx| this.ask_claude(agents_ask.clone(), window, cx),
                        ))),
                )
                .children(made_rule.filter(|_| !own_rule).map(|rule| {
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .pt_3()
                        .child(note("What Claude is told now:"))
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

    /// Copies `text` and, when the message box is empty, puts it there and
    /// closes the settings, ready to send; a draft already there is kept.
    fn ask_claude(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
        if self.pane.composer.read(cx).text().trim().is_empty() {
            self.pane.composer.update(cx, |c, cx| c.set_text(text, cx));
            self.settings = None;
            self.focus_input(window, cx);
            self.toast(Toast::info("The request is in the message box: send it, then paste the answer back here"));
        } else {
            self.toast(Toast::info("Copied the request: paste it into a session (the message box has a draft)"));
        }
        cx.notify();
    }
}
