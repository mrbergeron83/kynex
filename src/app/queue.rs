//! Messages queued while Claude works, with Claude Code's keys: Enter queues,
//! they all go out together when the turn ends (Esc included), Ctrl+Enter sends
//! them now, and ↑ from the composer's first line takes them back to edit. A
//! small stack above the composer shows them. The rules live in
//! `kynex_core::queue`; this file only asks and draws.

use crate::ui::px;
use gpui::{AnyElement, Context, Image, div, prelude::*};

use super::KynexApp;
use crate::theme::theme;
use crate::ui::{Icon, Tooltip, ghost_button, icon_button};

impl KynexApp {
    /// Queues the composer's message on the shown session.
    pub(super) fn queue_message(&mut self, text: String, cx: &mut Context<Self>) {
        let atts = std::mem::take(&mut self.attachments);
        if let Some(s) = self.session_mut() {
            s.queue.push(text, atts);
        }
        self.composer.update(cx, |c, cx| c.set_text("", cx));
        cx.notify();
    }

    /// Ctrl+Enter: the queue and the draft behind it go now, stopping Claude
    /// if it's working. With nothing queued and Claude idle, it just sends.
    pub(super) fn send_queue_now(&mut self, cx: &mut Context<Self>) {
        let text = self.composer.read(cx).text().trim().to_string();
        self.remember_sent(&text, cx);
        if !text.is_empty() || !self.attachments.is_empty() {
            self.queue_message(text, cx);
        }
        let model = self.active_model();
        if let Some(s) = self.session_mut() {
            s.use_model(model);
            s.send_queue_now();
        }
        self.follow_tail(cx);
        cx.notify();
    }

    /// Takes a queued message back into the composer, ahead of what's there.
    pub(super) fn edit_queued(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some(m) = self.session_mut().and_then(|s| s.queue.remove(i)) else { return };
        self.take_back(m, cx);
    }

    /// ↑ on the composer's first line: everything queued comes back to edit.
    pub(super) fn take_back_queue(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(m) = self.session_mut().and_then(|s| s.queue.take_all()) else { return false };
        self.take_back(m, cx);
        true
    }

    fn take_back(&mut self, m: kynex_core::queue::Queued, cx: &mut Context<Self>) {
        let current = self.composer.read(cx).text().to_string();
        let text = if current.trim().is_empty() { m.text } else { format!("{}\n\n{current}", m.text) };
        self.composer.update(cx, |c, cx| c.set_text(text, cx));
        self.attachments.splice(0..0, m.attachments);
        cx.notify();
    }

    pub(super) fn render_queue(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let session = self.active_session()?;
        let (items, busy) = (session.queue.items(), session.busy);
        if items.is_empty() {
            return None;
        }
        let t = theme(cx).clone();
        let rows = items.iter().enumerate().map(|(i, m)| {
            let first_line = m.text.lines().find(|l| !l.trim().is_empty()).unwrap_or("").to_string();
            let images = m.attachments.iter().filter_map(|a| a.preview.clone().downcast::<Image>().ok());
            div()
                .flex()
                .items_center()
                .gap_2()
                .pl_3()
                .pr_1()
                .py_0p5()
                .rounded_md()
                .border_1()
                .border_color(t.border_variant)
                .bg(t.panel)
                .children(images.map(|p| gpui::img(p).h(px(18.)).max_w(px(32.)).rounded_sm()))
                .child(
                    div()
                        .id(("queued-edit", i))
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_sm()
                        .text_color(t.text_muted)
                        .cursor_pointer()
                        .hover(|s| s.text_color(t.text))
                        .child(first_line)
                        .tooltip(Tooltip::with_meta("Edit in the message box", "↑ takes them all back"))
                        .on_click(cx.listener(move |this, _, _, cx| this.edit_queued(i, cx))),
                )
                .child(
                    icon_button(("queued-remove", i), Icon::Close, cx)
                        .tooltip(Tooltip::text("Remove from the queue"))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(s) = this.session_mut() {
                                s.queue.remove(i);
                            }
                            cx.notify();
                        })),
                )
        });
        let n = items.len();
        let what = if n == 1 { "1 message queued".to_string() } else { format!("{n} messages queued") };
        let when = if busy { "sent when Claude finishes or is stopped" } else { "not sent yet" };
        let header = div()
            .flex()
            .items_center()
            .gap_2()
            .px_1()
            .child(div().flex_1().text_xs().text_color(t.text_placeholder).child(format!("{what} · {when}")))
            .child(
                ghost_button("queue-send-now", cx)
                    .h(px(20.))
                    .text_xs()
                    .text_color(t.text_muted)
                    .child("Send now")
                    .tooltip(Tooltip::with_meta(
                        if busy { "Stop Claude and send these right away" } else { "Send these now" },
                        kynex_core::fmt::keystroke(
                            kynex_core::fmt::Mods { control: true, ..Default::default() },
                            "enter",
                            cfg!(target_os = "macos"),
                        ),
                    ))
                    .on_click(cx.listener(|this, _, _, cx| this.send_queue_now(cx))),
            );
        Some(div().flex().flex_col().gap_1().child(header).children(rows).into_any_element())
    }
}
