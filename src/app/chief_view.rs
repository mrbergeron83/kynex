//! The chief of staff's view: the warning before it's turned on, the reporter
//! over its conversation (every worker, what it's doing and has used, its
//! question or warnings, with Open and Stop), and Kynex's own messages in its
//! transcript.

use gpui::{AnyElement, Context, FontWeight, Window, div, prelude::*};
use kynex_core::chief::State;
use kynex_core::fmt;

use super::KynexApp;
use crate::theme::theme;
use crate::ui::{Icon, Lamp, Tooltip, ghost_button, icon, lamp, outline_button, primary_button, px};

impl KynexApp {
    /// The Chat tab's Chief view before it's turned on: what it does and what it risks.
    pub(super) fn render_chief_consent(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx).clone();
        let point = |text: &'static str| {
            div()
                .flex()
                .gap_2()
                .child(div().flex_none().pt(px(7.)).child(div().size(px(4.)).rounded_full().bg(t.text_muted)))
                .child(div().flex_1().child(text))
        };
        let mut points = vec![
            point(
                "Workers run with no permission prompts: any command, any file, the network, git push. Nothing \
                 is reviewed before it runs; pushes, deletions and publishing are flagged after the fact.",
            ),
            point(
                "Each worker changes only its own git worktree of the project (branch chief-worker), so it \
                 doesn't touch the copy you work in. You review and merge its branch.",
            ),
            point(
                "Every worker uses your Claude quota, on its project's model. The reporter shows what each has used.",
            ),
        ];
        if kynex_core::terminal::elevated() {
            points.push(point("Kynex is running as administrator, so the workers' commands do too."));
        }
        div()
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .p_6()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .max_w(px(520.))
                    .p_5()
                    .rounded_lg()
                    .border_1()
                    .border_color(t.border)
                    .bg(t.panel)
                    .text_sm()
                    .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child("Chief of staff"))
                    .child(div().text_color(t.text_muted).child(
                        "Give the chief a goal. It sends the work to one chief worker per project, follows it, \
                         answers their questions or brings them to you, and reports back. The chief runs on \
                         Haiku; each worker runs on its project's model and can use subagents.",
                    ))
                    .child(div().font_weight(FontWeight::MEDIUM).child("Before you turn it on"))
                    .child(div().flex().flex_col().gap_2().text_color(t.text_muted).children(points))
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .pt_2()
                            .child(primary_button("chief-on", "Turn on the chief", cx).on_click(cx.listener(
                                |this, _, window, cx| {
                                    this.saved.chief_consented = true;
                                    this.save();
                                    this.chief_live_mut();
                                    this.focus_input(window, cx);
                                    cx.notify();
                                },
                            )))
                            .child(outline_button("chief-back", "Back to Chat", cx).on_click(cx.listener(
                                |this, _, _, cx| {
                                    this.saved.chief_view = false;
                                    this.save();
                                    cx.notify();
                                },
                            ))),
                    ),
            )
            .into_any_element()
    }

    /// The reporter, over the chief's conversation: the chief and every
    /// worker, what each is doing and has used, with Open and Stop.
    pub(super) fn render_reporter(&mut self, _: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.in_chief() || !self.saved.chief_consented {
            return None;
        }
        let t = theme(cx).clone();
        let workers = self.chief_workers();
        let chief = self.chief.live.current().map(|s| (s.spent, s.busy));
        let total = workers.iter().fold(chief.map(|c| c.0).unwrap_or_default(), |a, w| a.plus(w.spent));
        let anything = chief.is_some_and(|c| c.1) || workers.iter().any(|w| w.state == State::Working || w.queued > 0);
        let head = div()
            .flex()
            .items_center()
            .gap_2()
            .h(px(24.))
            .text_xs()
            .child(div().font_weight(FontWeight::MEDIUM).child("Chief"))
            .child(div().text_color(t.text_placeholder).child(format!(
                    "{} · {} tokens · all {} tokens",
                    self.chief
                        .live
                        .current()
                        .map_or_else(|| self.dir_model(&kynex_core::paths::chief_dir()), |s| s.model.clone()),
                    fmt::tokens(chief.map(|c| c.0.tokens()).unwrap_or(0)),
                    fmt::tokens(total.tokens()),
                )))
            .child(div().flex_1())
            .when(anything, |d| {
                d.child(
                    ghost_button("chief-stop-all", cx)
                        .h(px(20.))
                        .text_xs()
                        .text_color(t.text_muted)
                        .child(icon(Icon::Stop, t.text_muted).size_3())
                        .child("Stop all")
                        .tooltip(Tooltip::text("Stop the chief and every worker, and drop what they had queued"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.stop_everything();
                            cx.notify();
                        })),
                )
            });
        // The workers' questions waiting for you, oldest first; the box below
        // answers the picked one (the oldest unless you pick another).
        let focus = self.chief_focus().map(|a| a.id);
        let stack = self.question_stack();
        let hover = t.hover;
        let stack_view = (!stack.is_empty()).then(|| {
            let n = stack.len();
            div()
                .flex()
                .flex_col()
                .gap_0p5()
                .pb_1()
                .child(div().text_xs().font_weight(FontWeight::MEDIUM).text_color(t.warning).child(format!(
                    "{n} question{} for you. Answer in the box below; pick one here to answer it first.",
                    if n == 1 { "" } else { "s" }
                )))
                .children(stack.into_iter().enumerate().map(|(i, a)| {
                    let current = focus == Some(a.id);
                    let id = a.id;
                    let age = fmt::elapsed(a.since.elapsed());
                    div()
                        .id(("ops-question", i))
                        .flex()
                        .items_center()
                        .gap_2()
                        .h(px(22.))
                        .px_2()
                        .rounded_md()
                        .text_xs()
                        .cursor_pointer()
                        .when(current, |d| d.bg(t.active))
                        .when(!current, |d| d.hover(move |s| s.bg(hover)))
                        .child(div().flex_none().font_weight(FontWeight::MEDIUM).child(a.name))
                        .child(div().flex_1().min_w_0().truncate().text_color(t.text_muted).child(a.headline))
                        .child(div().flex_none().text_color(t.text_placeholder).child(age))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.chief.focus = Some(id);
                            cx.notify();
                        }))
                }))
        });
        let rows: Vec<AnyElement> = workers
            .iter()
            .enumerate()
            .map(|(i, w)| {
                let key = self.chief.workers.get(&w.path).copied();
                let state = match w.state {
                    State::Working => Lamp::Working,
                    State::NeedsYou | State::Blocked => Lamp::NeedsYou,
                    State::Idle | State::Asleep => Lamp::Idle,
                };
                let mut detail = vec![match (w.state, w.turn) {
                    (State::Working, Some(d)) => format!("working {}", fmt::elapsed(d)),
                    (s, _) => s.label().to_string(),
                }];
                if w.agents > 0 {
                    detail.push(format!("{} subagent{}", w.agents, if w.agents == 1 { "" } else { "s" }));
                }
                if w.queued > 0 {
                    detail.push(format!("{} queued", w.queued));
                }
                // What it asks, or what it did that's risky, under its line.
                let note = w.question.clone().map(|q| (q, t.warning)).or_else(|| {
                    (!w.warnings.is_empty()).then(|| (format!("It {}.", w.warnings.join("; it ")), t.warning))
                });
                let stop = (w.state == State::Working || w.queued > 0).then(|| {
                    ghost_button(("worker-stop", i), cx)
                        .h(px(20.))
                        .text_xs()
                        .text_color(t.text_muted)
                        .child(icon(Icon::Stop, t.text_muted).size_3())
                        .child("Stop")
                        .tooltip(Tooltip::text("Stop this worker's turn and drop what it had queued"))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(k) = key {
                                this.stop_worker(k);
                            }
                            cx.notify();
                        }))
                });
                let open = key.map(|k| {
                    ghost_button(("worker-open", i), cx)
                        .h(px(20.))
                        .text_xs()
                        .text_color(t.text_muted)
                        .child("Open")
                        .tooltip(Tooltip::text("Show this worker's session in its project"))
                        .on_click(cx.listener(move |this, _, window, cx| this.go_to_session(k, window, cx)))
                });
                div()
                    .flex()
                    .flex_col()
                    .pl_2()
                    .pr_1()
                    .py_0p5()
                    .rounded_md()
                    .border_1()
                    .border_color(t.border_variant)
                    .bg(t.panel)
                    .text_xs()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .h(px(22.))
                            .child(lamp(("worker-lamp", i), state, cx))
                            .child(div().flex_none().font_weight(FontWeight::MEDIUM).child(w.name()))
                            .child(div().flex_none().text_color(t.text_placeholder).child(w.model.clone()))
                            .child(
                                div().flex_1().min_w_0().truncate().text_color(t.text_muted).child(detail.join(" · ")),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_color(t.text_placeholder)
                                    .child(format!("{} tokens", fmt::tokens(w.spent.tokens()))),
                            )
                            .children(open)
                            .children(stop),
                    )
                    .children(
                        note.map(|(text, colour)| {
                            div().pl(px(15.)).pb_0p5().text_color(colour).line_clamp(2).child(text)
                        }),
                    )
                    .into_any_element()
            })
            .collect();
        Some(
            div()
                .flex()
                .flex_col()
                .flex_none()
                .gap_1()
                .px_3()
                .py_2()
                .border_b_1()
                .border_color(t.border_variant)
                .child(head)
                .children(stack_view)
                .when(rows.is_empty(), |d| {
                    d.child(div().text_xs().text_color(t.text_placeholder).child("No workers yet."))
                })
                .children(rows)
                .into_any_element(),
        )
    }

    /// One of Kynex's messages to the chief, drawn as Kynex's rather than as yours.
    pub(super) fn render_chief_notice(&self, text: &str, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx).clone();
        let body = text.trim_start_matches(kynex_core::chief::NOTICE).trim_start();
        let (head, rest) = body.split_once('\n').unwrap_or((body, ""));
        div()
            .flex()
            .gap_2()
            .px_3()
            .py_2()
            .rounded_lg()
            .border_1()
            .border_color(t.border_variant)
            .text_xs()
            .child(icon(Icon::Timeline, t.text_placeholder).size_3p5().flex_none().mt(px(1.)))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .flex_1()
                    .min_w_0()
                    .child(div().text_color(t.text_muted).font_weight(FontWeight::MEDIUM).child(head.to_string()))
                    .when(!rest.trim().is_empty(), |d| {
                        d.child(div().text_color(t.text_placeholder).line_clamp(6).child(rest.trim().to_string()))
                    }),
            )
            .into_any_element()
    }
}
