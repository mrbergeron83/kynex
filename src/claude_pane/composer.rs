//! The message box under the transcript: text, attachments, model / effort /
//! mode pickers, send and stop. A pending permission prompt takes its place.

use std::path::PathBuf;
use std::sync::Arc;

use crate::ui::px;
use gpui::{
    AnyElement, ClickEvent, Context, ExternalPaths, Focusable, Image, ImageFormat, KeyDownEvent, PathPromptOptions,
    SharedString, Window, canvas, deferred, div, img, prelude::*,
};
use kynex_core::claude::attach::{is_image, prepare_image};
use kynex_core::claude::input::input_mode::{InputMode, Submit};
use kynex_core::claude::session::Attachment;
use kynex_core::project::{self, Mode};
use kynex_core::toasts::Toast;

use super::keys::CycleMode;
use super::menus::Menu;
use super::{ClaudePane, Key};
use crate::app::keys::Interrupt;
use crate::theme::theme;
use crate::ui::completions::completions;
use crate::ui::ease_height::ease_height;
use crate::ui::motion::{self, fade, fade_in, pop};
use crate::ui::{Icon, Tooltip, ghost_button, icon, icon_button, input, spinner};

impl ClaudePane {
    /// Enter in the field: sends on the pane's session.
    pub(super) fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.send_to(Some(self.key()), window, cx);
    }

    /// Sends the composer's message on the session `key` (queued while it works).
    fn send_to(&mut self, key: Option<Key>, window: &mut Window, cx: &mut Context<Self>) {
        let busy = key.and_then(|k| self.session_for(k)).is_some_and(|s| s.busy);
        let raw = self.composer.read(cx).text().to_string();
        self.slash.dismissed = None;
        self.mentions.dismissed = None;
        self.last_esc = None;
        // `!` runs a command, `#` asks where the note goes; pastes go in full.
        let text = match self.take_submit(&raw) {
            (_, Submit::Send(text)) => text,
            (mode, Submit::Shell(command)) => {
                self.remember_sent(&mode.prefixed(&command), cx);
                return self.run_shell(key, command, cx);
            }
            (_, Submit::Memory(_)) => return self.ask_memory(cx),
            (_, Submit::Nothing) => return,
        };
        let pasted = self.composer.read(cx).pasted() || !self.pastes.is_empty();
        self.remember_sent(&text, cx);
        // Built-ins Kynex runs itself (`/clear`, `/model`…) work even mid-reply.
        if self.attachments.is_empty() && self.run_command(&text, window, cx) {
            self.composer.update(cx, |c, cx| c.set_text("", cx));
            self.reset_input_mode();
            return;
        }
        if text.is_empty() && self.attachments.is_empty() {
            return;
        }
        if busy {
            return self.queue_message(key, text, cx);
        }
        let atts = std::mem::take(&mut self.attachments);
        let model = self.active_model();
        if let Some(s) = key.and_then(|k| self.session_for(k)) {
            s.use_model(model);
            // Typed, not pasted: the CLI may run the turn at extra-high effort.
            let ultrathink = s.ultrathink.typed(&text, pasted);
            s.send_with(text, atts, ultrathink);
        }
        self.composer.update(cx, |c, cx| c.set_text("", cx));
        self.reset_input_mode();
        self.follow_tail(cx);
        self.sync(cx);
        cx.notify();
    }

    /// Has Claude summarize the conversation so far, freeing up its context.
    pub(super) fn compact(&mut self, key: Option<Key>, cx: &mut Context<Self>) {
        let model = self.active_model();
        if let Some(s) = key.and_then(|k| self.session_for(k)).filter(|s| !s.busy) {
            s.use_model(model);
            s.send("/compact".into(), Vec::new());
        }
        self.follow_tail(cx);
        self.sync(cx);
        cx.notify();
    }

    fn interrupt_session(&mut self, key: Option<Key>, cx: &mut Context<Self>) {
        if let Some(s) = key.and_then(|k| self.session_for(k)) {
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

    /// A dropped (or picked) path: images attach, anything else goes into the
    /// message as its path.
    fn drop_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if is_image(&path) && !path.is_dir() {
            match std::fs::read(&path) {
                Ok(bytes) => self.attach_bytes(&bytes, cx),
                Err(e) => {
                    self.toast(Toast::error(format!("Couldn't read image {}: {e}", path.display())), cx);
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
        match prepare_image(bytes) {
            Ok(png) => self.attachments.push(Attachment {
                media_type: "image/png".into(),
                base64: png.base64,
                preview: Arc::new(Image::from_bytes(ImageFormat::Png, png.bytes)),
            }),
            Err(e) => {
                self.toast(Toast::error(e), cx);
            }
        }
        cx.notify();
    }

    pub(super) fn render_composer(
        &mut self,
        key: Option<Key>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let t = theme(cx).clone();
        let queued = key.and_then(|k| self.render_queue(k, cx));
        let running = key.and_then(|k| self.render_running_tasks(k, cx));
        let agents = key.and_then(|k| self.render_turn_agents(k, cx));
        let todos = key.and_then(|k| self.render_todos(k, cx));
        let handoff = key.and_then(|k| self.render_handoff(k, cx));
        // A prompt is the one surface that needs you: it gets the amber lamp colour.
        self.slash.shown = None;
        let popup = match () {
            _ if self.prompt.is_some() => None,
            _ if self.input_mode == InputMode::Shortcuts => Some(self.render_shortcuts_card(cx)),
            _ if self.rewinding() => self.render_rewind(cx),
            _ if self.history_searching() => self.render_history_search(cx),
            _ => self.render_slash(cx).or_else(|| self.render_mentions(cx)),
        };
        // The slash-command and mention lists play out when they close; the others just go.
        let slash = self.slash.shown.clone();
        let slash_leaving = self.slash_exit.track(slash.as_ref(), window, cx);
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
        let prompt_out = self.prompt_exit.track(prompt.as_ref(), window, cx).map(|(view, gone)| {
            let layer = div().absolute().inset_0().p_3().rounded_lg().overflow_hidden().bg(t.panel).child(view);
            fade("prompt-out", layer, motion::QUICK, Some(gone))
        });
        self.mark_composer(key, cx);
        let this = cx.entity().downgrade();
        // Bypass permissions, then the composer's mode, outrank the project's colour.
        let bypass = key.and_then(|k| self.session_by_key(k)).is_some_and(|s| s.mode == Mode::Bypass);
        let mode_colour = super::input_modes::mode_colour(self.input_mode, &t);
        let project_colour = self.colour.as_deref().map(|c| t.project_colour(c));
        let colour = if bypass { Some(t.error) } else { mode_colour.or(project_colour) };
        let max_prose = self.column.max_prose();
        let shell = |child: AnyElement, focused: bool, needs_you: bool| {
            div().flex_none().w_full().flex().justify_center().px_6().pb_4().pt_1().child(
                div()
                    .w_full()
                    .when_some(max_prose, |d, w| d.max_w(px(w)))
                    .flex()
                    .flex_col()
                    .gap_1p5()
                    .children(todos)
                    .children(agents)
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
                                // Slash commands, mentions, history search: rise into place above the box.
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

        let session = key.and_then(|k| self.session_by_key(k));
        let busy = session.is_some_and(|s| s.busy);
        super::models::fetch_models(cx);
        // Who answers: fixed by the pane (Claude Code, or Claude Code on OpenRouter).
        let provider = self.session.provider().provider();
        let on_openrouter = provider.is_openrouter();
        let mode = session.map(|s| s.mode);
        let model_label = self.active_model_name();
        // A session with a model of its own marks the chip with a message bubble.
        let own_model = self.session_model().is_some();
        // Effort lives in the project's Claude settings (a tiny file). Its own chip
        // beside the model's: "Effort" for the default, else the level.
        let effort_label = (mode.is_some() && provider.takes_effort()).then(|| {
            let effort = &self.claude.effort;
            project::EFFORTS
                .iter()
                .find(|e| e.0 == effort && !e.0.is_empty())
                .map_or_else(|| "Effort".to_string(), |(_, name)| format!("{name} effort"))
        });
        let chip_tip = if own_model {
            "Model pinned to this session; applies from its next start"
        } else {
            "Model for this project's sessions; applies from the next session start"
        };
        let input_mode = self.input_mode;
        let placeholder = if input_mode != InputMode::Prompt {
            input_mode.placeholder()
        } else if self.rewinding() {
            "Pick a message to go back to · Esc to cancel"
        } else if self.history_searching() {
            "Search what you've sent…"
        } else {
            "Ask Claude Code to do something in this project…"
        };
        self.composer.update(cx, |c, _| c.set_placeholder(placeholder));
        let mode_badge = self.render_mode_badge(cx);
        let thinking = effort_label.as_ref().and_then(|_| self.render_thinking_chip(cx));
        let focused = self.composer.read(cx).is_focused(window);
        let has_message = !self.composer.read(cx).text().trim().is_empty() || !self.attachments.is_empty();
        let can_send = !busy && has_message;
        let max_h = window.viewport_size().height * 0.4;
        let composer_scroll = self.composer.read(cx).scroll_handle().clone();

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
                .on_click(cx.listener(move |this, _, window, cx| this.send_to(key, window, cx)))
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
                .on_click(cx.listener(move |this, _, _, cx| this.interrupt_session(key, cx)))
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
                .on_click(cx.listener(move |this, _, window, cx| this.send_to(key, window, cx)))
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
            // Backspace in the empty field leaves `!`, `#` or `?`.
            .capture_action(cx.listener(|this, _: &input::Backspace, _, cx| {
                if this.leave_input_mode(cx) {
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, _: &Interrupt, _, cx| {
                if this.rewind_back(cx)
                    || this.cancel_history_search(cx)
                    || this.slash_dismiss(cx)
                    || this.mention_dismiss(cx)
                    || this.leave_input_mode(cx)
                    || this.double_esc(cx)
                {
                    cx.stop_propagation();
                }
            }))
            .capture_key_down(cx.listener(move |this, ev: &KeyDownEvent, window, cx| {
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
                    this.send_queue_now(key, cx);
                    cx.stop_propagation();
                }
                // Alt+Enter steers: the queue and the draft go into the running turn.
                if k.key == "enter" && m.alt && !m.control && !m.shift && !m.platform {
                    this.steer_now(key, cx);
                    cx.stop_propagation();
                }
            }))
            .when(!self.attachments.is_empty(), |d| d.child(attachments))
            .children(mode_badge)
            .child(
                div()
                    .id("composer-scroll")
                    .max_h(max_h)
                    .overflow_y_scroll()
                    .track_scroll(&composer_scroll)
                    .text_sm()
                    .child(self.composer.clone()),
            )
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
                    .when(on_openrouter, |d| {
                        d.child(
                            ghost_button("provider", cx)
                                .text_color(t.text_muted)
                                .gap_1()
                                .child(provider.picker().label())
                                .tooltip(Tooltip::text(
                                    "Fixed for this session: start a new session to use another provider",
                                )),
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
                                this.open_menu(Menu::Model, ev.position(), cx)
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
                                this.open_menu(Menu::Effort, ev.position(), cx)
                            }))
                    }))
                    .children(thinking)
                    .when_some(mode, |d, mode| {
                        d.child(
                            ghost_button("mode", cx)
                                .text_color(match mode {
                                    Mode::Plan => t.info,
                                    Mode::Bypass => t.error,
                                    _ => t.text_muted,
                                })
                                .child(mode.label())
                                .child(icon(Icon::ChevronDown, t.text_placeholder).size_3())
                                .tooltip(Tooltip::for_action(mode.help(), CycleMode, "next mode"))
                                .on_click(cx.listener(|this, ev: &ClickEvent, _, cx| {
                                    this.open_menu(Menu::Mode, ev.position(), cx)
                                })),
                        )
                    })
                    .child(div().flex_1())
                    .children(queue)
                    .child(send),
            );
        shell(body.into_any_element(), focused, false)
    }
}
