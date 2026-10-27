//! The model picker's list: the account's own models from the CLI, with a short
//! built-in list until (or unless) that arrives. Every session reports the list
//! as it starts; the last one is saved, so after a restart it's there at once.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui::{Context, prelude::*};
use kynex_core::session::{self, ModelInfo};

use super::KynexApp;

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
