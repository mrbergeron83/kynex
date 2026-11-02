//! Providers on launch and for new sessions: the OpenRouter key loaded from the
//! keychain (or a setup made by hand moved into it), which provider a new
//! session starts on, and OpenRouter on the setup screen, so a user without an
//! Anthropic sign-in still gets in.

use std::path::Path;

use gpui::{AnyElement, Context, div, prelude::*};
use kynex_core::openrouter::{self, import};
use kynex_core::provider::{Launch, Provider};
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
        self.refresh_openrouter_credit(cx);
        self.refresh_openrouter_tools(cx);
    }

    /// Asks OpenRouter's catalogue which of My models take tool calls, once a
    /// key is there: a model that doesn't can only chat.
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

    /// A Work session's request failed because `model` takes no tool calls:
    /// remember it, so new sessions on it start in Chat, and offer to carry
    /// session `key` on in Chat.
    pub(super) fn model_without_tools(&mut self, key: kynex_core::live::Key, model: &str) {
        if self.saved.openrouter.mark_no_tools(model) {
            self.save();
            self.refresh_new_sessions();
        }
        let name = super::models::openrouter_label(model);
        self.toast(
            Toast::warning(format!("{name} can't work in Claude Code: it takes no tool calls"))
                .detail("It can still chat. New sessions on it start in Chat.")
                .session(key)
                .action(kynex_core::toasts::Action::ContinueInChat(key)),
        );
    }

    /// Moves session `key` to Chat, same model and conversation, and sends
    /// its last message again there.
    pub(super) fn continue_in_chat(&mut self, key: kynex_core::live::Key, cx: &mut Context<Self>) {
        let Some(s) = self.find_session_mut(key) else { return };
        if !s.move_to_chat() {
            return;
        }
        if let Some(id) = s.session_id.clone() {
            let cwd = s.cwd.clone();
            self.edit_groups(&cwd, |g| g.set_provider(&id, Provider::OpenRouterChat));
        }
        cx.notify();
    }

    /// Takes in which of My models take tool calls, from OpenRouter's catalogue.
    pub(super) fn learn_tools(&mut self, models: &[openrouter::CatalogModel]) {
        if self.saved.openrouter.learn_tools(models) {
            self.save();
            self.refresh_new_sessions();
        }
    }

    /// Asks OpenRouter what's left on the key, for the footer; at most every
    /// few seconds, however many turns end.
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

    /// Whether the CLI can't run Anthropic sessions: not signed in.
    pub(super) fn anthropic_signed_out(&self) -> bool {
        matches!(self.setup, Setup::LoggedOut { .. } | Setup::SigningIn(_))
    }

    /// The providers set up, as the provider picker lists them: Anthropic while
    /// the CLI is signed in, OpenRouter once it has a key (its Chat mode,
    /// [`Provider::OpenRouterChat`], is picked beside it in the composer).
    pub(super) fn providers_available(&self) -> Vec<Provider> {
        let mut all = Vec::new();
        if !self.anthropic_signed_out() {
            all.push(Provider::Anthropic);
        }
        if self.openrouter_key.is_some() {
            all.push(Provider::OpenRouter);
        }
        // Opt-in: only once turned on in Settings → Providers, and installed.
        if self.saved.grok && kynex_core::grok::installed() {
            all.push(Provider::Grok);
        }
        all
    }

    /// `provider` with what its sessions need, if it's set up.
    fn launch_for(&self, provider: Provider) -> Option<Launch> {
        match provider {
            Provider::Anthropic => Some(Launch::Anthropic),
            Provider::OpenRouter => self
                .openrouter_key
                .clone()
                .map(|key| Launch::OpenRouter { key, settings: self.saved.openrouter.settings(None) }),
            Provider::OpenRouterChat => self
                .openrouter_key
                .clone()
                .map(|key| Launch::OpenRouterChat { key, settings: self.saved.openrouter.settings(None) }),
            Provider::Grok => Some(Launch::Grok { model: String::new() }),
        }
    }

    /// The provider `dir`'s new sessions start on: its own pick if that's set
    /// up, else whichever is.
    pub(super) fn folder_provider(&self, dir: &Path) -> Provider {
        let picked = self.claude_settings(dir).provider;
        let available = self.providers_available();
        if available.contains(&picked.picker()) { picked } else { available.first().copied().unwrap_or_default() }
    }

    /// The provider `dir`'s next new session starts on: its folder's, but
    /// Chat on OpenRouter when the Work model takes no tools (Claude Code
    /// can't use it).
    pub(super) fn new_session_provider(&self, dir: &Path) -> Provider {
        match self.folder_provider(dir) {
            Provider::OpenRouter if !self.saved.openrouter.takes_tools(&self.folder_openrouter_model(dir)) => {
                Provider::OpenRouterChat
            }
            other => other,
        }
    }

    /// The model `dir`'s new Chat sessions start on: its own Chat pick if it's
    /// still one of My models, else the default.
    pub(super) fn folder_openrouter_chat_model(&self, dir: &Path) -> String {
        let picked = self.claude_settings(dir).openrouter_chat_model;
        if self.saved.openrouter.models.contains(&picked) {
            picked
        } else {
            self.saved.openrouter.default_model().to_string()
        }
    }

    /// The OpenRouter model `dir`'s new OpenRouter sessions start on: its own,
    /// if it's still one of My models, else their default.
    pub(super) fn folder_openrouter_model(&self, dir: &Path) -> String {
        let picked = self.claude_settings(dir).openrouter_model;
        if self.saved.openrouter.models.contains(&picked) {
            picked
        } else {
            self.saved.openrouter.default_model().to_string()
        }
    }

    /// The model `dir`'s sessions on `provider` start on.
    pub(super) fn folder_model_on(&self, dir: &Path, provider: Provider) -> String {
        match provider {
            Provider::Anthropic => self.dir_model(dir),
            Provider::OpenRouter => self.folder_openrouter_model(dir),
            Provider::OpenRouterChat => self.folder_openrouter_chat_model(dir),
            // Empty is no `--model`: the installed `grok` picks.
            Provider::Grok => self.claude_settings(dir).grok_model,
        }
    }

    /// The models `dir`'s sessions start on, one per provider in [`Provider::ALL`]'s order.
    fn folder_models(&self, dir: &Path) -> [String; 4] {
        Provider::ALL.map(|p| self.folder_model_on(dir, p))
    }

    /// The provider a session in `cwd` starts on: a reopened one (`id`) the
    /// provider it ran on, Anthropic if that isn't set up any more; a new one
    /// its folder's.
    fn start_launch(&mut self, cwd: &Path, id: Option<&str>) -> Launch {
        let provider = match id {
            Some(id) => self.groups_for(cwd).provider_of(id),
            None => self.new_session_provider(cwd),
        };
        self.launch_in(cwd, provider)
    }

    /// `provider`'s launch for a session in `cwd`, Anthropic if it isn't set
    /// up. KNX Ops works through tools, so it never gets the chat-only one.
    fn launch_in(&self, cwd: &Path, provider: Provider) -> Launch {
        let launch = self.launch_for(provider).unwrap_or(Launch::Anthropic);
        if cwd == kynex_core::paths::chief_dir() { launch.for_work() } else { launch }
    }

    /// Gives `s` the provider and model it starts on: see [`Self::start_launch`],
    /// and the model pinned to it, else its folder's for that provider.
    pub(super) fn start_session(&mut self, s: &mut Session) {
        let id = s.session_id.clone().filter(|_| !s.is_new());
        let (launch, models) = self.start_parts(&s.cwd, id.as_deref());
        apply_start(s, launch, &models);
    }

    /// What [`apply_start`] needs for a session in `cwd` (reopened if `id`).
    pub(super) fn start_parts(&mut self, cwd: &Path, id: Option<&str>) -> (Launch, [String; 4]) {
        let models = self.folder_models(cwd);
        (self.start_launch(cwd, id), models)
    }

    /// Re-picks the provider and model of every open session that hasn't
    /// started, after a sign-in, sign-out, key change or a project's pick.
    pub(super) fn refresh_new_sessions(&mut self) {
        let fresh: Vec<_> = self
            .code
            .values()
            .chain([&self.chat, &self.chief.live])
            .flat_map(|live| live.sessions.iter())
            .filter(|s| s.is_new())
            .map(|s| {
                let models = self.folder_models(&s.cwd);
                let launch = self.launch_in(&s.cwd, self.new_session_provider(&s.cwd));
                (s.uid, launch, models)
            })
            .collect();
        for (uid, launch, models) in fresh {
            if let Some(s) = self.find_session_mut(uid) {
                apply_start(s, launch, &models);
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
            Setup::LoggedOut { .. } => {
                self.openrouter_key.is_some() || (self.saved.grok && kynex_core::grok::installed())
            }
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
                        && !matches!(p, openrouter::Problem::RateLimited | openrouter::Problem::FreeDailyLimit)
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
                        this.refresh_openrouter_credit(cx);
                        this.refresh_openrouter_tools(cx);
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

/// Puts `s` on `launch` (restoring it if reopened) and its pinned model, else
/// `models` (one per provider, in [`Provider::ALL`]'s order) for the provider it ends up on.
pub(super) fn apply_start(s: &mut Session, launch: Launch, models: &[String; 4]) {
    let _ = if s.is_new() { s.set_provider(launch) } else { s.restore_provider(launch) };
    let at = Provider::ALL.iter().position(|p| *p == s.provider().provider()).unwrap_or(0);
    s.model = s.own_model.clone().unwrap_or_else(|| models[at].clone());
}

fn lowercase_first(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_lowercase().chain(c).collect()).unwrap_or_default()
}
