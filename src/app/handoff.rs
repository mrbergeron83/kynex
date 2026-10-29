//! "Your turn in the browser": when Claude drove a browser and stopped to ask
//! you to log in, pass MFA or accept something there, a banner above the
//! composer says so, with the latest screenshot and a Continue button. The
//! rule for when it shows is `kynex_core::mcp::handoff`; sending anything ends it.

use gpui::{AnyElement, Context, FontWeight, div, prelude::*, px};
use kynex_core::mcp;
use kynex_core::session::Item;

use super::KynexApp;
use crate::theme::theme;
use crate::ui::{Icon, Tooltip, ghost_button, icon};

impl KynexApp {
    /// Tells Claude you've finished in the browser.
    fn continue_after_handoff(&mut self, cx: &mut Context<Self>) {
        let model = self.active_model();
        if let Some(s) = self.session_mut().filter(|s| !s.busy) {
            if !s.is_running() {
                s.model = model;
            }
            s.send(mcp::CONTINUE.into(), Vec::new());
        }
        self.follow_tail(cx);
        cx.notify();
    }

    pub(super) fn render_handoff(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let session = self.active_session().filter(|s| !s.busy)?;
        let handoff = mcp::handoff(&session.items)?;
        let images = handoff.screenshot.and_then(|r| match session.items.get(r) {
            Some(Item::ToolResult { images, .. }) => Some((r, images.clone())),
            _ => None,
        });
        let t = theme(cx).clone();
        let shot = images.map(|(r, images)| self.thumbnails(("handoff-shot", r).into(), &images, 48., cx));
        Some(
            div()
                .flex()
                .items_center()
                .gap_3()
                .p_2()
                .rounded_md()
                .border_1()
                .border_color(t.warning.opacity(0.5))
                .bg(t.warning.opacity(0.08))
                .children(shot)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_1p5()
                                .text_sm()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(t.text)
                                .child(icon(Icon::Shield, t.warning).size_3())
                                .child("Claude is waiting for you in the browser"),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(t.text_muted)
                                .child("Finish there (log in, confirm…), then continue, or reply below."),
                        ),
                )
                .child(
                    ghost_button("handoff-continue", cx)
                        .h(px(24.))
                        .px_2()
                        .flex_none()
                        .text_sm()
                        .border_1()
                        .border_color(t.border)
                        .text_color(t.text)
                        .child("Continue")
                        .tooltip(Tooltip::with_meta("Tell Claude you're done", mcp::CONTINUE))
                        .on_click(cx.listener(|this, _, _, cx| this.continue_after_handoff(cx))),
                )
                .into_any_element(),
        )
    }
}
