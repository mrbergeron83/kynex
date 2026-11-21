//! What the footer keeps in sight on the right: OpenRouter credit (when an OpenRouter key
//! is saved) and a pill while a Kynex or Claude Code update waits, which opens About Kynex.

use crate::ui::px;
use gpui::{AnyElement, Context, div, prelude::*};
use kynex_core::cli::UpdateState;
use kynex_core::openrouter;

use super::KynexApp;
use crate::theme::theme;
use crate::ui::{Icon, Tooltip, ghost_button, icon, spinner};

fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

impl KynexApp {
    /// What's left on the OpenRouter key, once asked; clicking opens Settings → Providers.
    pub(super) fn render_openrouter_credit(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let t = theme(cx).clone();
        let (info, at) = self.openrouter_credit.as_ref().filter(|_| self.openrouter_key.is_some())?;
        let low = info.limit_remaining.is_some_and(|left| left < 1.0);
        let checked = kynex_core::fmt::ago(std::time::SystemTime::now() - at.elapsed());
        let on_free = self
            .focused_pane_id()
            .and_then(|p| self.agent_pane_location(p))
            .and_then(|(_, m)| m.model)
            .is_some_and(|m| m.ends_with(":free"));
        let free = info.free.filter(|_| on_free);
        let resets = info
            .free
            .map(|_| format!(" · free count resets in {}", openrouter::free_reset_in(unix_now())))
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

    /// "Update" while a Kynex or managed CLI (`claude`, `grok`, `agy`) update waits, until installed.
    pub(super) fn render_update_pill(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let t = theme(cx).clone();
        let mut available: Vec<String> = Vec::new();
        let mut updating: Option<String> = None;

        match &self.app_update {
            UpdateState::Updating { latest } => updating = Some(format!("Installing Kynex {latest}")),
            UpdateState::Updated { version } => {
                return Some(self.update_pill_el("Restart", false, format!("Restart into Kynex {version}"), &t, cx));
            }
            UpdateState::Available { latest, .. } => available.push(format!("Kynex {latest} is available")),
            _ => {}
        }

        for cli in super::setup::ManagedCli::ALL {
            match &self.cli_updates.get(cli).update {
                UpdateState::Updating { latest } if updating.is_none() => {
                    updating = Some(format!("Installing {} {latest}", cli.label()));
                }
                UpdateState::Available { latest, .. } => {
                    available.push(format!("{} {latest} is available", cli.label()));
                }
                _ => {}
            }
        }

        let (label, busy, tip) = if let Some(tip) = updating {
            ("Updating…", true, tip)
        } else if !available.is_empty() {
            ("Update", false, available.join("\n"))
        } else {
            return None;
        };
        Some(self.update_pill_el(label, busy, tip, &t, cx))
    }

    fn update_pill_el(
        &self,
        label: &'static str,
        busy: bool,
        tip: String,
        t: &crate::theme::Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
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
            .into_any_element()
    }
}
