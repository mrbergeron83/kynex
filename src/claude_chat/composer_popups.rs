//! Autocomplete popups (`/` slash commands & skills, `@` fuzzy file picker) and
//! toolbar popovers (Model, Effort/Thinking, Permission Mode) docked cleanly
//! above the composer.

use gpui::{AnyElement, Context, Focusable, FontWeight, SharedString, Window, div, prelude::*};
use kynex_core::claude::composer::{CommandItem, ComposerTrigger, active_trigger, match_commands};
use kynex_core::claude::{EffortLevel, PermissionMode};

use super::state::{CompletionPopup, OpenPopover};
use super::{ClaudeChatView, centered_column};
use crate::theme::{mono, theme, ui_font};
use crate::ui::{Icon, icon, menu_item, popover, px, scrollbar};

const MAX_POPUP_ITEMS: usize = 10;

impl ClaudeChatView {
    /// Refreshes the active `/` or `@` completion popup based on current composer text and cursor.
    pub(super) fn refresh_completion_popup(&mut self, cx: &mut Context<Self>) {
        if self.refresh_history_matches(cx) {
            return;
        }
        let (text, cursor) = {
            let inp = self.composer_input.read(cx);
            (inp.text().to_string(), inp.cursor_offset())
        };
        let Some(trigger) = active_trigger(&text, cursor) else {
            self.completion = None;
            return;
        };
        match trigger {
            ComposerTrigger::Slash { start, query } => {
                let items = match_commands(&self.slash_commands, &query, MAX_POPUP_ITEMS);
                if items.is_empty() {
                    self.completion = None;
                } else {
                    self.completion = Some(CompletionPopup::Slash { start, end: cursor, items, selected: 0 });
                }
            }
            ComposerTrigger::AtFile { start, query } => {
                let items = self.file_index.search(&query, MAX_POPUP_ITEMS);
                if items.is_empty() {
                    self.completion = None;
                } else {
                    self.completion = Some(CompletionPopup::AtFile { start, end: cursor, items, selected: 0 });
                }
            }
        }
    }

    /// Moves the selection inside the active completion popup (`delta`: -1 or +1).
    pub(super) fn step_completion(&mut self, delta: isize, cx: &mut Context<Self>) -> bool {
        let Some(popup) = &mut self.completion else { return false };
        let len = match popup {
            CompletionPopup::Slash { items, .. } => items.len(),
            CompletionPopup::AtFile { items, .. } => items.len(),
            CompletionPopup::History { items, .. } => items.len(),
        };
        if len == 0 {
            return false;
        }
        let sel = match popup {
            CompletionPopup::Slash { selected, .. }
            | CompletionPopup::AtFile { selected, .. }
            | CompletionPopup::History { selected, .. } => selected,
        };
        *sel = (*sel as isize + delta).rem_euclid(len as isize) as usize;
        cx.notify();
        true
    }

    /// Accepts the currently highlighted `/` command or `@` file completion.
    pub(super) fn accept_completion(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(popup) = self.completion.take() else { return false };
        match popup {
            CompletionPopup::Slash { start, end, items, selected } => {
                let Some(hit) = items.get(selected) else { return false };
                let Some(cmd) = self.slash_commands.get(hit.index) else { return false };
                let replacement = format!("/{} ", cmd.name);
                self.composer_input.update(cx, |inp, cx| {
                    inp.replace_range(start..end, &replacement, window, cx);
                });
                self.sync_composer_marks(cx);
                cx.notify();
                true
            }
            CompletionPopup::AtFile { start, end, items, selected } => {
                let Some(hit) = items.get(selected) else { return false };
                let replacement = format!("@{} ", hit.path);
                self.composer_input.update(cx, |inp, cx| {
                    inp.replace_range(start..end, &replacement, window, cx);
                });
                self.sync_composer_marks(cx);
                cx.notify();
                true
            }
            CompletionPopup::History { items, selected, draft } => {
                let text = items.get(selected).cloned().unwrap_or(draft);
                self.set_composer(text, cx);
                true
            }
        }
    }

    /// Renders the active `/` or `@` completion popup above the composer.
    pub(super) fn render_completion_popup(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let popup = self.completion.as_ref()?;
        let t = theme(cx).clone();
        let scroll = scrollbar::handle("claude-completion-sb");

        let (header, rows): (&'static str, Vec<AnyElement>) = match popup {
            CompletionPopup::Slash { start, end, items, selected } => {
                let (start, end, selected) = (*start, *end, *selected);
                let rows = items
                    .iter()
                    .enumerate()
                    .filter_map(|(i, m)| {
                        let cmd = self.slash_commands.get(m.index)?;
                        let is_sel = i == selected;
                        let name = cmd.name.clone();
                        let hint = cmd.argument_hint.clone();
                        let desc = cmd.description.clone();
                        Some(
                            div()
                                .id(SharedString::from(format!("slash-opt-{i}")))
                                .flex()
                                .items_center()
                                .gap_2()
                                .h(px(28.))
                                .px_2()
                                .rounded_md()
                                .cursor_pointer()
                                .when(is_sel, |d| d.bg(t.selection))
                                .when(!is_sel, |d| d.hover(|s| s.bg(t.hover)))
                                .child(
                                    div()
                                        .flex_none()
                                        .font_family(mono())
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_xs()
                                        .text_color(t.accent)
                                        .child(format!("/{name}")),
                                )
                                .when_some(hint, |d, h| {
                                    d.child(
                                        div()
                                            .flex_none()
                                            .font_family(mono())
                                            .text_size(px(11.))
                                            .text_color(t.text_placeholder)
                                            .child(h),
                                    )
                                })
                                .child(
                                    div().flex_1().min_w_0().truncate().text_xs().text_color(t.text_muted).child(desc),
                                )
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.completion = None;
                                    let repl = format!("/{name} ");
                                    this.composer_input.update(cx, |inp, cx| {
                                        inp.replace_range(start..end, &repl, window, cx);
                                    });
                                    this.sync_composer_marks(cx);
                                    this.composer_input.read(cx).focus_handle(cx).focus(window, cx);
                                    cx.notify();
                                }))
                                .into_any_element(),
                        )
                    })
                    .collect();
                ("Commands & skills", rows)
            }
            CompletionPopup::AtFile { start, end, items, selected } => {
                let (start, end, selected) = (*start, *end, *selected);
                let rows = items
                    .iter()
                    .enumerate()
                    .map(|(i, m)| {
                        let is_sel = i == selected;
                        let path = m.path.clone();
                        let (dir, file) = match path.rsplit_once('/') {
                            Some((d, f)) => (Some(d.to_string()), f.to_string()),
                            None => (None, path.clone()),
                        };
                        div()
                            .id(SharedString::from(format!("at-file-opt-{i}")))
                            .flex()
                            .items_center()
                            .gap_2()
                            .h(px(26.))
                            .px_2()
                            .rounded_md()
                            .cursor_pointer()
                            .when(is_sel, |d| d.bg(t.selection))
                            .when(!is_sel, |d| d.hover(|s| s.bg(t.hover)))
                            .child(icon(Icon::File, t.text_muted).size(px(12.)))
                            .child(
                                div()
                                    .flex_none()
                                    .font_family(mono())
                                    .text_xs()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(t.text)
                                    .child(file),
                            )
                            .when_some(dir, |d, parent| {
                                d.child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .font_family(mono())
                                        .text_size(px(11.))
                                        .text_color(t.text_placeholder)
                                        .child(parent),
                                )
                            })
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.completion = None;
                                let repl = format!("@{path} ");
                                this.composer_input.update(cx, |inp, cx| {
                                    inp.replace_range(start..end, &repl, window, cx);
                                });
                                this.sync_composer_marks(cx);
                                this.composer_input.read(cx).focus_handle(cx).focus(window, cx);
                                cx.notify();
                            }))
                            .into_any_element()
                    })
                    .collect();
                ("Workspace files", rows)
            }
            CompletionPopup::History { items, selected, .. } => {
                let selected = *selected;
                let rows = items
                    .iter()
                    .enumerate()
                    .map(|(i, text)| {
                        let is_sel = i == selected;
                        let pick = text.clone();
                        let line = text.lines().next().unwrap_or("").to_string();
                        div()
                            .id(SharedString::from(format!("history-opt-{i}")))
                            .flex()
                            .items_center()
                            .h(px(26.))
                            .px_2()
                            .rounded_md()
                            .cursor_pointer()
                            .when(is_sel, |d| d.bg(t.selection))
                            .when(!is_sel, |d| d.hover(|s| s.bg(t.hover)))
                            .child(div().flex_1().min_w_0().truncate().text_xs().text_color(t.text).child(line))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.completion = None;
                                this.set_composer(pick.clone(), cx);
                                this.composer_input.read(cx).focus_handle(cx).focus(window, cx);
                            }))
                            .into_any_element()
                    })
                    .collect();
                ("Earlier messages (Enter to use, Esc to cancel)", rows)
            }
        };

        if rows.is_empty() {
            return None;
        }

        Some(
            div()
                .flex_none()
                .px_4()
                .mb_1()
                .child(centered_column(
                    popover(cx)
                        .w_full()
                        .font_family(ui_font())
                        .child(
                            div()
                                .px_2()
                                .py_1()
                                .text_size(px(10.5))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(t.text_placeholder)
                                .child(header),
                        )
                        .child(scrollbar::vertical_over(
                            "claude-completion-sbar",
                            scroll.clone(),
                            div()
                                .id("claude-completion-list")
                                .max_h(px(240.))
                                .overflow_y_scroll()
                                .track_scroll(&scroll)
                                .flex()
                                .flex_col()
                                .children(rows),
                            |w| w.w_full(),
                        )),
                ))
                .into_any_element(),
        )
    }

    /// Renders the open toolbar popover (Model, Effort, or Permission Mode) above the composer.
    pub(super) fn render_toolbar_popover(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let which = self.open_popover?;
        let t = theme(cx).clone();
        let scroll = scrollbar::handle("claude-popover-sb");

        let (title, rows): (&'static str, Vec<AnyElement>) = match which {
            OpenPopover::Model => {
                let rows = if self.is_openrouter() {
                    self.config
                        .openrouter_models
                        .iter()
                        .enumerate()
                        .map(|(i, m)| {
                            let selected = self.active_model.as_deref() == Some(m.as_str());
                            let val = m.clone();
                            menu_item(("or-model", i), m.clone(), selected, cx)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.set_model(val.clone(), cx);
                                    this.open_popover = None;
                                    cx.notify();
                                }))
                                .into_any_element()
                        })
                        .collect()
                } else {
                    let models = self.init_info.as_ref().map(|i| i.models.as_slice()).unwrap_or(&[]);
                    models
                        .iter()
                        .enumerate()
                        .map(|(i, m)| {
                            let selected = self
                                .active_model
                                .as_deref()
                                .is_some_and(|a| a == m.value || m.resolved_model.as_deref() == Some(a));
                            let val = m.value.clone();
                            let label =
                                if m.display_name.is_empty() { m.value.clone() } else { m.display_name.clone() };
                            let desc = m.description.clone();
                            div()
                                .id(("claude-model-opt", i))
                                .flex()
                                .items_center()
                                .gap_2()
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .cursor_pointer()
                                .when(selected, |d| d.bg(t.active))
                                .when(!selected, |d| d.hover(|s| s.bg(t.hover)))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .flex()
                                        .flex_col()
                                        .child(
                                            div()
                                                .text_xs()
                                                .font_weight(FontWeight::MEDIUM)
                                                .text_color(t.text)
                                                .child(label),
                                        )
                                        .when(!desc.is_empty(), |d| {
                                            d.child(
                                                div()
                                                    .truncate()
                                                    .text_size(px(11.))
                                                    .text_color(t.text_muted)
                                                    .child(desc),
                                            )
                                        }),
                                )
                                .when(selected, |d| d.child(icon(Icon::Check, t.accent).size(px(13.))))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.set_model(val.clone(), cx);
                                    this.open_popover = None;
                                    cx.notify();
                                }))
                                .into_any_element()
                        })
                        .collect()
                };
                ("Model", rows)
            }
            OpenPopover::Effort => {
                let levels = active_model_supported_efforts(self);
                let rows = levels
                    .into_iter()
                    .enumerate()
                    .map(|(i, level)| {
                        let selected = self.effort == level;
                        menu_item(("effort-opt", i), level.label(), selected, cx)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.set_effort(level, cx);
                                this.open_popover = None;
                                cx.notify();
                            }))
                            .into_any_element()
                    })
                    .collect();
                ("Thinking effort", rows)
            }
            OpenPopover::Mode => {
                let supports_auto = self.active_model_supports_auto();
                let mut modes = vec![PermissionMode::Default, PermissionMode::AcceptEdits];
                if supports_auto {
                    modes.push(PermissionMode::Auto);
                }
                modes.push(PermissionMode::Plan);
                modes.push(PermissionMode::DontAsk);
                modes.push(PermissionMode::BypassPermissions);

                let rows = modes
                    .into_iter()
                    .enumerate()
                    .map(|(i, mode)| {
                        let selected = self.permission_mode == mode;
                        menu_item(("mode-opt", i), mode.label(), selected, cx)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.set_permission_mode(mode, cx);
                                this.open_popover = None;
                                cx.notify();
                            }))
                            .into_any_element()
                    })
                    .collect();
                ("Permission mode (Shift+Tab)", rows)
            }
        };

        if rows.is_empty() {
            return None;
        }

        Some(
            div()
                .flex_none()
                .px_4()
                .mb_1()
                .child(centered_column(
                    popover(cx)
                        .max_w(px(360.))
                        .font_family(ui_font())
                        .child(
                            div()
                                .px_2()
                                .py_1()
                                .text_size(px(10.5))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(t.text_placeholder)
                                .child(title),
                        )
                        .child(scrollbar::vertical_over(
                            "claude-popover-sbar",
                            scroll.clone(),
                            div()
                                .id("claude-popover-list")
                                .max_h(px(260.))
                                .overflow_y_scroll()
                                .track_scroll(&scroll)
                                .flex()
                                .flex_col()
                                .children(rows),
                            |w| w.w_full(),
                        )),
                ))
                .into_any_element(),
        )
    }

    /// Sets effort level live on the running CLI without restarting.
    pub(super) fn set_effort(&mut self, effort: EffortLevel, cx: &mut Context<Self>) {
        self.effort = effort;
        if let Some(client) = &self.client {
            let _ = client.set_effort(effort);
        }
        cx.notify();
    }

    pub(super) fn set_model(&mut self, model: String, cx: &mut Context<Self>) {
        self.active_model = Some(model.clone());
        if let Some(client) = &self.client {
            let _ = client.set_model(&model);
        }
        cx.notify();
    }

    pub(super) fn set_permission_mode(&mut self, mode: PermissionMode, cx: &mut Context<Self>) {
        self.permission_mode = mode;
        if let Some(client) = &self.client {
            let _ = client.set_permission_mode(mode);
        }
        cx.notify();
    }

    pub(super) fn toggle_fast_mode(&mut self, cx: &mut Context<Self>) {
        self.fast_mode = !self.fast_mode;
        if let Some(client) = &self.client {
            let _ = client.set_fast_mode(self.fast_mode);
        }
        cx.notify();
    }

    pub(super) fn toggle_popover(&mut self, which: OpenPopover, cx: &mut Context<Self>) {
        self.open_popover = if self.open_popover == Some(which) { None } else { Some(which) };
        cx.notify();
    }

    pub(super) fn active_model_display_name(&self) -> String {
        let Some(active) = self.active_model.as_deref() else {
            return "Default model".into();
        };
        if let Some(info) = &self.init_info
            && let Some(m) =
                info.models.iter().find(|m| m.value == active || m.resolved_model.as_deref() == Some(active))
            && !m.display_name.is_empty()
        {
            return m.display_name.clone();
        }
        active.rsplit('/').next().unwrap_or(active).to_string()
    }

    pub(super) fn active_model_supports_auto(&self) -> bool {
        if self.is_openrouter() {
            return false;
        }
        let Some(info) = &self.init_info else { return false };
        let active = self.active_model.as_deref();
        active
            .and_then(|a| info.models.iter().find(|m| m.value == a || m.resolved_model.as_deref() == Some(a)))
            .or_else(|| info.models.first())
            .is_some_and(|m| m.supports_auto_mode)
    }

    pub(super) fn active_model_supports_fast(&self) -> bool {
        if self.is_openrouter() {
            return false;
        }
        let Some(info) = &self.init_info else { return false };
        let active = self.active_model.as_deref();
        active
            .and_then(|a| info.models.iter().find(|m| m.value == a || m.resolved_model.as_deref() == Some(a)))
            .or_else(|| info.models.first())
            .is_some_and(|m| m.supports_fast_mode)
    }
}

pub(super) fn active_model_supported_efforts(view: &ClaudeChatView) -> Vec<EffortLevel> {
    if view.is_openrouter() {
        return Vec::new();
    }
    let Some(info) = &view.init_info else { return Vec::new() };
    let active = view.active_model.as_deref();
    let model = active
        .and_then(|a| info.models.iter().find(|m| m.value == a || m.resolved_model.as_deref() == Some(a)))
        .or_else(|| info.models.first());
    let Some(m) = model else { return Vec::new() };
    if !m.supports_effort {
        return Vec::new();
    }
    if m.supported_effort_levels.is_empty() { EffortLevel::ALL.to_vec() } else { m.supported_effort_levels.clone() }
}

pub(super) fn default_slash_commands() -> Vec<CommandItem> {
    vec![
        CommandItem {
            name: "fork".into(),
            description: "Fork this session into a new side-by-side pane".into(),
            argument_hint: None,
            aliases: vec!["branch".into()],
        },
        CommandItem {
            name: "clear".into(),
            description: "Clear the visible transcript".into(),
            argument_hint: None,
            aliases: vec!["reset".into()],
        },
        CommandItem {
            name: "compact".into(),
            description: "Compact conversation history".into(),
            argument_hint: Some("[instructions]".into()),
            aliases: vec![],
        },
    ]
}
