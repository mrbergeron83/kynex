//! Providers on launch and for new sessions: the OpenRouter key loaded from the
//! keychain (or a setup made by hand moved into it), which provider a new
//! session starts on, and OpenRouter on the setup screen, so a user without an
//! Anthropic sign-in still gets in.

use gpui::{AnyElement, Context, div, prelude::*};
use kynex_core::openrouter::{self, import};
use kynex_core::provider::Launch;
use kynex_core::secrets::{Keychain, Secret};
use kynex_core::session::Session;
use kynex_core::toasts::Toast;

use super::KynexApp;
use super::setup::Setup;
use crate::theme::Palette;
use crate::ui::primary_button;

/// What launch found for OpenRouter.
enum Loaded {
    /// A key in the keychain, or none there and no setup to bring in.
    Key(Option<String>),
    /// A setup made by hand, the key now in the keychain (`moved`) or, with no
    /// keychain, kept for this run only; and how its check went.
    Imported {
        found: import::Found,
        moved: Result<(), String>,
        check: Result<openrouter::KeyInfo, openrouter::Problem>,
    },
    /// No keychain to read.
    NoKeychain(String),
}

impl KynexApp {
    /// Loads the OpenRouter key on launch, bringing in a setup made by hand
    /// (`OPENROUTER_API_KEY`, `ANTHROPIC_*` for OpenRouter, an env block in
    /// `~/.claude/settings.json`) unless the user removed the key in Settings.
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
                this.openrouter_loaded(loaded);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn openrouter_loaded(&mut self, loaded: Loaded) {
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
                self.saved.openrouter.record_check(&model, &check, kynex_core::usage::now());
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
        self.refresh_new_sessions();
    }

    /// Whether the CLI can't run Anthropic sessions: not signed in.
    fn anthropic_signed_out(&self) -> bool {
        matches!(self.setup, Setup::LoggedOut { .. } | Setup::SigningIn(_))
    }

    /// The provider a new session starts on: Anthropic, or OpenRouter when
    /// that's the only one set up.
    pub(super) fn new_session_launch(&self) -> Launch {
        match &self.openrouter_key {
            Some(key) if self.anthropic_signed_out() => {
                Launch::OpenRouter { key: key.clone(), settings: self.saved.openrouter.settings(None) }
            }
            _ => Launch::Anthropic,
        }
    }

    /// Gives a session that hasn't started the provider new sessions get. On
    /// OpenRouter the model is left to the provider's own (`ANTHROPIC_MODEL`).
    pub(super) fn give_provider(&self, s: &mut Session) {
        if s.set_provider(self.new_session_launch()).is_ok() && !matches!(s.provider(), Launch::Anthropic) {
            s.model.clear();
        }
    }

    /// Re-picks the provider (and model) of every open session that hasn't
    /// started, after a sign-in, sign-out or key change.
    pub(super) fn refresh_new_sessions(&mut self) {
        let launch = self.new_session_launch();
        let mut fresh: Vec<&mut Session> = self
            .code
            .values_mut()
            .chain([&mut self.chat, &mut self.chief.live])
            .flat_map(|live| live.sessions.iter_mut())
            .filter(|s| s.session_id.is_none() && s.items.is_empty() && !s.is_running())
            .collect();
        let mut models = Vec::new();
        for s in &mut fresh {
            if s.set_provider(launch.clone()).is_ok() {
                models.push((s.uid, s.cwd.clone(), s.own_model.clone()));
            }
        }
        let on_openrouter = !matches!(launch, Launch::Anthropic);
        for (uid, cwd, own) in models {
            let model = if on_openrouter { String::new() } else { own.unwrap_or_else(|| self.dir_model(&cwd)) };
            if let Some(s) = self.find_session_mut(uid) {
                s.model = model;
            }
        }
    }

    fn find_session_mut(&mut self, uid: u64) -> Option<&mut Session> {
        self.code
            .values_mut()
            .chain([&mut self.chat, &mut self.chief.live])
            .flat_map(|live| live.sessions.iter_mut())
            .find(|s| s.uid == uid)
    }

    /// The setup screen gives way to the workspace: the CLI is ready, or it's
    /// only signed out of Anthropic and OpenRouter has a key.
    pub(super) fn workspace_ready(&self) -> bool {
        match self.setup {
            Setup::Checking | Setup::Ready => true,
            Setup::LoggedOut { .. } => self.openrouter_key.is_some(),
            _ => false,
        }
    }

    /// Checks a key typed on the setup screen and keeps it only if OpenRouter
    /// takes it; then the workspace opens on OpenRouter.
    pub(super) fn setup_openrouter_key(&mut self, cx: &mut Context<Self>) {
        let key = self.setup_key.read(cx).text().trim().to_string();
        if key.is_empty() || self.setup_key_busy {
            return;
        }
        self.setup_key_busy = true;
        self.setup_key_error = None;
        let model = self.saved.openrouter.default_model().to_string();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let check = openrouter::check(&key, &model);
                    // A rate limit still means the key is good.
                    if let Err(p) = &check
                        && *p != openrouter::Problem::RateLimited
                    {
                        return Err(p.message());
                    }
                    Keychain::os().and_then(|k| k.set(Secret::OpenRouterKey, &key))?;
                    Ok((key, check))
                })
                .await;
            this.update(cx, |this, cx| {
                this.setup_key_busy = false;
                match result {
                    Ok((key, check)) => {
                        let model = this.saved.openrouter.default_model().to_string();
                        this.saved.openrouter.record_check(&model, &check, kynex_core::usage::now());
                        this.saved.openrouter.key_removed = false;
                        this.save();
                        this.openrouter_key = Some(key);
                        this.setup_key.update(cx, |input, cx| input.set_text("", cx));
                        this.refresh_new_sessions();
                    }
                    Err(e) => this.setup_key_error = Some(e),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// OpenRouter on the setup screen, beside signing in to Anthropic.
    pub(super) fn render_setup_openrouter(&self, t: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let field = div()
            .flex_1()
            .min_w_0()
            .px_2()
            .py_1()
            .rounded_md()
            .border_1()
            .border_color(t.border)
            .bg(t.editor)
            .text_sm()
            .child(self.setup_key.clone());
        let button = if self.setup_key_busy {
            primary_button("setup-openrouter", "Checking…", cx).opacity(0.6)
        } else {
            primary_button("setup-openrouter", "Use OpenRouter", cx)
                .on_click(cx.listener(|this, _, _, cx| this.setup_openrouter_key(cx)))
        };
        div()
            .flex()
            .flex_col()
            .gap_2()
            .pt_3()
            .border_t_1()
            .border_color(t.border_variant)
            .child(div().text_sm().child("Or use OpenRouter"))
            .child(div().text_xs().text_color(t.text_muted).child(
                "Run sessions on other models with your own OpenRouter key, kept in the OS keychain. \
                 Change it, the output cap and your models later in Settings → Providers.",
            ))
            .child(div().flex().items_center().gap_2().child(field).child(button))
            .children(self.setup_key_error.clone().map(|e| div().text_xs().text_color(t.error).child(e)))
            .into_any_element()
    }
}

fn lowercase_first(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_lowercase().chain(c).collect()).unwrap_or_default()
}
