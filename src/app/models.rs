//! The model picker's list: the account's own models from the CLI, with a short
//! built-in list until (or unless) that arrives.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui::{Context, prelude::*};
use kynex_core::session::{self, ModelInfo};

use super::KynexApp;

/// Until the CLI's own list arrives (or if it can't be had).
const MODELS: [(&str, &str); 5] =
    [("", "Default"), ("opus", "Opus"), ("fable", "Fable"), ("sonnet", "Sonnet"), ("haiku", "Haiku")];

/// The models the account can use, fetched once from the CLI. Kept in a static
/// rather than on `KynexApp` so it doesn't change the app's layout.
static MODEL_LIST: OnceLock<Vec<ModelInfo>> = OnceLock::new();
static MODELS_FETCHING: AtomicBool = AtomicBool::new(false);

/// The list to offer: the CLI's, or the built-in one.
pub fn models() -> Vec<ModelInfo> {
    MODEL_LIST.get().cloned().unwrap_or_else(|| {
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
    MODEL_LIST.get()?.iter().find(|m| m.value == value).cloned()
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
    if MODEL_LIST.get().is_none() {
        MODELS_FETCHING.store(false, Ordering::SeqCst);
    }
}

/// Starts the one background fetch of the model list; repaints when it lands.
/// If it fails the built-in short list stays.
pub fn fetch_models(cx: &mut Context<KynexApp>) {
    if MODELS_FETCHING.swap(true, Ordering::SeqCst) {
        return;
    }
    cx.spawn(async move |this, cx| {
        let result = cx.background_spawn(async { session::list_models() }).await;
        match result {
            Ok(list) if !list.is_empty() => {
                let _ = MODEL_LIST.set(list);
            }
            Ok(_) => eprintln!("kynex: claude reported no models"),
            Err(e) => eprintln!("kynex: couldn't list models: {e}"),
        }
        this.update(cx, |_, cx| cx.notify()).ok();
    })
    .detach();
}
