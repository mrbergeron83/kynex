//! Inline keyboard-first provider picker shown inside a pending `agent_cli` pane,
//! plus one-click CLI installation (`claude`, `agy`, `grok`).

use std::path::Path;

use gpui::{AnyElement, Context, KeyDownEvent, div, prelude::*};
use kynex_core::agent_cli::{self, ProviderEntry};
use kynex_core::layout::PaneId;
use kynex_core::toasts::Toast;

use super::KynexApp;
use crate::theme::{mono, theme};
use crate::ui::{Icon, ghost_button, icon, outline_button, primary_button, px, spinner};

/// Inline provider picker state for a pending `agent_cli` pane.
#[derive(Clone, Default)]
pub(super) struct AgentPickerState {
    pub selected: usize,
    /// Selected OpenRouter model override (when an OpenRouter provider is highlighted).
    pub model: Option<String>,
}

impl KynexApp {
    /// Refreshes the cached provider catalog (checking disk/PATH install status) off the UI thread.
    pub(super) fn refresh_provider_catalog(&mut self, cx: &mut Context<Self>) {
        let rules = self.agent_rules.clone();
        let has_key = self.openrouter_key.is_some();
        cx.spawn(async move |this, cx| {
            let catalog = cx.background_spawn(async move { agent_cli::catalog(&rules, has_key) }).await;
            this.update(cx, |this, cx| {
                if this.provider_catalog != catalog {
                    this.provider_catalog = catalog;
                    this.rebuild_palette_if_open(cx);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Cached provider catalog for UI rendering (never blocks on PATH lookup).
    pub(super) fn provider_catalog(&self) -> &[ProviderEntry] {
        &self.provider_catalog
    }

    /// Installs a missing CLI (`claude`, `agy`, `grok`) right from the inline provider picker.
    fn install_provider_cli(&mut self, provider_id: String, cx: &mut Context<Self>) {
        if self.agent_installing.is_some() {
            return;
        }
        let base = agent_cli::rule_id(&provider_id).to_string();
        self.agent_installing = Some(base.clone());
        cx.notify();
        cx.spawn(async move |this, cx| {
            let base_bg = base.clone();
            let result = cx
                .background_spawn(async move {
                    match base_bg.as_str() {
                        agent_cli::CLAUDE => kynex_core::cli::install(),
                        agent_cli::AGY => {
                            let r = kynex_core::antigravity::setup::install();
                            kynex_core::antigravity::recheck();
                            r
                        }
                        agent_cli::GROK => {
                            let r = kynex_core::grok::setup::install();
                            kynex_core::grok::recheck();
                            r
                        }
                        other => Err(format!("Automatic install is not available for {other}.")),
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                this.agent_installing = None;
                match result {
                    Ok(()) => {
                        this.refresh_provider_catalog(cx);
                        this.toast(Toast::success(format!("Installed {base}. Press Enter to start.")));
                    }
                    Err(e) => {
                        this.toast(Toast::error(e));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Inline keyboard-first provider picker shown in a new pending `agent_cli` pane.
    pub(super) fn render_provider_picker(
        &mut self,
        pane: PaneId,
        ws: &Path,
        focused: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx).clone();
        let catalog = self.provider_catalog.clone();
        let state = self.agent_pickers.entry(pane).or_default().clone();
        let selected = state.selected.min(catalog.len().saturating_sub(1));
        let ws_buf = ws.to_path_buf();
        let folder = ws.file_name().map_or_else(|| ws.display().to_string(), |n| n.to_string_lossy().into_owned());
        let installing = self.agent_installing.clone();

        let chosen = catalog.get(selected).cloned();
        let active_model = chosen.as_ref().and_then(|e| {
            if !e.uses_openrouter {
                return None;
            }
            state.model.clone().or_else(|| {
                Some(if e.id == agent_cli::AGY_OPENROUTER {
                    kynex_core::provider::DEFAULT_ANTIGRAVITY_MODEL.to_string()
                } else {
                    self.saved.openrouter.default_model().to_string()
                })
            })
        });

        let rows: Vec<AnyElement> = catalog
            .iter()
            .enumerate()
            .map(|(i, entry)| {
                let is_sel = i == selected;
                let id = entry.id.clone();
                let ws_click = ws_buf.clone();
                let uses_or = entry.uses_openrouter;
                let ready = entry.installed && (!uses_or || self.openrouter_key.is_some());
                let status_badge: Option<(String, gpui::Hsla)> = if !entry.installed {
                    Some(("Not installed".into(), t.warning))
                } else if uses_or && self.openrouter_key.is_none() {
                    Some(("Add OpenRouter key".into(), t.warning))
                } else {
                    None
                };
                let model_for_launch = active_model.clone();
                let hover = t.hover;
                div()
                    .id(("picker-provider", pane as usize * 100 + i))
                    .flex()
                    .items_center()
                    .gap_2p5()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .border_1()
                    .border_color(if is_sel { t.accent } else { t.border_variant })
                    .bg(if is_sel { t.active } else { t.panel })
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let picker = this.agent_pickers.entry(pane).or_default();
                        if picker.selected == i && ready {
                            this.launch_agent(
                                pane,
                                ws_click.clone(),
                                id.clone(),
                                None,
                                model_for_launch.clone(),
                                window,
                                cx,
                            );
                        } else {
                            picker.selected = i;
                            picker.model = None;
                            // For OpenRouter providers, first click highlights them so you can pick a
                            // model from the strip below; second click (or Enter / Start) launches.
                            if ready && !uses_or {
                                this.launch_agent(pane, ws_click.clone(), id.clone(), None, None, window, cx);
                            }
                            cx.notify();
                        }
                    }))
                    .child(
                        div()
                            .flex_none()
                            .w(px(18.))
                            .h(px(18.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_sm()
                            .bg(t.editor)
                            .border_1()
                            .border_color(t.border_variant)
                            .font_family(mono())
                            .text_size(px(10.))
                            .text_color(t.text_placeholder)
                            .child(if i < 9 { format!("{}", i + 1) } else { "·".into() }),
                    )
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
                                    .child(div().text_sm().text_color(t.text).child(entry.label.clone()))
                                    .child(
                                        div()
                                            .px_1()
                                            .rounded_sm()
                                            .font_family(mono())
                                            .text_size(px(10.))
                                            .text_color(t.text_placeholder)
                                            .child(entry.short_badge.clone()),
                                    ),
                            )
                            .child(
                                div().text_xs().text_color(t.text_muted).truncate().child(entry.description.clone()),
                            ),
                    )
                    .when_some(status_badge, |d, (label, color)| {
                        d.child(div().flex_none().text_xs().text_color(color).child(label))
                    })
                    .into_any_element()
            })
            .collect();

        let extra: Option<AnyElement> = chosen.as_ref().and_then(|entry| {
            let base = agent_cli::rule_id(&entry.id);
            if !entry.installed && matches!(base, agent_cli::CLAUDE | agent_cli::AGY | agent_cli::GROK) {
                let busy = installing.as_deref() == Some(base);
                let id = entry.id.clone();
                return Some(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .pt_1()
                        .child(if busy {
                            div()
                                .flex()
                                .items_center()
                                .gap_1p5()
                                .text_xs()
                                .text_color(t.text_muted)
                                .child(spinner(("picker-install-spin", pane as usize), t.accent, px(12.)))
                                .child(format!("Installing {}…", entry.label))
                                .into_any_element()
                        } else {
                            primary_button(("picker-install", pane as usize), format!("Install {}", entry.label), cx)
                                .on_click(cx.listener(move |this, _, _, cx| this.install_provider_cli(id.clone(), cx)))
                                .into_any_element()
                        })
                        .into_any_element(),
                );
            }
            if entry.uses_openrouter && self.openrouter_key.is_none() {
                return Some(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .pt_1()
                        .child(
                            outline_button(("picker-add-or-key", pane as usize), "Add OpenRouter key in Settings…", cx)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.open_settings_on(super::app_settings::PROVIDERS_SECTION, cx)
                                })),
                        )
                        .into_any_element(),
                );
            }
            if entry.uses_openrouter {
                let mut models = self.saved.openrouter.models.clone();
                if entry.id == agent_cli::AGY_OPENROUTER
                    && !models.iter().any(|m| m == kynex_core::provider::DEFAULT_ANTIGRAVITY_MODEL)
                {
                    models.insert(0, kynex_core::provider::DEFAULT_ANTIGRAVITY_MODEL.to_string());
                }
                let current = active_model.clone().unwrap_or_default();
                let chips = models.into_iter().enumerate().map(|(mi, m)| {
                    let on = m == current;
                    let pick_m = m.clone();
                    ghost_button(("picker-model", pane as usize * 100 + mi), cx)
                        .h(px(22.))
                        .text_xs()
                        .font_family(mono())
                        .when(on, |b| b.bg(t.active).text_color(t.text))
                        .when(!on, |b| b.text_color(t.text_muted))
                        .child(m)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.agent_pickers.entry(pane).or_default().model = Some(pick_m.clone());
                            cx.notify();
                        }))
                });
                let launch_id = entry.id.clone();
                let launch_ws = ws_buf.clone();
                let launch_model = active_model.clone();
                return Some(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1p5()
                        .pt_2()
                        .border_t_1()
                        .border_color(t.border_variant)
                        .child(div().text_xs().text_color(t.text_muted).child("OpenRouter model"))
                        .child(div().flex().flex_wrap().gap_1().children(chips))
                        .child(
                            div().flex().pt_1().child(
                                primary_button(
                                    ("picker-start-or", pane as usize),
                                    format!("Start {}", entry.label),
                                    cx,
                                )
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.launch_agent(
                                        pane,
                                        launch_ws.clone(),
                                        launch_id.clone(),
                                        None,
                                        launch_model.clone(),
                                        window,
                                        cx,
                                    );
                                })),
                            ),
                        )
                        .into_any_element(),
                );
            }
            None
        });

        let ws_key = ws_buf.clone();
        let catalog_key = catalog.clone();
        let active_model_key = active_model.clone();
        div()
            .id(("agent-picker", pane as usize))
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .p_6()
            .bg(t.editor)
            .when(focused, |d| d.track_focus(&self.picker_focus))
            .on_key_down(cx.listener(move |this, ev: &KeyDownEvent, window, cx| {
                let k = &ev.keystroke;
                if k.modifiers.control || k.modifiers.platform || k.modifiers.alt {
                    return;
                }
                let len = catalog_key.len();
                if len == 0 {
                    return;
                }
                match k.key.as_str() {
                    "up" | "k" => {
                        let p = this.agent_pickers.entry(pane).or_default();
                        p.selected = p.selected.saturating_sub(1);
                        p.model = None;
                        cx.stop_propagation();
                        cx.notify();
                    }
                    "down" | "j" => {
                        let p = this.agent_pickers.entry(pane).or_default();
                        p.selected = (p.selected + 1).min(len - 1);
                        p.model = None;
                        cx.stop_propagation();
                        cx.notify();
                    }
                    "enter" => {
                        let idx = this.agent_pickers.get(&pane).map_or(0, |p| p.selected).min(len - 1);
                        if let Some(entry) = catalog_key.get(idx) {
                            let ready = entry.installed && (!entry.uses_openrouter || this.openrouter_key.is_some());
                            if ready {
                                let model = this
                                    .agent_pickers
                                    .get(&pane)
                                    .and_then(|p| p.model.clone())
                                    .or(active_model_key.clone());
                                this.launch_agent(pane, ws_key.clone(), entry.id.clone(), None, model, window, cx);
                            } else if entry.uses_openrouter && this.openrouter_key.is_none() {
                                this.open_settings_on(super::app_settings::PROVIDERS_SECTION, cx);
                            }
                        }
                        cx.stop_propagation();
                    }
                    d if d.len() == 1 && d.as_bytes()[0].is_ascii_digit() => {
                        let digit = (d.as_bytes()[0] - b'0') as usize;
                        if let Some(idx) = digit.checked_sub(1)
                            && let Some(entry) = catalog_key.get(idx)
                        {
                            let ready = entry.installed && (!entry.uses_openrouter || this.openrouter_key.is_some());
                            this.agent_pickers.entry(pane).or_default().selected = idx;
                            if ready {
                                this.launch_agent(pane, ws_key.clone(), entry.id.clone(), None, None, window, cx);
                            }
                            cx.stop_propagation();
                            cx.notify();
                        }
                    }
                    _ => {}
                }
            }))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .w_full()
                    .max_w(px(460.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(icon(Icon::Terminal, t.accent).size_4())
                            .child(div().text_base().font_weight(gpui::FontWeight::SEMIBOLD).child("New agent session"))
                            .child(div().flex_1())
                            .child(div().font_family(mono()).text_xs().text_color(t.text_placeholder).child(folder)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(t.text_muted)
                            .child("Choose a provider to launch — Claude Code opens in native chat; other CLIs open in a terminal pane (↑/↓ and Enter, or 1–9)."),
                    )
                    .child(div().flex().flex_col().gap_1p5().children(rows))
                    .children(extra),
            )
            .into_any_element()
    }
}
