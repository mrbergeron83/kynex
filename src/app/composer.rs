//! The message box under the transcript: text, attachments, model / effort /
//! mode pickers, send and stop. A pending permission prompt takes its place.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    Anchor, Animation, AnimationExt, AnyElement, ClickEvent, Context, ExternalPaths, Focusable, Image, ImageFormat,
    KeyDownEvent, PathPromptOptions, Window, canvas, deferred, div, img, prelude::*, px,
};
use kynex_core::attach::{is_image, prepare_image};
use kynex_core::project::{self, ClaudeSettings, Mode};
use kynex_core::session::{Attachment, Kind};

use super::keys::Interrupt;
use super::menus::Menu;
use super::state::{Active, View};
use super::transcript::THREAD_WIDTH;
use super::{KynexApp, models};
use crate::theme::{Palette, theme};
use crate::ui::{Icon, Tooltip, ghost_button, icon, icon_button, input, spinner};

impl KynexApp {
    pub(super) fn send(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let busy = self.session_mut().is_some_and(|s| s.busy);
        let text = self.composer.read(cx).text().trim().to_string();
        self.slash.dismissed = None;
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
        let model = self.saved.model.clone();
        if let Some(s) = self.session_mut() {
            if !s.is_running() {
                s.model = model;
            }
            s.send(text, atts);
        }
        self.composer.update(cx, |c, cx| c.set_text("", cx));
        self.follow_tail();
        cx.notify();
    }

    /// Has Claude summarize the conversation so far, freeing up its context.
    pub(super) fn compact(&mut self, cx: &mut Context<Self>) {
        let model = self.saved.model.clone();
        if let Some(s) = self.session_mut().filter(|s| !s.busy) {
            if !s.is_running() {
                s.model = model;
            }
            s.send("/compact".into(), Vec::new());
        }
        self.follow_tail();
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
                Err(e) => self.notice = Some(format!("Couldn't read image {}: {e}", path.display())),
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
            Err(e) => self.notice = Some(e),
        }
        cx.notify();
    }

    pub(super) fn render_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = theme(cx).clone();
        // At a glance: is Claude working, waiting on you, or idle?
        let waiting = self.prompt.is_some();
        let working = !waiting && self.active_session().is_some_and(|s| s.busy);
        let status = (working || waiting).then(|| working_bar(working, &t));
        let update = self.render_cli_update(cx);
        let queued = self.render_queue(cx);
        // A prompt is the one surface that needs you: it gets the amber lamp colour.
        let popup = if self.prompt.is_none() { self.render_slash(cx) } else { None };
        let this = cx.entity().downgrade();
        let shell = |child: AnyElement, focused: bool, needs_you: bool| {
            div().w_full().flex().justify_center().px_6().pb_4().pt_1().child(
                div()
                    .w_full()
                    .max_w(THREAD_WIDTH)
                    .flex()
                    .flex_col()
                    .gap_1p5()
                    .children(update)
                    .children(status)
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
                                d.child(deferred(div().absolute().bottom_full().left_0().w_full().pb_1().child(popup)))
                            })
                            .p_3()
                            .rounded_lg()
                            .border_1()
                            .border_color(if needs_you {
                                t.warning
                            } else if focused {
                                t.border_focused
                            } else {
                                t.border
                            })
                            .bg(t.panel)
                            .shadow_sm()
                            .child(child),
                    ),
            )
        };
        if let Some(p) = &self.prompt {
            let focused = p.view.read(cx).focus_handle(cx).contains_focused(window, cx);
            return shell(p.view.clone().into_any_element(), focused, true);
        }

        let session = self.active_session();
        let busy = session.is_some_and(|s| s.busy);
        let mode = session.filter(|s| s.kind == Kind::Code).map(|s| s.mode);
        models::fetch_models(cx);
        let model_label = models::model_label(&self.saved.model);
        // Effort lives in the project's Claude settings (a tiny file); chat has none.
        let effort_label = self.active_project().filter(|_| mode.is_some()).map(|p| {
            let effort = ClaudeSettings::load(&p.path).effort;
            match project::EFFORTS.iter().find(|e| e.0 == effort) {
                Some((id, name)) if !id.is_empty() => format!("{name} effort"),
                _ => "Default effort".to_string(),
            }
        });
        let in_project = matches!(self.saved.active, Active::Project(_));
        let placeholder =
            if in_project { "Ask Claude Code to do something in this project…" } else { "Message Claude…" };
        self.composer.update(cx, |c, _| c.set_placeholder(placeholder));
        let focused = self.composer.read(cx).is_focused(window);
        let has_message = !self.composer.read(cx).text().trim().is_empty() || !self.attachments.is_empty();
        let can_send = !busy && has_message;
        let max_h = window.viewport_size().height * 0.4;

        let attachments =
            div().flex().flex_wrap().gap_2().children(self.attachments.iter().enumerate().map(|(i, a)| {
                let preview = a.preview.clone().downcast::<Image>().ok();
                div()
                    .id(("attachment", i))
                    .rounded_md()
                    .border_1()
                    .border_color(t.border)
                    .overflow_hidden()
                    .cursor_pointer()
                    .hover(|s| s.opacity(0.7))
                    .children(preview.map(|p| img(p).h(px(56.)).max_w(px(120.))))
                    .tooltip(Tooltip::text("Click to remove"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.attachments.remove(i);
                        cx.notify();
                    }))
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
                .child(icon(Icon::Stop, t.text).size_3())
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

        // While the slash-command popup is open it takes the keys first.
        let body = div()
            .flex()
            .flex_col()
            .gap_2()
            .capture_action(cx.listener(|this, _: &input::Up, _, cx| {
                if this.slash_move(false, cx) {
                    cx.stop_propagation();
                }
            }))
            // The field passes ↑ on from its first line: take the queue back, as in Claude Code.
            .on_action(cx.listener(|this, _: &input::Up, _, cx| {
                if !this.take_back_queue(cx) {
                    cx.propagate();
                }
            }))
            .capture_action(cx.listener(|this, _: &input::Down, _, cx| {
                if this.slash_move(true, cx) {
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, _: &input::Enter, window, cx| {
                if this.slash_accept(true, window, cx) {
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, _: &Interrupt, _, cx| {
                if this.slash_dismiss(cx) {
                    cx.stop_propagation();
                }
            }))
            .capture_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                let k = &ev.keystroke;
                if k.key == "tab" && !k.modifiers.modified() && this.slash_accept(false, window, cx) {
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
                    .child(
                        ghost_button("model", cx)
                            .text_color(t.text_muted)
                            .child(model_label)
                            .child(icon(Icon::ChevronDown, t.text_placeholder).size_3())
                            .tooltip(Tooltip::text("Model, for every tab; applies from the next session start"))
                            .on_click(cx.listener(|this, ev: &ClickEvent, _, cx| {
                                this.open_menu(Menu::Model, ev.position(), Anchor::BottomLeft, cx)
                            })),
                    )
                    .when_some(effort_label, |d, effort| {
                        d.child(
                            ghost_button("effort", cx)
                                .text_color(t.text_muted)
                                .child(effort)
                                .child(icon(Icon::ChevronDown, t.text_placeholder).size_3())
                                .tooltip(Tooltip::text("How hard Claude thinks in this project (--effort)"))
                                .on_click(cx.listener(|this, ev: &ClickEvent, _, cx| {
                                    this.open_menu(Menu::Effort, ev.position(), Anchor::BottomLeft, cx)
                                })),
                        )
                    })
                    .when_some(mode, |d, mode| {
                        d.child(
                            ghost_button("mode", cx)
                                .text_color(if mode == Mode::Plan { t.info } else { t.text_muted })
                                .child(mode.label())
                                .child(icon(Icon::ChevronDown, t.text_placeholder).size_3())
                                .tooltip(Tooltip::with_meta(mode.help(), "Shift+Tab to cycle"))
                                .on_click(cx.listener(|this, ev: &ClickEvent, _, cx| {
                                    this.open_menu(Menu::Mode, ev.position(), Anchor::BottomLeft, cx)
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

/// The strip above the composer: a label and a track with a sliding accent
/// while Claude works, or a steady warning-coloured one while it waits for you.
fn working_bar(working: bool, t: &Palette) -> AnyElement {
    let color = if working { t.accent } else { t.warning };
    let label = div()
        .flex()
        .items_center()
        .gap_1p5()
        .text_xs()
        .text_color(color)
        .child(if working {
            spinner("working-spin", color, px(12.)).into_any_element()
        } else {
            icon(Icon::Dot, color).size_2().into_any_element()
        })
        .child(if working { "Working…" } else { "Waiting for you" });
    let track = div().relative().w_full().h(px(2.)).rounded_full().overflow_hidden().bg(t.border_variant);
    let track = if working {
        track.child(div().absolute().top_0().h_full().w(gpui::relative(0.3)).rounded_full().bg(color).with_animation(
            "working-slide",
            Animation::new(Duration::from_millis(1400)).repeat().with_easing(gpui::ease_in_out),
            |bar, delta| bar.left(gpui::relative(delta * 1.3 - 0.3)),
        ))
    } else {
        track.child(div().size_full().bg(color.opacity(0.6)))
    };
    div().flex().flex_col().gap_1().px_1().child(label).child(track).into_any_element()
}
