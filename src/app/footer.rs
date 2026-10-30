//! The status footer along the bottom of the window (#172): what Kynex and the
//! processes it started cost on the left (`kynex_core::resources`), and on the
//! right the usage-limit meters and the update pill (`status.rs`).

use std::time::Duration;

use gpui::{Context, div, prelude::*};
use kynex_core::resources::Sampler;

use super::KynexApp;
use crate::theme::theme;
use crate::ui::Tooltip;

pub(super) const FOOTER_HEIGHT: gpui::Rems = gpui::Rems(26. / 16.);

/// How often Kynex reads its processes' CPU and memory.
const SAMPLE_EVERY: Duration = Duration::from_secs(3);

impl KynexApp {
    /// Reads Kynex's process tree every few seconds, off the UI thread.
    pub(super) fn watch_resources(cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let mut sampler = Sampler::default();
            loop {
                let (back, usage) = cx
                    .background_executor()
                    .spawn(async move {
                        let usage = sampler.sample();
                        (sampler, usage)
                    })
                    .await;
                sampler = back;
                let shown = this.update(cx, |this, cx| {
                    if this.resources.as_ref() != Some(&usage) {
                        this.resources = Some(usage);
                        cx.notify();
                    }
                });
                if shown.is_err() {
                    break;
                }
                cx.background_executor().timer(SAMPLE_EVERY).await;
            }
        })
        .detach();
    }

    pub(super) fn render_footer(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = theme(cx).clone();
        let resources = self.resources.as_ref().filter(|u| !u.processes.is_empty()).map(|u| {
            div()
                .id("resources")
                .px_1p5()
                .text_color(t.text_placeholder)
                .child(u.label())
                .tooltip(Tooltip::with_meta("Kynex and what it started", u.describe()))
        });
        div()
            .id("footer")
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .h(FOOTER_HEIGHT)
            .px_2()
            .bg(t.title_bar)
            .border_t_1()
            .border_color(t.border)
            .text_xs()
            .children(resources)
            .child(div().flex_1())
            .children(self.render_usage(cx))
            .children(self.render_update_pill(cx))
    }
}
