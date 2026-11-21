//! OpenRouter key loading from the OS keychain, credit checks, and model catalogue sync
//! for the `claude-openrouter` and `agy-openrouter` overrides.

use gpui::{AppContext, Context};
use kynex_core::openrouter::{self, import};
use kynex_core::secrets::{Keychain, Secret};
use kynex_core::toasts::Toast;

use super::KynexApp;

fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

enum Loaded {
    Key(Option<String>),
    Imported {
        found: import::Found,
        moved: Result<(), String>,
        check: Result<openrouter::KeyInfo, openrouter::Problem>,
    },
    NoKeychain(String),
}

impl KynexApp {
    /// Loads the OpenRouter key on launch, importing a manual setup unless removed in Settings.
    pub(super) fn load_openrouter(&mut self, cx: &mut Context<Self>) {
        let config = self.saved.openrouter.clone();
        cx.spawn(async move |this, cx| {
            let loaded = cx
                .background_spawn(async move {
                    let keychain = Keychain::os();
                    let saved = keychain.as_ref().map_err(Clone::clone).and_then(|k| k.get(Secret::OpenRouterKey));
                    match saved {
                        Ok(Some(key)) => return Loaded::Key(Some(key)),
                        Err(e) if config.key_removed => return Loaded::NoKeychain(e),
                        _ if config.key_removed => return Loaded::Key(None),
                        _ => {}
                    }
                    let Some(found) = import::find_existing() else {
                        return match saved {
                            Err(e) => Loaded::NoKeychain(e),
                            _ => Loaded::Key(None),
                        };
                    };
                    let moved = keychain.and_then(|k| k.set(Secret::OpenRouterKey, &found.key));
                    let model = found.model.clone().unwrap_or_else(|| config.default_model().to_string());
                    let check = openrouter::check(&found.key, &model);
                    Loaded::Imported { found, moved, check }
                })
                .await;
            this.update(cx, |this, cx| {
                this.openrouter_loaded(loaded, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn openrouter_loaded(&mut self, loaded: Loaded, cx: &mut Context<Self>) {
        match loaded {
            Loaded::Key(key) => self.openrouter_key = key,
            Loaded::NoKeychain(e) => eprintln!("kynex: no keychain for the OpenRouter key: {e}"),
            Loaded::Imported { found, moved, check } => {
                if let Some(model) = &found.model
                    && self.saved.openrouter.add_model(model).is_ok()
                {
                    self.saved.openrouter.set_default(model);
                }
                let model = self.saved.openrouter.default_model().to_string();
                self.saved.openrouter.record_check(&model, &check, unix_now());
                self.save();
                let toast = match (&moved, &check) {
                    (Ok(()), Ok(_)) => Toast::info(format!(
                        "Found your OpenRouter setup in {} and moved the key into the keychain. \
                         It's in Settings → Providers.",
                        found.source.label()
                    )),
                    (Ok(()), Err(p)) => Toast::error(format!(
                        "Found your OpenRouter setup in {} and moved the key into the keychain, but {}",
                        found.source.label(),
                        lowercase_first(&p.message())
                    )),
                    (Err(e), _) => Toast::error(format!(
                        "Found an OpenRouter key in {}, but couldn't keep it: {e} It works until Kynex closes.",
                        found.source.label()
                    )),
                };
                self.toast(toast);
                self.openrouter_key = Some(found.key);
            }
        }
        self.refresh_provider_catalog(cx);
        self.refresh_openrouter_credit(cx);
        self.refresh_openrouter_tools(cx);
    }

    /// Asks OpenRouter's catalogue which of My models take tool calls.
    pub(super) fn refresh_openrouter_tools(&mut self, cx: &mut Context<Self>) {
        if self.openrouter_key.is_none() {
            return;
        }
        cx.spawn(async move |this, cx| {
            let found = cx.background_spawn(async { openrouter::catalogue() }).await;
            this.update(cx, |this, cx| match found {
                Ok(models) => {
                    this.learn_tools(&models);
                    cx.notify();
                }
                Err(e) => eprintln!("kynex: couldn't read OpenRouter's models: {e}"),
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn learn_tools(&mut self, models: &[openrouter::CatalogModel]) {
        if self.saved.openrouter.learn_tools(models) {
            self.save();
        }
    }

    /// Asks OpenRouter what's left on the key, for the footer; throttled to every 10s.
    pub(super) fn refresh_openrouter_credit(&mut self, cx: &mut Context<Self>) {
        const MIN_GAP: std::time::Duration = std::time::Duration::from_secs(10);
        let Some(key) = self.openrouter_key.clone() else {
            self.openrouter_credit = None;
            return;
        };
        let recent = self.openrouter_credit.as_ref().is_some_and(|(_, at)| at.elapsed() < MIN_GAP);
        if self.openrouter_credit_asking || recent {
            return;
        }
        self.openrouter_credit_asking = true;
        cx.spawn(async move |this, cx| {
            let credit = cx.background_spawn(async move { openrouter::credit(&key) }).await;
            this.update(cx, |this, cx| {
                this.openrouter_credit_asking = false;
                match credit {
                    Ok(info) => this.openrouter_credit = Some((info, std::time::Instant::now())),
                    Err(e) => eprintln!("kynex: couldn't read the OpenRouter credit: {e}"),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

fn lowercase_first(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_lowercase().chain(c).collect()).unwrap_or_default()
}
