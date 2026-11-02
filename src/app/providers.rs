//! Settings → Providers: who answers sessions. Anthropic is the CLI's own
//! sign-in; OpenRouter is a key in the OS keychain, checked against
//! OpenRouter, an output cap and My models, the models the menus offer.
//! Everything saves at once; new sessions use it with no restart.

use crate::ui::px;
use gpui::{AnyElement, Context, Entity, Focusable, div, prelude::*};
use kynex_core::cli;
use kynex_core::openrouter::{self, CatalogModel, OpenRouterConfig};
use kynex_core::secrets::{Keychain, Secret};

use super::KynexApp;
use super::settings::{heading, radio};
use crate::theme::{Palette, mono};
use crate::ui::input::{InputEvent, TextInput};
use crate::ui::{Icon, Tooltip, icon, icon_button, outline_button, primary_button};

const KEYS_URL: &str = "https://openrouter.ai/settings/keys";

/// The Providers section's state, made when Settings opens.
#[derive(Clone)]
pub struct ProvidersView {
    /// Who the CLI is signed in as, once asked.
    account: Option<Result<cli::Account, String>>,
    key: KeyState,
    /// Typing a new key over a saved one.
    replacing: bool,
    key_input: Entity<TextInput>,
    cap_input: Entity<TextInput>,
    /// Searches the catalogue, or takes a model ID typed out.
    add_input: Entity<TextInput>,
    /// OpenRouter's models, fetched the first time a search is typed.
    catalogue: Option<Result<Vec<CatalogModel>, String>>,
    /// What's underway: "Checking the key…".
    busy: Option<&'static str>,
    error: Option<String>,
    /// Provider variables in `~/.claude/settings.json`'s env block, which may
    /// override what Kynex sets, in words; read when Settings opens.
    overrides: Option<String>,
    /// Grok: opted in or not, installed, signed in (`grok_setup.rs`).
    pub(super) grok: super::grok_setup::GrokView,
}

#[derive(Clone)]
enum KeyState {
    Loading,
    Missing,
    /// Saved; its hint (`sk-or-v1-…3f9a`), never the key.
    Saved(String),
    /// No keychain to keep it in, and why.
    NoKeychain(String),
}

impl KynexApp {
    /// The Providers section's state, and what it shows looked up off the UI thread.
    pub(super) fn new_providers_view(&mut self, cx: &mut Context<Self>) -> ProvidersView {
        let key_input = cx.new(|cx| TextInput::new(cx).mono().masked().placeholder("Paste your OpenRouter key"));
        cx.subscribe(&key_input, |this, _, ev, cx| {
            if let InputEvent::Submit = ev {
                this.save_openrouter_key(cx);
            }
        })
        .detach();
        let cap = self.saved.openrouter.max_output_tokens.to_string();
        let cap_input = cx.new(|cx| {
            let mut input = TextInput::new(cx).mono().placeholder("8000");
            input.set_text(cap, cx);
            input
        });
        cx.subscribe(&cap_input, |this, _, ev, cx| {
            if let InputEvent::Submit = ev {
                this.save_output_cap(cx);
            }
        })
        .detach();
        let add_input = cx.new(|cx| TextInput::new(cx).mono().placeholder("Search OpenRouter's models, or type an ID"));
        cx.subscribe(&add_input, |this, input, ev, cx| match ev {
            InputEvent::Changed => {
                if !input.read(cx).text().trim().is_empty() {
                    this.load_catalogue(cx);
                }
                cx.notify();
            }
            InputEvent::Submit => {
                let id = input.read(cx).text().trim().to_string();
                this.add_openrouter_model(&id, cx);
            }
            _ => {}
        })
        .detach();
        self.load_account(cx);
        self.load_openrouter_key(cx);
        self.load_settings_overrides(cx);
        if self.saved.grok {
            self.load_grok_status(cx);
        }
        ProvidersView {
            grok: Default::default(),
            account: None,
            key: KeyState::Loading,
            replacing: false,
            key_input,
            cap_input,
            add_input,
            catalogue: None,
            busy: None,
            error: None,
            overrides: None,
        }
    }

    fn providers_mut(&mut self) -> Option<&mut ProvidersView> {
        self.app_settings.as_mut().map(|s| &mut s.providers)
    }

    fn load_settings_overrides(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let names = cx.background_spawn(async { kynex_core::provider::user_settings_overrides() }).await;
            this.update(cx, |this, cx| {
                if let Some(p) = this.providers_mut() {
                    p.overrides = kynex_core::provider::overrides_warning(&names);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn load_account(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let account = cx.background_spawn(async { cli::account() }).await;
            this.update(cx, |this, cx| {
                if let Some(p) = this.providers_mut() {
                    p.account = Some(account);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn load_openrouter_key(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let key = cx
                .background_spawn(async {
                    match Keychain::os().and_then(|k| k.get(Secret::OpenRouterKey)) {
                        Ok(Some(key)) => KeyState::Saved(openrouter::key_hint(&key)),
                        Ok(None) => KeyState::Missing,
                        Err(e) => KeyState::NoKeychain(e),
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                if let Some(p) = this.providers_mut() {
                    p.key = key;
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Fetches OpenRouter's catalogue once per Settings opening.
    fn load_catalogue(&mut self, cx: &mut Context<Self>) {
        let Some(p) = self.providers_mut() else { return };
        if p.catalogue.is_some() {
            return;
        }
        p.catalogue = Some(Ok(Vec::new()));
        cx.spawn(async move |this, cx| {
            let found = cx.background_spawn(async { openrouter::catalogue() }).await;
            this.update(cx, |this, cx| {
                if let Ok(models) = &found {
                    this.learn_tools(models);
                }
                if let Some(p) = this.providers_mut() {
                    p.catalogue = Some(found);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Runs `job` off the UI thread with `label` showing, then `done` with its result.
    fn providers_job<T: Send + 'static>(
        &mut self,
        label: &'static str,
        job: impl FnOnce() -> T + Send + 'static,
        done: impl FnOnce(&mut Self, T, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) {
        let Some(p) = self.providers_mut() else { return };
        if p.busy.is_some() {
            return;
        }
        p.busy = Some(label);
        p.error = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { job() }).await;
            this.update(cx, |this, cx| {
                if let Some(p) = this.providers_mut() {
                    p.busy = None;
                }
                done(this, result, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Saves the typed key in the keychain, then checks it.
    fn save_openrouter_key(&mut self, cx: &mut Context<Self>) {
        let Some(p) = self.providers_mut() else { return };
        let key = p.key_input.read(cx).text().trim().to_string();
        if key.is_empty() {
            return;
        }
        let model = self.saved.openrouter.default_model().to_string();
        self.providers_job(
            "Saving and checking the key…",
            move || {
                Keychain::os().and_then(|k| k.set(Secret::OpenRouterKey, &key))?;
                Ok::<_, String>((key.clone(), openrouter::check(&key, &model)))
            },
            move |this, saved, cx| match saved {
                Ok((key, check)) => {
                    let hint = openrouter::key_hint(&key);
                    this.openrouter_key = Some(key);
                    this.saved.openrouter.key_removed = false;
                    this.refresh_new_sessions();
                    this.openrouter_credit = None;
                    this.refresh_openrouter_credit(cx);
                    if let Some(p) = this.providers_mut() {
                        p.key = KeyState::Saved(hint);
                        p.replacing = false;
                        p.key_input.update(cx, |input, cx| input.set_text("", cx));
                    }
                    this.record_openrouter_check(check);
                }
                Err(e) => this.providers_error(e),
            },
            cx,
        );
    }

    /// Checks the saved key against the default model.
    fn check_openrouter_key(&mut self, cx: &mut Context<Self>) {
        let model = self.saved.openrouter.default_model().to_string();
        self.providers_job(
            "Checking the key…",
            move || {
                let key = Keychain::os()?.get(Secret::OpenRouterKey)?.ok_or("No OpenRouter key is saved.")?;
                Ok::<_, String>(openrouter::check(&key, &model))
            },
            |this, checked, _| match checked {
                Ok(check) => this.record_openrouter_check(check),
                Err(e) => this.providers_error(e),
            },
            cx,
        );
    }

    fn remove_openrouter_key(&mut self, cx: &mut Context<Self>) {
        self.providers_job(
            "Removing the key…",
            || Keychain::os().and_then(|k| k.remove(Secret::OpenRouterKey)),
            |this, removed, _| match removed {
                Ok(()) => {
                    if let Some(p) = this.providers_mut() {
                        p.key = KeyState::Missing;
                        p.replacing = false;
                    }
                    this.saved.openrouter.last_check = None;
                    this.saved.openrouter.key_removed = true;
                    this.openrouter_key = None;
                    this.openrouter_credit = None;
                    this.refresh_new_sessions();
                    this.save();
                }
                Err(e) => this.providers_error(e),
            },
            cx,
        );
    }

    fn record_openrouter_check(&mut self, check: Result<openrouter::KeyInfo, openrouter::Problem>) {
        let model = self.saved.openrouter.default_model().to_string();
        self.saved.openrouter.record_check(&model, &check, kynex_core::usage::now());
        self.save();
    }

    fn providers_error(&mut self, e: String) {
        if let Some(p) = self.providers_mut() {
            p.error = Some(e);
        }
    }

    /// Changes the OpenRouter settings and saves them, or shows why not.
    fn edit_openrouter(
        &mut self,
        cx: &mut Context<Self>,
        edit: impl FnOnce(&mut OpenRouterConfig) -> Result<(), String>,
    ) -> bool {
        let result = edit(&mut self.saved.openrouter);
        if let Some(p) = self.providers_mut() {
            p.error = result.as_ref().err().cloned();
        }
        if result.is_ok() {
            self.save();
        }
        cx.notify();
        result.is_ok()
    }

    fn save_output_cap(&mut self, cx: &mut Context<Self>) {
        let Some(p) = self.providers_mut() else { return };
        let typed = p.cap_input.read(cx).text().to_string();
        if self.edit_openrouter(cx, |c| c.set_max_output_tokens(&typed)) {
            let cap = self.saved.openrouter.max_output_tokens.to_string();
            if let Some(p) = self.providers_mut() {
                p.cap_input.update(cx, |input, cx| input.set_text(cap, cx));
            }
        }
    }

    fn add_openrouter_model(&mut self, id: &str, cx: &mut Context<Self>) {
        if id.is_empty() {
            return;
        }
        if !self.edit_openrouter(cx, |c| c.add_model(id)) {
            return;
        }
        let catalogue = self.providers_mut().map(|p| {
            p.add_input.update(cx, |input, cx| input.set_text("", cx));
            p.catalogue.clone()
        });
        // Whether it takes tools: from the catalogue it was found in, from the
        // one still loading once it's in, else asked for now.
        match catalogue.flatten() {
            Some(Ok(models)) if !models.is_empty() => self.learn_tools(&models),
            Some(Ok(_)) => {}
            _ => self.refresh_openrouter_tools(cx),
        }
    }

    /// Signs the CLI out; the setup screen then offers signing in again.
    fn sign_out(&mut self, cx: &mut Context<Self>) {
        self.providers_job(
            "Signing out…",
            cli::logout,
            |this, out, cx| match out {
                Ok(()) => {
                    this.load_account(cx);
                    this.check_setup(cx);
                }
                Err(e) => this.providers_error(e),
            },
            cx,
        );
    }

    pub(super) fn render_providers(&self, t: &Palette, cx: &mut Context<Self>) -> gpui::Div {
        let Some(p) = self.app_settings.as_ref().map(|s| &s.providers) else { return div() };
        let overrides = p.overrides.clone().map(|warning| {
            div()
                .flex()
                .items_start()
                .gap_2()
                .px_2()
                .py_1p5()
                .rounded_md()
                .border_1()
                .border_color(t.warning)
                .text_xs()
                .text_color(t.text_muted)
                .child(div().mt(px(1.)).child(icon(Icon::Shield, t.warning).size_3()))
                .child(div().flex_1().min_w_0().child(warning))
        });
        div()
            .flex()
            .flex_col()
            .gap_6()
            .children(overrides)
            .child(self.render_anthropic(p, t, cx))
            .child(self.render_openrouter_key(p, t, cx))
            .child(self.render_output_cap(p, t, cx))
            .child(self.render_my_models(p, t, cx))
            .child(self.render_grok(t, cx))
    }

    fn render_anthropic(&self, p: &ProvidersView, t: &Palette, cx: &mut Context<Self>) -> gpui::Div {
        let (line, color) = match &p.account {
            None => ("Checking…".to_string(), t.text_placeholder),
            Some(Ok(a)) => (a.describe(), t.text_muted),
            Some(Err(e)) => (e.clone(), t.error),
        };
        let account = p.account.as_ref().and_then(|a| a.as_ref().ok());
        let can_sign_out = account.is_some_and(|a| a.logged_in && a.other_provider.is_none());
        let can_sign_in = account.is_some_and(|a| !a.logged_in && a.other_provider.is_none());
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(heading("Anthropic", "Claude Code's own sign-in: your Claude plan or Anthropic Console account.", t))
            .child(boxed(t).child(div().text_sm().text_color(color).child(line)))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .when(can_sign_in, |d| {
                        d.child(primary_button("anthropic-sign-in", "Sign in", cx).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.app_settings = None;
                                this.sign_in(false, cx);
                            },
                        )))
                    })
                    .when(can_sign_out, |d| {
                        d.child(
                            outline_button("anthropic-sign-out", "Sign out", cx)
                                .tooltip(Tooltip::text("Anthropic sessions stop working until you sign in again"))
                                .on_click(cx.listener(|this, _, _, cx| this.sign_out(cx))),
                        )
                    }),
            )
    }

    fn render_openrouter_key(&self, p: &ProvidersView, t: &Palette, cx: &mut Context<Self>) -> gpui::Div {
        let entry = |cancel: bool, cx: &mut Context<Self>| {
            div().flex().flex_col().gap_2().child(input_box(p.key_input.clone(), t)).child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        primary_button("openrouter-save-key", "Save and check", cx)
                            .on_click(cx.listener(|this, _, _, cx| this.save_openrouter_key(cx))),
                    )
                    .when(cancel, |d| {
                        d.child(outline_button("openrouter-cancel-key", "Cancel", cx).on_click(cx.listener(
                            |this, _, _, cx| {
                                if let Some(p) = this.providers_mut() {
                                    p.replacing = false;
                                    p.key_input.update(cx, |input, cx| input.set_text("", cx));
                                }
                                cx.notify();
                            },
                        )))
                    })
                    .child(
                        div()
                            .id("openrouter-get-key")
                            .text_xs()
                            .text_color(t.accent)
                            .cursor_pointer()
                            .hover(|s| s.underline())
                            .child("Get a key at openrouter.ai")
                            .on_click(|_, _, cx| cx.open_url(KEYS_URL)),
                    ),
            )
        };
        let body: AnyElement = match &p.key {
            KeyState::Loading => muted("Looking in the keychain…", t).into_any_element(),
            KeyState::NoKeychain(e) => div().text_sm().text_color(t.error).child(e.clone()).into_any_element(),
            KeyState::Missing => entry(false, cx).into_any_element(),
            KeyState::Saved(_) if p.replacing => entry(true, cx).into_any_element(),
            KeyState::Saved(hint) => div()
                .flex()
                .flex_col()
                .gap_2()
                .child(boxed(t).child(div().font_family(mono()).text_sm().child(hint.clone())))
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(
                            outline_button("openrouter-check", "Check", cx)
                                .on_click(cx.listener(|this, _, _, cx| this.check_openrouter_key(cx))),
                        )
                        .child(outline_button("openrouter-replace", "Replace", cx).on_click(cx.listener(
                            |this, _, window, cx| {
                                if let Some(p) = this.providers_mut() {
                                    p.replacing = true;
                                    let focus = p.key_input.read(cx).focus_handle(cx);
                                    window.focus(&focus, cx);
                                }
                                cx.notify();
                            },
                        )))
                        .child(
                            outline_button("openrouter-remove", "Remove", cx)
                                .on_click(cx.listener(|this, _, _, cx| this.remove_openrouter_key(cx))),
                        ),
                )
                .into_any_element(),
        };
        let last =
            self.saved.openrouter.last_check.as_ref().map(|c| {
                let (mark, color) = if c.ok { (Icon::Check, t.success) } else { (Icon::Close, t.error) };
                div()
                    .flex()
                    .items_start()
                    .gap_1p5()
                    .text_xs()
                    .child(div().mt(px(1.)).child(icon(mark, color).size_3()))
                    .child(div().flex_1().min_w_0().text_color(if c.ok { t.text_muted } else { t.error }).child(
                        format!("{} · checked {} with {}", c.message, kynex_core::fmt::ago_unix(c.at as i64), c.model),
                    ))
            });
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(heading(
                "OpenRouter",
                "Run sessions on other models through openrouter.ai with your own key. It's kept in the OS keychain, \
                 never in Kynex's files.",
                t,
            ))
            .child(body)
            .children(last)
            .children(p.busy.map(|b| muted(b, t)))
            .children(p.error.clone().map(|e| div().text_xs().text_color(t.error).child(e)))
    }

    fn render_output_cap(&self, p: &ProvidersView, t: &Palette, cx: &mut Context<Self>) -> gpui::Div {
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(heading(
                "Output cap",
                "The most tokens one OpenRouter reply may use. Lower it if OpenRouter says you're out of credits (402).",
                t,
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().w(px(120.)).child(input_box(p.cap_input.clone(), t)))
                    .child(muted("tokens", t))
                    .child(
                        outline_button("openrouter-save-cap", "Save", cx)
                            .on_click(cx.listener(|this, _, _, cx| this.save_output_cap(cx))),
                    ),
            )
    }

    fn render_my_models(&self, p: &ProvidersView, t: &Palette, cx: &mut Context<Self>) -> gpui::Div {
        let config = &self.saved.openrouter;
        let default = config.default_model().to_string();
        let mine = config.models.iter().enumerate().map(|(n, id)| {
            let (pick, drop) = (id.clone(), id.clone());
            let hover = t.hover;
            div()
                .id(("openrouter-model", n))
                .flex()
                .items_center()
                .gap_2()
                .px_2()
                .py_1()
                .rounded_md()
                .cursor_pointer()
                .hover(move |s| s.bg(hover))
                .tooltip(Tooltip::text("New OpenRouter sessions start on the default"))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.edit_openrouter(cx, |c| {
                        c.set_default(&pick);
                        Ok(())
                    });
                }))
                .child(radio(*id == default, t))
                .child(div().flex_1().min_w_0().truncate().font_family(mono()).text_sm().child(id.clone()))
                .when(id.ends_with(":free"), |d| d.child(tag("Free", t)))
                .when(*id == default, |d| d.child(tag("Default", t)))
                .child(
                    icon_button(("openrouter-model-remove", n), Icon::Trash, cx)
                        .tooltip(Tooltip::text("Remove from My models"))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.edit_openrouter(cx, |c| c.remove_model(&drop));
                        })),
                )
        });
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(heading(
                "My models",
                "The OpenRouter models the model menus offer. Click one to make it the default.",
                t,
            ))
            .child(div().flex().flex_col().gap_0p5().children(mine))
            .child(input_box(p.add_input.clone(), t))
            .children(self.render_model_results(p, t, cx))
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap_2()
                    .px_2()
                    .py_1p5()
                    .rounded_md()
                    .border_1()
                    .border_color(t.border_variant)
                    .text_xs()
                    .text_color(t.text_muted)
                    .child(div().mt(px(1.)).child(icon(Icon::Shield, t.warning).size_3()))
                    .child(div().flex_1().min_w_0().child(
                        "Weaker and free models may struggle with Claude Code's file edits and tool use, and keep their \
                         own safety training. Free models are rate-limited and shared, so expect 429s at busy times.",
                    )),
            )
    }

    /// What the search finds, with a row to add a typed ID the catalogue doesn't list.
    fn render_model_results(&self, p: &ProvidersView, t: &Palette, cx: &mut Context<Self>) -> Option<gpui::Div> {
        let query = p.add_input.read(cx).text().trim().to_string();
        if query.is_empty() {
            return None;
        }
        let mine = &self.saved.openrouter.models;
        let list = div().flex().flex_col().gap_0p5().max_h(px(280.)).overflow_hidden();
        let found: Vec<&CatalogModel> = match &p.catalogue {
            Some(Ok(all)) if all.is_empty() => return Some(list.child(muted("Loading OpenRouter's models…", t))),
            Some(Ok(all)) => openrouter::search(all, &query),
            Some(Err(e)) => return Some(list.child(div().text_xs().text_color(t.error).child(e.clone()))),
            None => Vec::new(),
        };
        let typed = (openrouter::valid_model_id(&query) && !found.iter().any(|m| m.id == query)).then(|| {
            let id = query.clone();
            result_row(("openrouter-add-typed", 0usize), t)
                .on_click(cx.listener(move |this, _, _, cx| this.add_openrouter_model(&id, cx)))
                .child(icon(Icon::Plus, t.text_muted).size_3())
                .child(div().font_family(mono()).text_sm().child(format!("Add {query}")))
        });
        let rows = found.into_iter().enumerate().map(|(n, m)| {
            let added = mine.contains(&m.id);
            let id = m.id.clone();
            let detail = [Some(m.price()), m.context_label()].into_iter().flatten().collect::<Vec<_>>().join(" · ");
            result_row(("openrouter-result", n), t)
                .when(!added, |d| d.on_click(cx.listener(move |this, _, _, cx| this.add_openrouter_model(&id, cx))))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(div().truncate().font_family(mono()).text_sm().child(m.id.clone()))
                        .child(
                            div().truncate().text_xs().text_color(t.text_muted).child(format!("{} · {detail}", m.name)),
                        ),
                )
                .when(!m.tools, |d| d.child(tag("No tools", t)))
                .when(m.is_free(), |d| d.child(tag("Free", t)))
                .when(added, |d| d.child(tag("Added", t)))
        });
        let empty = matches!(&p.catalogue, Some(Ok(_))) && typed.is_none() && rows.len() == 0;
        Some(
            list.children(typed).children(rows).when(empty, |d| {
                d.child(muted("No OpenRouter model matches. Type a full ID like qwen/qwen3-coder.", t))
            }),
        )
    }
}

fn boxed(t: &Palette) -> gpui::Div {
    div().px_2().py_1p5().rounded_md().border_1().border_color(t.border_variant).bg(t.editor)
}

fn input_box(input: Entity<TextInput>, t: &Palette) -> gpui::Div {
    div().px_2().py_1().rounded_md().border_1().border_color(t.border).bg(t.editor).text_sm().child(input)
}

fn muted(text: &'static str, t: &Palette) -> gpui::Div {
    div().text_xs().text_color(t.text_placeholder).child(text)
}

fn tag(text: &'static str, t: &Palette) -> gpui::Div {
    div()
        .flex_none()
        .px_1p5()
        .rounded_sm()
        .border_1()
        .border_color(t.border_variant)
        .text_xs()
        .text_color(t.text_muted)
        .child(text)
}

fn result_row(id: impl Into<gpui::ElementId>, t: &Palette) -> gpui::Stateful<gpui::Div> {
    let hover = t.hover;
    div().id(id).flex().items_center().gap_2().px_2().py_1().rounded_md().cursor_pointer().hover(move |s| s.bg(hover))
}
