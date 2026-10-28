//! What the title bar keeps in sight on the right: the account's usage limits
//! (`kynex_core::usage`, from the CLI's `rate_limit_event`s) and a pill while a
//! Kynex or Claude Code update waits, which opens the About modal.

use std::time::Duration;

use gpui::{AnyElement, Context, div, prelude::*, px};
use kynex_core::cli::UpdateState;
use kynex_core::usage::{self, Level, Limits};

use super::KynexApp;
use crate::theme::theme;
use crate::ui::{Icon, Tooltip, ghost_button, icon, spinner};

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
        if self.usage.as_ref() == Some(&limits) {
            return;
        }
        self.usage = Some(limits.clone());
        cx.background_spawn(async move { limits.save() }).detach();
        cx.notify();
    }

    /// The fuller of the 5-hour and weekly windows, "5h 42%", with both in the
    /// tooltip. Amber close to the limit, red with the wait once it's hit.
    pub(super) fn render_usage(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let t = theme(cx).clone();
        let now = usage::now();
        let limits = self.usage.as_ref()?.at(now);
        let w = limits.tightest()?;
        let name = if Some(w) == limits.five_hour { "5h" } else { "Week" };
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
        Some(
            div()
                .id("usage")
                .flex()
                .items_center()
                .gap_1p5()
                .mr_2()
                .px_1p5()
                .h(px(24.))
                .text_xs()
                .child(div().text_color(t.text_placeholder).child(name))
                .child(div().w(px(28.)).h(px(4.)).rounded_full().bg(t.border).child(
                    div().h_full().w(px(fill)).rounded_full().bg(match level {
                        Level::Ok => t.accent,
                        _ => color,
                    }),
                ))
                .child(div().text_color(color).child(text))
                .tooltip(Tooltip::with_meta("Usage limits", limits.describe(now)))
                .into_any_element(),
        )
    }

    /// "Update" while a Kynex or Claude Code update waits, until it's installed:
    /// the toast says so once, this stays. Kynex's own update comes first.
    pub(super) fn render_update_pill(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let t = theme(cx).clone();
        let (label, busy, tip) = match (&self.app_update, &self.cli_update) {
            (UpdateState::Updating { latest }, _) => ("Updating…", true, format!("Installing Kynex {latest}")),
            (UpdateState::Updated { version }, _) => ("Restart", false, format!("Restart into Kynex {version}")),
            (UpdateState::Available { latest, .. }, cli) => {
                let mut tip = format!("Kynex {latest} is available");
                if let UpdateState::Available { latest, .. } = cli {
                    tip.push_str(&format!("\nClaude Code {latest} is available"));
                }
                ("Update", false, tip)
            }
            (_, UpdateState::Updating { latest }) => ("Updating…", true, format!("Installing Claude Code {latest}")),
            (_, UpdateState::Available { latest, .. }) => {
                ("Update", false, format!("Claude Code {latest} is available"))
            }
            _ => return None,
        };
        Some(
            ghost_button("update-pill", cx)
                .mr_2()
                .border_1()
                .border_color(t.border)
                .text_color(t.accent)
                .child(if busy {
                    spinner("update-pill-spin", t.accent, px(12.)).into_any_element()
                } else {
                    icon(Icon::ArrowUp, t.accent).size_3().into_any_element()
                })
                .child(label)
                .tooltip(Tooltip::with_meta(tip, "Open About Kynex"))
                .on_click(cx.listener(|this, _, _, cx| this.open_about(cx)))
                .into_any_element(),
        )
    }
}
