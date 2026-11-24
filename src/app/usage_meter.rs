//! The footer's usage-limit meters: the account's limits
//! (`kynex_core::claude::usage`, from the CLI's `rate_limit_event`s), as the
//! Claude panes last reported them, this run or the last.

use std::time::Duration;

use crate::ui::px;
use gpui::{AnyElement, Context, div, prelude::*};
use kynex_core::claude::usage::{self, Level, Limits, Window};

use super::KynexApp;
use crate::theme::theme;
use crate::ui::Tooltip;

/// How often the meter catches up with the clock: a window that resets empties.
const TICK: Duration = Duration::from_secs(60);

impl KynexApp {
    /// Repaints the meter every so often, so reset times and passed resets stay true.
    pub(super) fn watch_usage(cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(TICK).await;
                if this
                    .update(cx, |this, cx| {
                        if this.usage.is_some() {
                            cx.notify()
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    /// A session's process reported the account's limits; kept for the next start.
    pub(super) fn limits_reported(&mut self, limits: Limits, cx: &mut Context<Self>) {
        let limits = match &self.usage {
            Some(older) => older.clone().then(limits),
            None => limits,
        };
        if self.usage.as_ref() == Some(&limits) {
            return;
        }
        self.usage = Some(limits.clone());
        cx.background_spawn(async move { limits.save() }).detach();
        cx.notify();
    }

    /// The account's 5-hour and weekly windows side by side, "5h ▬ 42%  7d ▬ 53%",
    /// then Fable's own weekly one once a Fable turn has reported it, with reset
    /// times in the tooltip. Each turns amber close to its limit, and
    /// red with the wait once it's hit.
    pub(super) fn render_usage(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let t = theme(cx).clone();
        let now = usage::now();
        let limits = self.usage.as_ref()?.at(now);
        let meter = |name: &'static str, w: Window| {
            let level = w.level();
            let color = match level {
                Level::Ok => t.text_muted,
                Level::Near => t.warning,
                Level::Limited => t.error,
            };
            let text = match level {
                Level::Limited => format!("Limited · {}", usage::until(w.resets_at.saturating_sub(now))),
                _ => w.percent(),
            };
            let fill = (w.used.clamp(0.0, 1.0) * 28.0) as f32;
            div()
                .flex()
                .items_center()
                .gap_1p5()
                .child(div().text_color(t.text_placeholder).child(name))
                .child(div().w(px(28.)).h(px(4.)).rounded_full().bg(t.border).child(
                    div().h_full().w(px(fill)).rounded_full().bg(match level {
                        Level::Ok => t.accent,
                        _ => color,
                    }),
                ))
                .child(div().text_color(color).child(text))
        };
        let meters: Vec<_> = [("5h", limits.five_hour), ("7d", limits.seven_day), ("Fable", limits.fable)]
            .into_iter()
            .filter_map(|(name, w)| Some(meter(name, w?)))
            .collect();
        (!meters.is_empty()).then(|| {
            div()
                .id("usage")
                .flex()
                .items_center()
                .gap_3()
                .px_1p5()
                .text_xs()
                .children(meters)
                .tooltip(Tooltip::with_meta("Claude usage limits", limits.describe(now)))
                .into_any_element()
        })
    }
}
