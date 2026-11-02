//! The model picker's list: the account's own models from the CLI, with a short
//! built-in list until (or unless) that arrives. Every session reports the list
//! as it starts; the last one is saved, so after a restart it's there at once.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui::{AnyElement, App, ClickEvent, Context, SharedString, Window, div, prelude::*};
use kynex_core::model_groups;
use kynex_core::session::{self, ModelInfo};

use super::KynexApp;
use crate::theme::theme;
use crate::ui::{Icon, Tooltip, divider, icon, px};

/// Until the CLI's own list arrives (or if it can't be had).
const MODELS: [(&str, &str); 5] =
    [("", "Default"), ("opus", "Opus"), ("fable", "Fable"), ("sonnet", "Sonnet"), ("haiku", "Haiku")];

/// The models the account can use, as the CLI last reported them. Kept in a
/// static rather than on `KynexApp` so it doesn't change the app's layout.
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

/// Lets a fetch that failed (say, while the CLI was missing) run again.
pub fn retry_fetch() {
    if known().is_none() {
        MODELS_FETCHING.store(false, Ordering::SeqCst);
    }
}

/// Loads the saved list once, or if there's none asks the CLI; repaints when it
/// lands. If both fail the built-in short list stays until a session reports one.
pub fn fetch_models(cx: &mut Context<KynexApp>) {
    if MODELS_FETCHING.swap(true, Ordering::SeqCst) {
        return;
    }
    cx.spawn(async move |this, cx| {
        let result = cx
            .background_spawn(async {
                // Grok's saved list too, unless a Grok session has reported one already.
                let grok = kynex_core::grok::saved_models();
                if !grok.is_empty()
                    && let Ok(mut known) = GROK_MODEL_LIST.lock()
                {
                    known.get_or_insert(grok);
                }
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

impl KynexApp {
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
                    kynex_core::prices::with_price(&title, &newest.description),
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
}

/// How the composer and menu name "no `--model`" on Grok.
pub(super) const GROK_DEFAULT_LABEL: &str = "Grok default";

/// Grok's models, as a Grok session last reported them (saved across restarts).
static GROK_MODEL_LIST: Mutex<Option<Vec<ModelInfo>>> = Mutex::new(None);

fn grok_known() -> Vec<ModelInfo> {
    GROK_MODEL_LIST.lock().ok().and_then(|l| l.clone()).unwrap_or_default()
}

/// The Grok model `id`, if Grok has said which it has; `""` is the first, Grok's default.
pub fn grok_model(id: &str) -> Option<ModelInfo> {
    let list = grok_known();
    if id.is_empty() { list.into_iter().next() } else { list.into_iter().find(|m| m.value == id) }
}

/// A Grok session's model list came: kept for the picker, and saved.
pub fn grok_reported(list: Vec<ModelInfo>, cx: &mut Context<KynexApp>) {
    if list.is_empty() || grok_known() == list {
        return;
    }
    if let Ok(mut known) = GROK_MODEL_LIST.lock() {
        *known = Some(list.clone());
    }
    cx.background_spawn(async move {
        if let Err(e) = kynex_core::grok::save_models(&list) {
            eprintln!("kynex: {e}");
        }
    })
    .detach();
}

impl KynexApp {
    /// The model menu on Grok: its default, then the models Grok offers (once a
    /// Grok session has said), or the id this folder already uses until then.
    pub(super) fn grok_model_items(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let t = theme(cx).clone();
        let current = self.active_model();
        // "default" is no `--model`: whatever the installed `grok` picks.
        let mut rows = vec![("default".to_string(), GROK_DEFAULT_LABEL.to_string(), String::new())];
        rows.extend(grok_known().into_iter().map(|m| (m.value, m.name, m.description)));
        if !current.is_empty() && !rows.iter().any(|r| r.0 == current) {
            rows.push((current.clone(), current.clone(), String::new()));
        }
        rows.into_iter()
            .enumerate()
            .map(|(n, (id, name, description))| {
                let selected = if current.is_empty() { id == "default" } else { id == current };
                div()
                    .id(("grok-pick", n))
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
                            .child(div().text_sm().text_color(t.text).child(name))
                            .when(!description.is_empty(), |d| {
                                d.child(div().text_xs().text_color(t.text_muted).child(description))
                            }),
                    )
                    .when(selected, |d| d.child(icon(Icon::Check, t.text_muted)))
                    .on_click(pick(id, cx))
                    .into_any_element()
            })
            .collect()
    }

    /// The model menu on OpenRouter: My models, and a way to change them.
    pub(super) fn openrouter_model_items(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let t = theme(cx).clone();
        let current = self.active_model();
        let current = if current.is_empty() { self.folder_model() } else { current };
        let default = self.saved.openrouter.default_model().to_string();
        // On Work, a model without tool calls can't run Claude Code: picking
        // one on a new session switches it to Chat; a started one can't take it.
        let work = self.active_provider() == kynex_core::provider::Provider::OpenRouter;
        let fresh = self.active_session().is_none_or(kynex_core::session::Session::is_new);
        let no_tools: Vec<String> = self.saved.openrouter.no_tools.clone();
        let mut items: Vec<AnyElement> = self
            .saved
            .openrouter
            .models
            .clone()
            .into_iter()
            .enumerate()
            .map(|(n, id)| {
                let selected = id == current;
                let chat_only = no_tools.contains(&id);
                let blocked = work && chat_only && !fresh;
                let note = [
                    (id.ends_with(":free"), "Free"),
                    (id == default, "Default"),
                    (chat_only, if work { "No tools: switches to Chat" } else { "No tools" }),
                ]
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
                    .when(work && chat_only && !blocked, |d| {
                        let id = id.clone();
                        d.on_click(cx.listener(move |this, _, _, cx| {
                            this.menu = None;
                            this.set_project_openrouter_chat_model(this.active_dir(), id.clone(), cx);
                            this.set_provider(kynex_core::provider::Provider::OpenRouterChat, cx);
                        }))
                    })
                    .when(!chat_only || !work, |d| d.on_click(pick(id, cx)))
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
                    this.open_settings_on(super::app_settings::PROVIDERS_SECTION, cx);
                }))
                .into_any_element(),
        );
        items
    }
}

/// An OpenRouter model as the composer shows it: the name after its vendor,
/// `qwen3-coder:free` for `qwen/qwen3-coder:free`.
pub fn openrouter_label(id: &str) -> String {
    id.split_once('/').map_or(id, |(_, name)| name).to_string()
}

/// Picking `value` in the model menu; a version pill's click stops at the pill.
fn pick(value: String, cx: &mut Context<KynexApp>) -> impl Fn(&ClickEvent, &mut Window, &mut App) + use<> {
    let app = cx.entity().downgrade();
    move |_, _, cx| {
        cx.stop_propagation();
        app.update(cx, |this, cx| {
            this.menu = None;
            this.set_model(if value == "default" { String::new() } else { value.clone() }, cx);
        })
        .ok();
    }
}

/// A session's fresh list: shown from now on, and saved for the next start.
pub fn reported(list: Vec<ModelInfo>, cx: &mut Context<KynexApp>) {
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

/// The default's description with its price: it starts with the model it
/// resolves to ("Opus 5.5 · Best for everyday, complex tasks").
fn default_description(description: &str) -> String {
    let resolved = description.split(" · ").next().unwrap_or("");
    kynex_core::prices::with_price(resolved, description)
}
