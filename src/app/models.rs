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
                            d.child(div().text_xs().text_color(t.text_muted).truncate().child(description))
                        }),
                )
        };
        if let Some(m) = default {
            let selected = current.is_empty();
            items.push(
                row("model-default".into(), m.name, m.description, selected)
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
                row(format!("model-flagship-{n}").into(), title, newest.description.clone(), selected)
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
