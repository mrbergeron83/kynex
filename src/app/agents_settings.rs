//! Settings → Agents: delegating on or off, and Kynex's subagents
//! (`kynex_core::delegate`): turn each on or off, add, edit, remove, or go back
//! to the defaults. They're Kynex's own, in its data folder, and reach each
//! session on the command line; nothing goes into the user's `.claude` folders.

use gpui::{Context, Entity, div, prelude::*};
use kynex_core::delegate::{self, Agent};
use kynex_core::toasts::Toast;

use super::KynexApp;
use super::app_settings::choice;
use super::settings::{heading, radio};
use crate::theme::{Palette, mono};
use crate::ui::input::TextInput;
use crate::ui::{Icon, Tooltip, icon_button, outline_button, primary_button};

/// More agents than this and the tab warns: each description goes into every session.
const MANY: usize = 6;

/// The Agents section's state, made when Settings opens.
#[derive(Clone)]
pub struct AgentsView {
    list: Vec<Agent>,
    /// The form, while open: `Some(i)` edits agent `i`, `None` adds one.
    editing: Option<Option<usize>>,
    /// The form's model pick.
    model: String,
    name: Entity<TextInput>,
    description: Entity<TextInput>,
    tools: Entity<TextInput>,
    prompt: Entity<TextInput>,
    /// Why the form can't be saved yet.
    error: Option<String>,
}

impl KynexApp {
    /// Reads the agents from disk (a small file) and makes the form's fields.
    pub(super) fn new_agents_view(&mut self, cx: &mut Context<Self>) -> AgentsView {
        AgentsView {
            list: delegate::load(),
            editing: None,
            model: "haiku".into(),
            name: cx.new(|cx| TextInput::new(cx).mono().placeholder("test-writer")),
            description: cx.new(|cx| {
                TextInput::new(cx)
                    .multiline()
                    .placeholder("When Claude should use it, e.g. \"Writes unit tests for code you name.\"")
            }),
            tools: cx.new(|cx| TextInput::new(cx).mono().placeholder("Every tool; or a list like Bash, Read, Grep")),
            prompt: cx.new(|cx| TextInput::new(cx).multiline().placeholder("Its instructions")),
            error: None,
        }
    }

    fn agents_view(&mut self) -> Option<&mut AgentsView> {
        self.app_settings.as_mut().map(|s| &mut s.agents)
    }

    /// Saves the list; sessions started from now on get it.
    fn save_agents(&mut self, cx: &mut Context<Self>) {
        let Some(list) = self.agents_view().map(|v| v.list.clone()) else { return };
        if let Err(e) = delegate::save(&list) {
            self.toast(Toast::error(e));
        }
        cx.notify();
    }

    /// Opens the form on agent `i`, or empty for a new one.
    fn edit_agent(&mut self, i: Option<usize>, cx: &mut Context<Self>) {
        let Some(view) = self.agents_view() else { return };
        let agent = i.and_then(|i| view.list.get(i).cloned()).unwrap_or_default();
        view.editing = Some(i);
        view.model = agent.model.clone();
        view.error = None;
        let fields = [
            (view.name.clone(), agent.name),
            (view.description.clone(), agent.description),
            (view.tools.clone(), agent.tools.join(", ")),
            (view.prompt.clone(), agent.prompt),
        ];
        for (input, text) in fields {
            input.update(cx, |i, cx| i.set_text(text, cx));
        }
        cx.notify();
    }

    fn save_agent_form(&mut self, cx: &mut Context<Self>) {
        let Some(view) = self.agents_view() else { return };
        let Some(editing) = view.editing else { return };
        let text = |input: &Entity<TextInput>| input.read(cx).text().trim().to_string();
        let agent = Agent {
            name: text(&view.name),
            description: text(&view.description),
            prompt: text(&view.prompt),
            model: view.model.clone(),
            tools: delegate::parse_tools(&text(&view.tools)),
            on: editing.and_then(|i| view.list.get(i)).is_none_or(|a| a.on),
        };
        let others: Vec<Agent> =
            view.list.iter().enumerate().filter(|(j, _)| Some(*j) != editing).map(|(_, a)| a.clone()).collect();
        if let Err(e) = delegate::check(&agent, &others) {
            view.error = Some(e);
            return cx.notify();
        }
        match editing {
            Some(i) if i < view.list.len() => view.list[i] = agent,
            _ => view.list.push(agent),
        }
        view.editing = None;
        self.save_agents(cx);
    }

    fn change_agents(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut Vec<Agent>)) {
        let Some(view) = self.agents_view() else { return };
        f(&mut view.list);
        self.save_agents(cx);
    }

    pub(super) fn render_agents_settings(&self, t: &Palette, cx: &mut Context<Self>) -> gpui::Div {
        let Some(view) = self.app_settings.as_ref().map(|s| &s.agents) else { return div() };
        let delegate_on = self.saved.delegate;
        let switch = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(heading(
                "Delegate to subagents",
                "Sessions on Sonnet or Opus hand contained work, build and test runs and web lookups to the cheaper agents below, with short reports, and keep design and debugging. A project can override this in its Claude settings. From a session's next start; Anthropic only.",
                t,
            ))
            .children([(true, "On"), (false, "Off: the session's model does everything")].into_iter().map(|(on, label)| {
                choice(("delegate", usize::from(on)), delegate_on == on, label.into(), t).on_click(cx.listener(
                    move |this, _, _, cx| {
                        this.saved.delegate = on;
                        delegate::set_default(on);
                        this.save();
                        cx.notify();
                    },
                ))
            }));
        let body = match view.editing {
            Some(editing) => self.render_agent_form(view, editing, t, cx).into_any_element(),
            None => self.render_agent_list(view, t, cx).into_any_element(),
        };
        div().flex().flex_col().gap_6().child(switch).child(body)
    }

    fn render_agent_list(&self, view: &AgentsView, t: &Palette, cx: &mut Context<Self>) -> gpui::Div {
        let on = view.list.iter().filter(|a| a.on).count();
        let rows = view.list.iter().enumerate().map(|(i, a)| {
            let hover = t.hover;
            let model = delegate::MODELS.iter().find(|m| m.0 == a.model).map_or(a.model.as_str(), |m| m.1);
            let model = if a.model.is_empty() { "Session's model".to_string() } else { model.to_string() };
            div()
                .id(("agent", i))
                .flex()
                .items_start()
                .gap_2()
                .px_2()
                .py_1()
                .rounded_md()
                .hover(move |s| s.bg(hover))
                .child(
                    div()
                        .id(("agent-on", i))
                        .pt_0p5()
                        .cursor_pointer()
                        .child(radio(a.on, t))
                        .tooltip(Tooltip::text(if a.on { "In use: click to turn off" } else { "Off: click to use" }))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.change_agents(cx, |list| list[i].on = !list[i].on);
                        })),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .flex()
                                .gap_2()
                                .items_baseline()
                                .child(div().font_family(mono()).text_sm().child(a.name.clone()))
                                .child(div().text_xs().text_color(t.text_placeholder).child(model)),
                        )
                        .child(div().text_xs().text_color(t.text_muted).child(a.description.clone())),
                )
                .child(
                    icon_button(("agent-edit", i), Icon::Settings, cx)
                        .tooltip(Tooltip::text("Edit"))
                        .on_click(cx.listener(move |this, _, _, cx| this.edit_agent(Some(i), cx))),
                )
                .child(icon_button(("agent-remove", i), Icon::Trash, cx).tooltip(Tooltip::text("Remove")).on_click(
                    cx.listener(move |this, _, _, cx| {
                        this.change_agents(cx, |list| {
                            list.remove(i);
                        })
                    }),
                ))
        });
        let shown: Vec<&Agent> = view.list.iter().filter(|a| a.on).collect();
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(heading(
                "Agents",
                "Kynex's own, passed to each session when it starts; nothing is written to your .claude folders. Each project can turn single agents off.",
                t,
            ))
            .children(rows)
            .when(view.list.is_empty(), |d| {
                d.child(div().px_2().text_sm().text_color(t.text_placeholder).child("No agents."))
            })
            .when(on > MANY, |d| {
                d.child(div().px_2().pt_1().text_xs().text_color(t.warning).child(format!(
                    "{on} agents in use. Every description goes into every session, and more choices make Claude pick less well; a handful works best."
                )))
            })
            .child(
                div()
                    .pt_2()
                    .flex()
                    .gap_2()
                    .child(
                        outline_button("agent-add", "Add agent", cx)
                            .on_click(cx.listener(|this, _, _, cx| this.edit_agent(None, cx))),
                    )
                    .child(outline_button("agent-defaults", "Restore defaults", cx).on_click(cx.listener(
                        |this, _, _, cx| this.change_agents(cx, |list| *list = delegate::defaults()),
                    ))),
            )
            .when(!shown.is_empty(), |d| {
                d.child(div().pt_4().text_xs().text_color(t.text_muted).child(
                    "What Claude is told (a Sonnet session skips sonnet-worker, a Haiku one haiku-worker):",
                ))
                .child(
                    div()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .border_1()
                        .border_color(t.border_variant)
                        .bg(t.editor)
                        .text_xs()
                        .text_color(t.text_muted)
                        .child(delegate::rule(&shown)),
                )
            })
    }

    fn render_agent_form(
        &self,
        view: &AgentsView,
        editing: Option<usize>,
        t: &Palette,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let field = |label: &'static str, input: &Entity<TextInput>| {
            div().flex().flex_col().gap_1().child(div().text_xs().text_color(t.text_muted).child(label)).child(
                div()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .border_1()
                    .border_color(t.border)
                    .bg(t.editor)
                    .text_sm()
                    .child(input.clone()),
            )
        };
        let model = view.model.clone();
        let models = delegate::MODELS.iter().enumerate().map(|(n, (id, label))| {
            let id = id.to_string();
            choice(("agent-model", n), model == id, label.to_string(), t).on_click(cx.listener(
                move |this, _, _, cx| {
                    if let Some(v) = this.agents_view() {
                        v.model = id.clone();
                    }
                    cx.notify();
                },
            ))
        });
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(heading(
                if editing.is_some() { "Edit agent" } else { "New agent" },
                "Shift+Enter for a new line. Short reports are asked of every agent, so you needn't.",
                t,
            ))
            .child(field("Name", &view.name))
            .child(field("When to use it (Claude reads this to decide)", &view.description))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(div().text_xs().text_color(t.text_muted).child("Model"))
                    .children(models),
            )
            .child(field("Tools", &view.tools))
            .child(field("Instructions", &view.prompt))
            .children(view.error.clone().map(|e| div().text_xs().text_color(t.error).child(e)))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        primary_button("agent-save", "Save", cx)
                            .on_click(cx.listener(|this, _, _, cx| this.save_agent_form(cx))),
                    )
                    .child(outline_button("agent-cancel", "Cancel", cx).on_click(cx.listener(|this, _, _, cx| {
                        if let Some(v) = this.agents_view() {
                            v.editing = None;
                        }
                        cx.notify();
                    }))),
            )
    }
}
