//! The model picker's list: the account's own models from the CLI, with a short
//! built-in list until (or unless) that arrives. Every session reports the list
//! as it starts; the last one is saved, so after a restart it's there at once.
//! Which model a session runs on: its own, if one is pinned to it, else its
//! project's (Claude settings), on its provider.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui::{AnyElement, App, ClickEvent, Context, SharedString, Window, div, prelude::*};
use kynex_core::claude::model_groups;
use kynex_core::claude::session::{self, ModelInfo};
use kynex_core::provider::Provider;
use kynex_core::toasts::Toast;

use super::ClaudePane;
use super::{ClaudeChatEvent, SettingsPage};
use crate::theme::theme;
use crate::ui::{Icon, Tooltip, divider, icon, px};

/// Until the CLI's own list arrives (or if it can't be had).
const MODELS: [(&str, &str); 5] =
    [("", "Default"), ("opus", "Opus"), ("fable", "Fable"), ("sonnet", "Sonnet"), ("haiku", "Haiku")];

/// The models the account can use, as the CLI last reported them. Shared by
/// every pane, like the CLI's own list.
static MODEL_LIST: Mutex<Option<Vec<ModelInfo>>> = Mutex::new(None);
static MODELS_FETCHING: AtomicBool = AtomicBool::new(false);

fn known() -> Option<Vec<ModelInfo>> {
    MODEL_LIST.lock().ok()?.clone()
}

/// The list to offer: the CLI's, or the built-in one.
pub fn models() -> Vec<ModelInfo> {
    known().unwrap_or_else(|| {
        MODELS
            .iter()
            .map(|(id, name)| ModelInfo {
                value: if id.is_empty() { "default".into() } else { id.to_string() },
                name: name.to_string(),
                description: String::new(),
                efforts: Vec::new(),
            })
            .collect()
    })
}

/// The entry for a saved model choice (`""` is the CLI's default).
pub fn current_model(saved: &str) -> Option<ModelInfo> {
    let value = if saved.is_empty() { "default" } else { saved };
    known()?.into_iter().find(|m| m.value == value)
}

/// The composer's label for a saved choice. The default shows what it resolves
/// to, e.g. "Default (Opus 5.5)": the CLI's description of it starts with the model's name.
pub fn model_label(saved: &str) -> String {
    match current_model(saved) {
        Some(m) if saved.is_empty() => match m.description.split(" · ").next().filter(|s| !s.is_empty()) {
            Some(resolved) if m.description.contains(" · ") => format!("Default ({resolved})"),
            _ => "Default".to_string(),
        },
        Some(m) => m.name,
        None => MODELS.iter().find(|m| m.0 == saved).map_or(saved.to_string(), |m| m.1.to_string()),
    }
}

/// Loads the saved list once, or if there's none asks the CLI; repaints when it
/// lands. If both fail the built-in short list stays until a session reports one.
pub fn fetch_models(cx: &mut Context<ClaudePane>) {
    if MODELS_FETCHING.swap(true, Ordering::SeqCst) {
        return;
    }
    cx.spawn(async move |this, cx| {
        let result = cx
            .background_spawn(async {
                let saved = session::saved_models();
                if saved.is_empty() { session::list_models() } else { Ok(saved) }
            })
            .await;
        match result {
            // A session may have reported a fresher list meanwhile.
            Ok(list) => {
                if let Ok(mut known) = MODEL_LIST.lock() {
                    known.get_or_insert(list);
                }
            }
            Err(e) => eprintln!("kynex: couldn't list models: {e}"),
        }
        this.update(cx, |_, cx| cx.notify()).ok();
    })
    .detach();
}

/// A session's fresh list: shown from now on, and saved for the next start.
pub fn reported(list: Vec<ModelInfo>, cx: &mut Context<ClaudePane>) {
    if list.is_empty() || known().as_ref() == Some(&list) {
        return;
    }
    if let Ok(mut known) = MODEL_LIST.lock() {
        *known = Some(list.clone());
    }
    cx.background_spawn(async move {
        if let Err(e) = session::save_models(&list) {
            eprintln!("kynex: {e}");
        }
    })
    .detach();
}

/// An OpenRouter model as the composer shows it: the name after its vendor,
/// `qwen3-coder:free` for `qwen/qwen3-coder:free`.
pub fn openrouter_label(id: &str) -> String {
    id.split_once('/').map_or(id, |(_, name)| name).to_string()
}

/// The default's description with its price: it starts with the model it
/// resolves to ("Opus 5.5 · Best for everyday, complex tasks").
fn default_description(description: &str) -> String {
    let resolved = description.split(" · ").next().unwrap_or("");
    kynex_core::claude::prices::with_price(resolved, description)
}

/// Picking `value` in the model menu; a version pill's click stops at the pill.
fn pick(value: String, cx: &mut Context<ClaudePane>) -> impl Fn(&ClickEvent, &mut Window, &mut App) + use<> {
    let pane = cx.entity().downgrade();
    move |_, _, cx| {
        cx.stop_propagation();
        pane.update(cx, |this, cx| {
            this.menu = None;
            this.set_model(if value == "default" { String::new() } else { value.clone() }, cx);
        })
        .ok();
    }
}

impl ClaudePane {
    /// The model the session runs on: its own, if one is pinned to it, else its project's.
    pub(super) fn active_model(&self) -> String {
        self.session_model().unwrap_or_else(|| self.folder_model())
    }

    /// The model pinned to the session, if any.
    pub(super) fn session_model(&self) -> Option<String> {
        self.session.own_model.clone()
    }

    /// The model the project's sessions start on unless pinned otherwise, on
    /// this session's provider: on OpenRouter, the project's pick if it's still
    /// one of My models, else the default.
    pub(super) fn folder_model(&self) -> String {
        match self.session.provider().provider() {
            Provider::OpenRouter | Provider::OpenRouterChat => {
                let picked = &self.claude.openrouter_model;
                if self.config.openrouter.models.contains(picked) {
                    picked.clone()
                } else {
                    self.config.openrouter.default_model().to_string()
                }
            }
            _ => self.claude.model.clone(),
        }
    }

    /// The session's model, named as the composer shows it.
    pub(super) fn active_model_name(&self) -> String {
        let m = self.active_model();
        let m = if m.is_empty() { self.folder_model() } else { m };
        if self.session.provider().provider().is_openrouter() { openrouter_label(&m) } else { model_label(&m) }
    }

    /// Picks a model where it applies now: the session's own, if it has one,
    /// else its project's. A session starts on it from its next start.
    pub(super) fn set_model(&mut self, id: String, cx: &mut Context<Self>) {
        if self.session_model().is_some() {
            self.set_session_model(Some(id), cx);
        } else {
            self.set_project_model(id, cx);
        }
        cx.notify();
    }

    /// Pins a model to the session (`Some`), or has it follow its project's
    /// (`None`). Kept with the pane, so a restart comes back on it.
    pub(super) fn set_session_model(&mut self, model: Option<String>, cx: &mut Context<Self>) {
        let folder = self.folder_model();
        self.session.own_model = model.clone();
        self.session.use_model(model.clone().unwrap_or(folder));
        cx.emit(ClaudeChatEvent::ModelPinned(model));
        cx.notify();
    }

    /// Sets the project's model (on this session's provider). Its sessions
    /// without a pin start on it from their next start.
    fn set_project_model(&mut self, model: String, cx: &mut Context<Self>) {
        let openrouter = self.session.provider().provider().is_openrouter();
        let slot = if openrouter { &mut self.claude.openrouter_model } else { &mut self.claude.model };
        if *slot != model {
            *slot = model.clone();
            let dir = self.session.settings_folder().to_path_buf();
            if let Err(e) = self.claude.save(&dir) {
                self.toast(Toast::error(e), cx);
            }
            cx.emit(ClaudeChatEvent::ProjectSettingsChanged(dir));
        }
        if self.session.own_model.is_none() {
            self.session.use_model(model);
        }
    }

    /// The model menu's list: Default, the flagships (each family's newest,
    /// Fable first), then older versions as pills, a line per family.
    pub(super) fn model_items(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let t = theme(cx).clone();
        let current = self.active_model();
        let (default, families) = model_groups::group(&models());
        let mut items = Vec::new();
        let row = |id: SharedString, title: String, description: String, selected: bool| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap_2()
                .px_2()
                .py_1()
                .rounded_md()
                .cursor_pointer()
                .hover(move |s| s.bg(t.hover))
                .when(selected, |d| d.bg(t.active))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(div().text_sm().text_color(t.text).child(title))
                        .when(!description.is_empty(), |d| {
                            d.child(div().text_xs().text_color(t.text_muted).child(description))
                        }),
                )
        };
        if let Some(m) = default {
            let selected = current.is_empty();
            items.push(
                row("model-default".into(), m.name, default_description(&m.description), selected)
                    .when(selected, |d| d.child(icon(Icon::Check, t.text_muted)))
                    .on_click(pick(m.value, cx))
                    .into_any_element(),
            );
        }
        // The flagships: each family's newest, most capable family first.
        for (n, f) in families.iter().enumerate() {
            let Some(newest) = f.versions.first() else { continue };
            let selected = newest.value == current;
            let title = if newest.label.is_empty() { f.name.clone() } else { format!("{} {}", f.name, newest.label) };
            items.push(
                row(
                    format!("model-flagship-{n}").into(),
                    title.clone(),
                    kynex_core::claude::prices::with_price(&title, &newest.description),
                    selected,
                )
                .when(selected, |d| d.child(icon(Icon::Check, t.text_muted)))
                .on_click(pick(newest.value.clone(), cx))
                .into_any_element(),
            );
        }
        // The rest: a line per family, its older versions newest first.
        let older: Vec<_> = families.iter().filter(|f| f.versions.len() > 1).collect();
        if !older.is_empty() {
            items.push(div().my_1().child(divider(cx)).into_any_element());
            items.push(
                div()
                    .px_2()
                    .py_0p5()
                    .text_xs()
                    .text_color(t.text_placeholder)
                    .child("Older versions")
                    .into_any_element(),
            );
        }
        for (n, f) in older.into_iter().enumerate() {
            let pills = f.versions[1..].iter().enumerate().map(|(k, v)| {
                let on = v.value == current;
                div()
                    .id(SharedString::from(format!("model-version-{n}-{k}")))
                    .px(px(6.))
                    .py(px(1.))
                    .rounded_sm()
                    .border_1()
                    .text_xs()
                    .cursor_pointer()
                    .map(|d| {
                        if on {
                            d.bg(t.accent).border_color(t.accent).text_color(t.on_accent)
                        } else {
                            d.border_color(t.border_variant)
                                .text_color(t.text_muted)
                                .hover(move |s| s.bg(t.hover).text_color(t.text))
                        }
                    })
                    .tooltip(Tooltip::text(v.description.clone()))
                    .child(v.label.clone())
                    .on_click(pick(v.value.clone(), cx))
            });
            items.push(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_0p5()
                    .child(div().w(px(56.)).flex_none().text_sm().text_color(t.text_muted).child(f.name.clone()))
                    .child(div().flex().flex_wrap().gap_1().children(pills))
                    .into_any_element(),
            );
        }
        items
    }

    /// The model menu on OpenRouter: My models, and a way to change them. A
    /// model without tool calls can't run Claude Code, so it can't be picked.
    pub(super) fn openrouter_model_items(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let t = theme(cx).clone();
        let current = self.active_model();
        let current = if current.is_empty() { self.folder_model() } else { current };
        let config = self.config.openrouter.clone();
        let default = config.default_model().to_string();
        let mut items: Vec<AnyElement> = config
            .models
            .iter()
            .cloned()
            .enumerate()
            .map(|(n, id)| {
                let selected = id == current;
                let blocked = config.no_tools.contains(&id);
                let note = [(id.ends_with(":free"), "Free"), (id == default, "Default"), (blocked, "No tools")]
                    .into_iter()
                    .filter_map(|(on, n)| on.then_some(n))
                    .collect::<Vec<_>>()
                    .join(" · ");
                div()
                    .id(("openrouter-pick", n))
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .when(!blocked, |d| d.cursor_pointer().hover(move |s| s.bg(t.hover)))
                    .when(blocked, |d| d.opacity(0.5))
                    .when(selected, |d| d.bg(t.active))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(div().text_sm().text_color(t.text).child(openrouter_label(&id)))
                            .child(div().text_xs().text_color(t.text_muted).truncate().child(if note.is_empty() {
                                id.clone()
                            } else {
                                format!("{id} · {note}")
                            })),
                    )
                    .when(selected, |d| d.child(icon(Icon::Check, t.text_muted)))
                    .when(!blocked, |d| d.on_click(pick(id, cx)))
                    .into_any_element()
            })
            .collect();
        items.push(div().my_1().child(divider(cx)).into_any_element());
        items.push(
            div()
                .id("openrouter-manage")
                .px_2()
                .py_1()
                .rounded_md()
                .cursor_pointer()
                .text_sm()
                .text_color(t.text_muted)
                .hover(move |s| s.bg(t.hover).text_color(t.text))
                .child("Add or remove models in Settings → Providers…")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.menu = None;
                    cx.emit(ClaudeChatEvent::OpenSettings(SettingsPage::Providers));
                    cx.notify();
                }))
                .into_any_element(),
        );
        items
    }
}
