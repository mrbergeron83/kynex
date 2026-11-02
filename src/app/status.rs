//! What the footer keeps in sight on the right: the account's usage limits
//! (`kynex_core::usage`, from the CLI's `rate_limit_event`s) and a pill while a
//! Kynex or Claude Code update waits, which opens the About modal.

use std::time::Duration;

use crate::ui::px;
use gpui::{AnyElement, Context, div, prelude::*};
use kynex_core::cli::UpdateState;
use kynex_core::openrouter;
use kynex_core::usage::{self, Level, Limits, Window};

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

    /// What's left on the OpenRouter key, once asked: beside the Anthropic
    /// limits, opening Settings → Providers.
    pub(super) fn render_openrouter_credit(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let t = theme(cx).clone();
        let (info, at) = self.openrouter_credit.as_ref().filter(|_| self.openrouter_key.is_some())?;
        let low = info.limit_remaining.is_some_and(|left| left < 1.0);
        let checked = kynex_core::fmt::ago(std::time::SystemTime::now() - at.elapsed());
        // Today's free requests, beside the credit while the session is on a
        // `:free` model; their daily cap is the limit that bites there.
        let on_free = self
            .active_session()
            .and_then(|s| s.provider().with_model(&s.model).model().map(|m| m.ends_with(":free")))
            .unwrap_or(false);
        let free = info.free.filter(|_| on_free);
        let resets = info
            .free
            .map(|_| format!(" · free count resets in {}", openrouter::free_reset_in(usage::now())))
            .unwrap_or_default();
        Some(
            ghost_button("openrouter-credit", cx)
                .gap_1p5()
                .child(div().text_color(t.text_placeholder).child("OpenRouter"))
                .child(div().text_color(if low { t.warning } else { t.text_muted }).child(info.short()))
                .children(free.map(|f| {
                    let color = if f.spent() {
                        t.error
                    } else if f.low() {
                        t.warning
                    } else {
                        t.text_muted
                    };
                    div().text_color(color).child(f.short())
                }))
                .tooltip(Tooltip::with_meta(
                    "OpenRouter credit",
                    format!("{}{resets} · checked {checked}", info.credit()),
                ))
                .on_click(
                    cx.listener(|this, _, _, cx| this.open_settings_on(super::app_settings::PROVIDERS_SECTION, cx)),
                )
                .into_any_element(),
        )
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
                .h(px(20.))
                .text_xs()
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
