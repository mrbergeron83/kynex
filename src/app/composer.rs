//! The message box under the transcript: text, attachments, model / effort /
//! mode pickers, send and stop. A pending permission prompt takes its place.

use std::path::PathBuf;
use std::sync::Arc;

use crate::ui::px;
use gpui::{
    Anchor, AnyElement, ClickEvent, Context, ExternalPaths, Focusable, Image, ImageFormat, KeyDownEvent,
    PathPromptOptions, SharedString, Window, canvas, deferred, div, img, prelude::*,
};
use kynex_core::attach::{is_image, prepare_image};
use kynex_core::project::{self, Mode};
use kynex_core::session::{Attachment, Kind, Session};

use kynex_core::toasts::Toast;

use super::keys::{CycleMode, Interrupt};
use super::menus::Menu;
use super::state::{Active, View};
use super::{KynexApp, models};
use crate::theme::theme;
use crate::ui::completions::completions;
use crate::ui::ease_height::ease_height;
use crate::ui::motion::{self, fade, fade_in, pop};
use crate::ui::{Icon, Tooltip, ghost_button, icon, icon_button, input, spinner};

impl KynexApp {
    pub(super) fn send(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let busy = self.session_mut().is_some_and(|s| s.busy);
        let text = self.composer.read(cx).text().trim().to_string();
        self.slash.dismissed = None;
        self.mentions.dismissed = None;
        self.last_esc = None;
        self.remember_sent(&text, cx);
        // Built-ins Kynex runs itself (`/clear`, `/model`…) work even mid-reply.
        if self.attachments.is_empty() && self.run_command(&text, cx) {
            self.composer.update(cx, |c, cx| c.set_text("", cx));
            return;
        }
        if text.is_empty() && self.attachments.is_empty() {
            return;
        }
        if busy {
            return self.queue_message(text, cx);
        }
        let atts = std::mem::take(&mut self.attachments);
        let model = self.active_model();
        if let Some(s) = self.session_mut() {
            s.use_model(model);
            s.send(text, atts);
        }
        self.composer.update(cx, |c, cx| c.set_text("", cx));
        self.follow_tail(cx);
        cx.notify();
    }

    /// Has Claude summarize the conversation so far, freeing up its context.
    pub(super) fn compact(&mut self, cx: &mut Context<Self>) {
        let model = self.active_model();
        if let Some(s) = self.session_mut().filter(|s| !s.busy) {
            s.use_model(model);
            s.send("/compact".into(), Vec::new());
        }
        self.follow_tail(cx);
        cx.notify();
    }

    pub(super) fn interrupt(&mut self, cx: &mut Context<Self>) {
        if let Some(s) = self.session_mut() {
            s.interrupt();
        }
        cx.notify();
    }

    fn attach_files(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Attach".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = paths.await {
                this.update(cx, |this, cx| {
                    for p in paths {
                        this.drop_path(p, cx);
                    }
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    /// A dropped (or picked) path: folders become projects, images attach,
    /// anything else goes into the message as its path.
    fn drop_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if path.is_dir() {
            self.add_project(path);
        } else if is_image(&path) {
            match std::fs::read(&path) {
                Ok(bytes) => self.attach_bytes(&bytes, cx),
                Err(e) => {
                    self.toast(Toast::error(format!("Couldn't read image {}: {e}", path.display())));
                }
            }
        } else {
            self.composer.update(cx, |c, cx| {
                let mut text = c.text().to_string();
                if !text.is_empty() && !text.ends_with(' ') {
                    text.push(' ');
                }
                text.push_str(&path.display().to_string());
                c.set_text(text, cx);
            });
        }
    }

    pub(super) fn on_drop(&mut self, paths: &ExternalPaths, _: &mut Window, cx: &mut Context<Self>) {
        for p in paths.paths() {
            self.drop_path(p.clone(), cx);
        }
        cx.notify();
    }

    pub(super) fn attach_bytes(&mut self, bytes: &[u8], cx: &mut Context<Self>) {
        let in_code = self.active_project().is_none_or(|p| p.view == View::Code);
        if !in_code {
            return;
        }
        match prepare_image(bytes) {
            Ok(png) => self.attachments.push(Attachment {
                media_type: "image/png".into(),
                base64: png.base64,
                preview: Arc::new(Image::from_bytes(ImageFormat::Png, png.bytes)),
            }),
            Err(e) => {
                self.toast(Toast::error(e));
            }
        }
        cx.notify();
    }

    pub(super) fn render_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = theme(cx).clone();
        let update = self.render_cli_update(cx);
        let trust = self.render_trust(cx);
        let queued = self.render_queue(cx);
        let running = self.render_running_tasks(cx);
        let handoff = self.render_handoff(cx);
        let files_badge = self.scratch_badge(cx);
        // A prompt is the one surface that needs you: it gets the amber lamp colour.
        self.slash.shown = None;
        let popup = match () {
            _ if self.prompt.is_some() => None,
            _ if self.rewinding() => self.render_rewind(cx),
            _ if self.history_searching() => self.render_history_search(cx),
            _ => self.render_slash(cx).or_else(|| self.render_mentions(cx)),
        };
        // The slash-command and mention lists play out when they close; the others just go.
        let slash = self.slash.shown.clone();
        let slash_leaving = self.exits.slash.track(slash.as_ref(), window, cx);
        let (popup, popup_leaving) = match (popup, slash_leaving) {
            (Some(p), _) => (Some(p), None),
            (None, Some(((items, selected), t))) => {
                let list = completions(&items, selected, "", cx, |_, _, _| {});
                (Some(list.w_full().max_w(px(560.)).into_any_element()), Some(t))
            }
            (None, None) => (None, None),
        };
        // A prompt that just closed fades out over the composer it hands back.
        let prompt = self.prompt.as_ref().map(|p| p.view.clone());
        let prompt_out = self.exits.prompt.track(prompt.as_ref(), window, cx).map(|(view, gone)| {
            let layer = div().absolute().inset_0().p_3().rounded_lg().overflow_hidden().bg(t.panel).child(view);
            fade("prompt-out", layer, motion::QUICK, Some(gone))
        });
        let this = cx.entity().downgrade();
        let colour = self.active_colour(&t);
        let shell = |child: AnyElement, focused: bool, needs_you: bool| {
            div().w_full().flex().justify_center().px_6().pb_4().pt_1().child(
                div()
                    .w_full()
                    .when_some(self.saved.column.max_prose(), |d, w| d.max_w(px(w)))
                    .flex()
                    .flex_col()
                    .gap_1p5()
                    .children(update)
                    .children(trust)
                    .children(running)
                    .children(handoff)
                    .children(queued)
                    .child(
                        div()
                            .relative()
                            .w_full()
                            // Remembered so a command can open a menu above the composer.
                            .child(
                                canvas(
                                    move |bounds, _, cx| {
                                        this.update(cx, |this, _| this.slash.composer = Some(bounds)).ok();
                                    },
                                    |_, _, _, _| {},
                                )
                                .absolute()
                                .size_full(),
                            )
                            .when_some(popup, |d, popup| {
                                // Slash commands, mentions, history search, rewind: rise into place above the box.
                                d.child(deferred(pop(
                                    "composer-popup-in",
                                    div().absolute().bottom_full().left_0().w_full().pb_1().occlude().child(popup),
                                    gpui::px(4.),
                                    popup_leaving,
                                )))
                            })
                            .p_3()
                            .rounded_lg()
                            .border_1()
                            // The project's colour edges it, so you see where a prompt goes.
                            .border_color(match colour {
                                _ if needs_you => t.warning,
                                Some(c) if focused => c,
                                Some(c) => c.opacity(0.5),
                                None if focused => t.border_focused,
                                None => t.border,
                            })
                            .bg(t.panel)
                            .shadow_sm()
                            // Grows and shrinks with its text, a prompt's turn, attachments: eased.
                            .child(ease_height("composer-ease", child))
                            .children(prompt_out),
                    ),
            )
        };
        if let Some(p) = &self.prompt {
            let focused = p.view.read(cx).focus_handle(cx).contains_focused(window, cx);
            // Each prompt fades in over the composer it replaces.
            let id = ("prompt-in", p.view.entity_id().as_u64() as usize);
            let view = fade_in(id, div().child(p.view.clone()), motion::QUICK);
            return shell(view.into_any_element(), focused, true);
        }

        let session = self.active_session();
        let busy = session.is_some_and(|s| s.busy);
        // A chief worker runs without prompts; its mode doesn't apply.
        let worker = session.is_some_and(|s| s.worker);
        let mode = session.filter(|s| s.kind == Kind::Code && !s.worker).map(|s| s.mode);
        models::fetch_models(cx);
        // Who answers: a picker on a new session when both are set up, else
        // a label (a session keeps its provider once it starts).
        let provider = self.active_provider();
        let on_openrouter = provider.is_openrouter();
        let fresh = session.is_none_or(Session::is_new);
        let can_switch = fresh && self.providers_available().len() > 1;
        let show_provider = can_switch || on_openrouter;
        let model_label = self.active_model_name();
        // A session with a model of its own marks the chip with a message bubble.
        let own_model = self.session_model().is_some();
        // Effort lives in the project's Claude settings (a tiny file); chat has
        // none. Its own chip beside the model's: "Effort" for the default, else the level.
        let effort_label = self.active_project().filter(|_| mode.is_some() && provider.takes_effort()).map(|p| {
            let effort = self.claude_settings(&p.path).effort;
            project::EFFORTS
                .iter()
                .find(|e| e.0 == effort && !e.0.is_empty())
                .map_or_else(|| "Effort".to_string(), |(_, name)| format!("{name} effort"))
        });
        let chip_tip = if own_model {
            "Model pinned to this session; applies from its next start"
        } else if self.active_project().is_some() {
            "Model for this project's sessions; applies from the next session start"
        } else {
            "Model for chats; applies from the next session start"
        };
        let in_project = matches!(self.saved.active, Active::Project(_));
        let placeholder = if self.rewinding() {
            "Pick a message to go back to · Esc to cancel"
        } else if self.history_searching() {
            "Search what you've sent…"
        } else if provider == kynex_core::provider::Provider::OpenRouterChat {
            "Message the model… (chat only: it can't see or change files)"
        } else if in_project {
            "Ask Claude Code to do something in this project…"
        } else {
            "Message Claude…"
        };
        self.composer.update(cx, |c, _| c.set_placeholder(placeholder));
        let focused = self.composer.read(cx).is_focused(window);
        let has_message = !self.composer.read(cx).text().trim().is_empty() || !self.attachments.is_empty();
        let can_send = !busy && has_message;
        let max_h = window.viewport_size().height * 0.4;

        let attachments =
            div().flex().flex_wrap().gap_2().children(self.attachments.iter().enumerate().map(|(i, a)| {
                let preview = a.preview.clone().downcast::<Image>().ok();
                let group = SharedString::from(format!("attachment-{i}"));
                div()
                    .group(group.clone())
                    .relative()
                    .child(
                        div()
                            .id(("attachment", i))
                            .rounded_md()
                            .border_1()
                            .border_color(t.border)
                            .overflow_hidden()
                            .cursor_pointer()
                            .children(preview.clone().map(|p| img(p).h(px(56.)).max_w(px(120.))))
                            .tooltip(Tooltip::text("Click to view"))
                            .when_some(preview, |d, p| {
                                d.on_click(
                                    cx.listener(move |this, _, window, cx| this.open_image(p.clone(), window, cx)),
                                )
                            }),
                    )
                    // Removing it is the small × that shows on hover.
                    .child(
                        div()
                            .absolute()
                            .top(px(-6.))
                            .right(px(-6.))
                            .invisible()
                            .group_hover(group, |s| s.visible())
                            .child(
                                div()
                                    .id(("remove-attachment", i))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .size(px(18.))
                                    .rounded_full()
                                    .border_1()
                                    .border_color(t.border)
                                    .bg(t.panel)
                                    .cursor_pointer()
                                    .hover(|s| s.bg(t.hover))
                                    .child(icon(Icon::Close, t.text_muted).size_3())
                                    .tooltip(Tooltip::text("Remove"))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.attachments.remove(i);
                                        cx.notify();
                                    })),
                            ),
                    )
            }));

        // While Claude works, a written message can be queued beside the stop button.
        let queue = (busy && has_message).then(|| {
            ghost_button("queue", cx)
                .text_sm()
                .text_color(t.text_muted)
                .child(icon(Icon::ArrowUp, t.text_muted).size_3())
                .child("Queue")
                .tooltip(Tooltip::with_meta("Send when Claude finishes", "Enter"))
                .on_click(cx.listener(|this, _, window, cx| this.send(window, cx)))
        });
        let send = if busy {
            div()
                .id("stop")
                .flex()
                .items_center()
                .gap_1p5()
                .h(px(26.))
                .px_2()
                .rounded_md()
                .border_1()
                .border_color(t.border)
                .text_sm()
                .cursor_pointer()
                .hover(|s| s.bg(t.hover))
                .child(spinner("busy", t.accent, px(14.)))
                .child("Stop")
                .tooltip(Tooltip::with_meta("Interrupt Claude", "Esc · change it in keymap.json"))
                .on_click(cx.listener(|this, _, _, cx| this.interrupt(cx)))
        } else {
            div()
                .id("send")
                .flex()
                .items_center()
                .justify_center()
                .size(px(26.))
                .rounded_md()
                .bg(if can_send { t.accent } else { t.active })
                .when(can_send, |d| d.cursor_pointer().hover(|s| s.opacity(0.9)))
                .child(icon(Icon::ArrowUp, if can_send { t.on_accent } else { t.text_placeholder }))
                .tooltip(Tooltip::with_meta("Send", "Enter · Shift+Enter for a new line"))
                .on_click(cx.listener(|this, _, window, cx| this.send(window, cx)))
        };

        // While the history search or a completion popup is open it takes the keys first.
        let body = div()
            .flex()
            .flex_col()
            .gap_2()
            .capture_action(cx.listener(|this, _: &input::Up, _, cx| {
                if this.rewind_move(false, cx)
                    || this.history_search_move(false, cx)
                    || this.slash_move(false, cx)
                    || this.mention_move(false, cx)
                {
                    cx.stop_propagation();
                }
            }))
            // The field passes ↑ / ↓ on from its first / last line: the queue
            // comes back, then earlier messages, as in Claude Code.
            .on_action(cx.listener(|this, _: &input::Up, _, cx| {
                if !this.history_up(cx) {
                    cx.propagate();
                }
            }))
            .capture_action(cx.listener(|this, _: &input::Down, _, cx| {
                if this.rewind_move(true, cx)
                    || this.history_search_move(true, cx)
                    || this.slash_move(true, cx)
                    || this.mention_move(true, cx)
                {
                    cx.stop_propagation();
                }
            }))
            .on_action(cx.listener(|this, _: &input::Down, _, cx| {
                if !this.history_down(cx) {
                    cx.propagate();
                }
            }))
            .capture_action(cx.listener(|this, _: &input::Enter, window, cx| {
                if this.rewind_accept(cx) {
                    cx.stop_propagation();
                } else if this.history_searching() {
                    this.take_history_match(cx);
                    cx.stop_propagation();
                } else if this.slash_accept(true, window, cx) || this.mention_accept(window, cx) {
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, _: &Interrupt, _, cx| {
                if this.rewind_back(cx)
                    || this.cancel_history_search(cx)
                    || this.slash_dismiss(cx)
                    || this.mention_dismiss(cx)
                    || this.double_esc(cx)
                {
                    cx.stop_propagation();
                }
            }))
            .capture_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                let k = &ev.keystroke;
                if k.key == "tab" && !k.modifiers.modified() && this.rewind_accept(cx) {
                    return cx.stop_propagation();
                }
                if k.key == "tab" && !k.modifiers.modified() && this.history_searching() {
                    this.take_history_match(cx);
                    return cx.stop_propagation();
                }
                if k.key == "tab"
                    && !k.modifiers.modified()
                    && (this.slash_accept(false, window, cx) || this.mention_accept(window, cx))
                {
                    cx.stop_propagation();
                }
                // Ctrl+Enter, Claude Code's "send now": the queue and the draft go right away.
                let m = &k.modifiers;
                if k.key == "enter" && m.control && !m.shift && !m.alt && !m.platform {
                    this.send_queue_now(cx);
                    cx.stop_propagation();
                }
            }))
            .when(!self.attachments.is_empty(), |d| d.child(attachments))
            .child(div().id("composer-scroll").max_h(max_h).overflow_y_scroll().text_sm().child(self.composer.clone()))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        icon_button("attach", Icon::Paperclip, cx)
                            .tooltip(Tooltip::with_meta("Attach images or files", "or paste / drop them"))
                            .on_click(cx.listener(|this, _, _, cx| this.attach_files(cx))),
                    )
                    .children(files_badge.map(|new| {
                        // The session's files; a dot with how many scratch files changed since you looked.
                        div()
                            .relative()
                            .child(
                                icon_button("session-files", Icon::Files, cx)
                                    .tooltip(Tooltip::with_meta(
                                        "This session's files",
                                        "scratch folder, edited files, images, task output",
                                    ))
                                    .on_click(cx.listener(|this, _, window, cx| this.open_session_scratch(window, cx))),
                            )
                            .when(new > 0, |d| {
                                d.child(
                                    div()
                                        .absolute()
                                        .top(px(-3.))
                                        .right(px(-3.))
                                        .min_w(px(14.))
                                        .h(px(14.))
                                        .px(px(3.))
                                        .rounded_full()
                                        .bg(t.accent)
                                        .text_color(t.on_accent)
                                        .text_size(px(9.))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .child(if new > 99 { "99+".to_string() } else { new.to_string() }),
                                )
                            })
                    }))
                    .when(show_provider, |d| {
                        d.child(
                            ghost_button("provider", cx)
                                .text_color(t.text_muted)
                                .gap_1()
                                .child(provider.label())
                                .when(can_switch, |b| b.child(icon(Icon::ChevronDown, t.text_placeholder).size_3()))
                                .tooltip(Tooltip::text(if can_switch {
                                    "Who answers this session; it's fixed once it starts"
                                } else if fresh {
                                    "Set up the other provider in Settings → Providers to switch"
                                } else {
                                    "Fixed for this session: start a new session to use another provider"
                                }))
                                .when(can_switch, |b| {
                                    b.on_click(cx.listener(|this, ev: &ClickEvent, _, cx| {
                                        this.open_menu(Menu::Provider, ev.position(), Anchor::BottomLeft, cx)
                                    }))
                                }),
                        )
                    })
                    .child(
                        ghost_button("model", cx)
                            .text_color(t.text_muted)
                            .gap_1()
                            .when(own_model, |b| b.child(icon(Icon::Message, t.text_placeholder).size_3()))
                            .child(model_label)
                            .child(icon(Icon::ChevronDown, t.text_placeholder).size_3())
                            .tooltip(Tooltip::text(chip_tip))
                            .on_click(cx.listener(|this, ev: &ClickEvent, _, cx| {
                                this.open_menu(Menu::Model, ev.position(), Anchor::BottomLeft, cx)
                            })),
                    )
                    .children(effort_label.map(|effort| {
                        ghost_button("effort", cx)
                            .text_color(t.text_muted)
                            .gap_1()
                            .child(effort)
                            .child(icon(Icon::ChevronDown, t.text_placeholder).size_3())
                            .tooltip(Tooltip::text("How hard Claude thinks in this project"))
                            .on_click(cx.listener(|this, ev: &ClickEvent, _, cx| {
                                this.open_menu(Menu::Effort, ev.position(), Anchor::BottomLeft, cx)
                            }))
                    }))
                    .when_some(mode, |d, mode| {
                        d.child(
                            ghost_button("mode", cx)
                                .text_color(if mode == Mode::Plan { t.info } else { t.text_muted })
                                .child(mode.label())
                                .child(icon(Icon::ChevronDown, t.text_placeholder).size_3())
                                .tooltip(Tooltip::for_action(mode.help(), CycleMode, "next mode"))
                                .on_click(cx.listener(|this, ev: &ClickEvent, _, cx| {
                                    this.open_menu(Menu::Mode, ev.position(), Anchor::BottomLeft, cx)
                                })),
                        )
                    })
                    .when(worker, |d| {
                        d.child(
                            div()
                                .id("worker-mode")
                                .px_1p5()
                                .text_color(t.warning)
                                .child("KNX Ops worker · no prompts")
                                .tooltip(Tooltip::text(
                                    "KNX Ops (the chief of staff) runs this session with no permission prompts, \
                                     in its own worktree",
                                )),
                        )
                    })
                    .child(div().flex_1())
                    .children(queue)
                    .child(send),
            );
        shell(body.into_any_element(), focused, false)
    }
}
