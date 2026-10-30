//! The Extensions modal's plugins: what each installed one adds, turning it on
//! or off, updating and removing it, and browsing the added marketplaces to
//! install more. Every change runs `claude plugin`; an install that runs a
//! command the marketplace declares shows that command and waits for a yes.

use crate::ui::px;
use gpui::{AnyElement, Context, FontWeight, div, prelude::*};
use kynex_core::extensions::Plugin;
use kynex_core::plugins::{self, Action, Outcome};

use super::KynexApp;
use super::extensions::{Market, PendingCommand, card, chip, status_lines};
use super::settings::{heading, radio};
use crate::theme::{Palette, mono};
use crate::ui::input::{InputEvent, TextInput};
use crate::ui::{Tooltip, outline_button, primary_button, spinner};

/// How many offered plugins show at once; the filter finds the rest.
const SHOWN: usize = 40;

impl KynexApp {
    /// Runs `action` on plugin `id` through the CLI, then reads everything
    /// again. `accept` is a marketplace command's sha256 the person agreed to.
    fn change_plugin(
        &mut self,
        action: Action,
        id: String,
        scope: String,
        accept: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(view) = self.extensions.as_mut().filter(|v| v.busy.is_none()) else { return };
        let verb = match action {
            Action::Enable => "Turning on",
            Action::Disable => "Turning off",
            Action::Update => "Updating",
            Action::Uninstall => "Uninstalling",
            Action::Install => "Installing",
        };
        view.busy = Some(format!("{verb} {}…", id.split('@').next().unwrap_or(&id)));
        view.error = None;
        view.confirm_plugin = None;
        view.confirm_command = None;
        let cwd = view.cwd.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn({
                    let (cwd, id, scope) = (cwd.clone(), id.clone(), scope.clone());
                    async move { plugins::change(&cwd, action, &id, &scope, accept.as_deref()) }
                })
                .await;
            this.update(cx, |this, cx| {
                let Some(view) = this.extensions.as_mut().filter(|v| v.cwd == cwd) else { return };
                view.busy = None;
                match result {
                    Ok(Outcome::Done(_)) => {
                        this.load_extensions(cx);
                        if action == Action::Install {
                            this.browse_plugins(cx);
                        }
                    }
                    Ok(Outcome::Confirm { command, sha256 }) => {
                        view.confirm_command = Some(PendingCommand { id, scope, action, command, sha256 })
                    }
                    Err(e) => view.error = Some(e),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn uninstall_plugin(&mut self, p: &Plugin, cx: &mut Context<Self>) {
        let Some(view) = &mut self.extensions else { return };
        if view.confirm_plugin.as_ref() != Some(&p.id) {
            view.confirm_plugin = Some(p.id.clone());
            cx.notify();
            return;
        }
        self.change_plugin(Action::Uninstall, p.id.clone(), p.scope.clone(), None, cx);
    }

    /// Opens the marketplace browser (or lists again), asking the CLI what
    /// the added marketplaces offer.
    fn browse_plugins(&mut self, cx: &mut Context<Self>) {
        if self.extensions.as_ref().is_some_and(|v| v.market.is_none()) {
            let filter = cx.new(|cx| TextInput::new(cx).placeholder("Filter plugins"));
            cx.subscribe(&filter, |_, _, ev, cx| {
                if let InputEvent::Changed = ev {
                    cx.notify();
                }
            })
            .detach();
            let source = cx.new(|cx| TextInput::new(cx).mono().placeholder("owner/repo, a git URL or a folder"));
            cx.subscribe(&source, |this, _, ev, cx| {
                if let InputEvent::Submit = ev {
                    this.add_marketplace(cx);
                }
            })
            .detach();
            if let Some(view) = &mut self.extensions {
                view.market = Some(Market { available: None, marketplaces: Vec::new(), filter, source, scope: "user" });
            }
        }
        let Some(view) = &mut self.extensions else { return };
        if let Some(m) = &mut view.market {
            m.available = None;
        }
        let cwd = view.cwd.clone();
        cx.spawn(async move |this, cx| {
            let (available, marketplaces) = cx
                .background_spawn({
                    let cwd = cwd.clone();
                    async move { (plugins::available(&cwd), plugins::marketplaces(&cwd).unwrap_or_default()) }
                })
                .await;
            this.update(cx, |this, cx| {
                if let Some(m) = this.extensions.as_mut().filter(|v| v.cwd == cwd).and_then(|v| v.market.as_mut()) {
                    m.available = Some(available);
                    m.marketplaces = marketplaces;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn add_marketplace(&mut self, cx: &mut Context<Self>) {
        let Some(m) = self.extensions.as_ref().and_then(|v| v.market.as_ref()) else { return };
        let source = m.source.read(cx).text().trim().to_string();
        let Some(view) = self.extensions.as_mut().filter(|v| v.busy.is_none()) else { return };
        view.busy = Some(format!("Adding the marketplace {source}…"));
        view.error = None;
        let cwd = view.cwd.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn({
                    let cwd = cwd.clone();
                    async move { plugins::add_marketplace(&cwd, &source) }
                })
                .await;
            this.update(cx, |this, cx| {
                let Some(view) = this.extensions.as_mut().filter(|v| v.cwd == cwd) else { return };
                view.busy = None;
                match result {
                    Ok(()) => {
                        if let Some(m) = &view.market {
                            m.source.update(cx, |i, cx| i.set_text("", cx));
                        }
                        this.browse_plugins(cx);
                    }
                    Err(e) => view.error = Some(e),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn render_plugins(&self, t: &Palette, cx: &mut Context<Self>) -> gpui::Div {
        let Some(view) = &self.extensions else { return div() };
        let (Some(config), Some(extra)) = (&view.config, &view.extra) else { return div() };
        let idle = view.busy.is_none();
        let browse = view.market.is_none().then(|| {
            outline_button("plugins-browse", "Browse marketplaces", cx)
                .on_click(cx.listener(|this, _, _, cx| this.browse_plugins(cx)))
        });
        let confirm = view.confirm_command.clone().map(|p| render_confirm(p, t, cx));
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap_2()
                    .child(div().flex_1().min_w_0().child(heading(
                        "Plugins",
                        "Installed for you, or for this folder. Plugins bring MCP servers, skills, commands, agents \
                         and hooks; sessions pick up changes when they start.",
                        t,
                    )))
                    .children(browse),
            )
            .children(status_lines(view, t))
            .children(confirm)
            .when(config.plugins.is_empty(), |d| {
                d.child(div().text_sm().text_color(t.text_muted).child("No plugins installed."))
            })
            .children(config.plugins.iter().enumerate().map(|(n, p)| {
                let adds = extra.contributes.get(&p.id).map(|c| c.summary()).unwrap_or_default();
                let confirming = view.confirm_plugin.as_ref() == Some(&p.id);
                plugin_row(n, p, adds, confirming, idle, t, cx)
            }))
            .children(view.market.as_ref().map(|m| self.render_market(m, idle, t, cx)))
    }

    fn render_market(&self, m: &Market, idle: bool, t: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let installed: Vec<&str> = self
            .extensions
            .iter()
            .flat_map(|v| v.config.iter())
            .flat_map(|c| &c.plugins)
            .map(|p| p.id.as_str())
            .collect();
        let query = m.filter.read(cx).text().to_lowercase();
        let list: AnyElement = match &m.available {
            None => div()
                .flex()
                .items_center()
                .gap_2()
                .text_xs()
                .text_color(t.text_muted)
                .child(spinner("plugins-listing", t.accent, px(12.)))
                .child("Asking the marketplaces…")
                .into_any_element(),
            Some(Err(e)) => div().text_xs().text_color(t.error).child(e.clone()).into_any_element(),
            Some(Ok(all)) => {
                let shown: Vec<_> = all
                    .iter()
                    .filter(|a| {
                        query.is_empty()
                            || a.name.to_lowercase().contains(&query)
                            || a.description.to_lowercase().contains(&query)
                    })
                    .collect();
                let more = shown.len().saturating_sub(SHOWN);
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .children(shown.into_iter().take(SHOWN).enumerate().map(|(n, a)| {
                        let have = installed.contains(&a.id.as_str());
                        let action = if have {
                            div()
                                .flex_none()
                                .text_xs()
                                .text_color(t.text_placeholder)
                                .child("installed")
                                .into_any_element()
                        } else {
                            let (id, scope) = (a.id.clone(), m.scope.to_string());
                            outline_button(("plugins-install", n), "Install", cx)
                                .when(!idle, |b| b.opacity(0.5))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.change_plugin(Action::Install, id.clone(), scope.clone(), None, cx)
                                }))
                                .into_any_element()
                        };
                        card(t)
                            .items_start()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap_0p5()
                                    .child(
                                        div()
                                            .flex()
                                            .items_baseline()
                                            .gap_2()
                                            .child(
                                                div().text_sm().font_weight(FontWeight::MEDIUM).child(a.name.clone()),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(t.text_placeholder)
                                                    .child(a.marketplace.clone()),
                                            )
                                            .when(a.installs > 0, |d| {
                                                d.child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(t.text_placeholder)
                                                        .child(format!("{} installs", thousands(a.installs))),
                                                )
                                            }),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(t.text_muted)
                                            .line_clamp(2)
                                            .child(a.description.clone()),
                                    ),
                            )
                            .child(action)
                    }))
                    .when(more > 0, |d| {
                        d.child(
                            div()
                                .text_xs()
                                .text_color(t.text_placeholder)
                                .child(format!("{more} more; filter to find them.")),
                        )
                    })
                    .into_any_element()
            }
        };
        let scopes =
            div().flex().items_center().gap_3().text_xs().text_color(t.text_muted).child("Install for").children(
                [("user", "you"), ("project", "this project"), ("local", "this folder, just you")]
                    .into_iter()
                    .enumerate()
                    .map(|(i, (s, label))| {
                        div()
                            .id(("plugins-scope", i))
                            .flex()
                            .items_center()
                            .gap_1()
                            .cursor_pointer()
                            .child(radio(m.scope == s, t))
                            .child(label)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(m) = this.extensions.as_mut().and_then(|v| v.market.as_mut()) {
                                    m.scope = s;
                                }
                                cx.notify();
                            }))
                    }),
            );
        let field = |input: gpui::Entity<TextInput>| {
            div()
                .flex_1()
                .px_2()
                .py_1()
                .rounded_md()
                .border_1()
                .border_color(t.border)
                .bg(t.editor)
                .text_sm()
                .child(input)
        };
        let markets =
            m.marketplaces.iter().map(|mk| format!("{} ({})", mk.name, mk.source)).collect::<Vec<_>>().join(", ");
        div()
            .flex()
            .flex_col()
            .gap_2()
            .pt_4()
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap_2()
                    .child(div().flex_1().min_w_0().child(heading(
                        "Marketplaces",
                        "Plugins the added marketplaces offer, most installed first.",
                        t,
                    )))
                    .child(
                        outline_button("plugins-refresh", "Refresh", cx)
                            .on_click(cx.listener(|this, _, _, cx| this.browse_plugins(cx))),
                    ),
            )
            .when(!markets.is_empty(), |d| d.child(div().text_xs().text_color(t.text_placeholder).child(markets)))
            .child(
                div().flex().items_center().gap_2().child(field(m.source.clone())).child(
                    outline_button("plugins-add-market", "Add marketplace", cx)
                        .on_click(cx.listener(|this, _, _, cx| this.add_marketplace(cx))),
                ),
            )
            .child(scopes)
            .child(div().flex().child(field(m.filter.clone())))
            .child(list)
            .into_any_element()
    }
}

/// One installed plugin: on or off, where it's installed, what it adds, and
/// its actions.
#[allow(clippy::too_many_arguments)]
fn plugin_row(
    n: usize,
    p: &Plugin,
    adds: String,
    confirming: bool,
    idle: bool,
    t: &Palette,
    cx: &mut Context<KynexApp>,
) -> AnyElement {
    let (color, state) = if p.enabled { (t.success, "on") } else { (t.text_placeholder, "off") };
    let button =
        |id: &'static str, label: &'static str, tip: &'static str, action: Action, cx: &mut Context<KynexApp>| {
            let (id_, scope) = (p.id.clone(), p.scope.clone());
            outline_button((id, n), label, cx).when(!idle, |b| b.opacity(0.5)).tooltip(Tooltip::text(tip)).on_click(
                cx.listener(move |this, _, _, cx| this.change_plugin(action, id_.clone(), scope.clone(), None, cx)),
            )
        };
    let toggle = if p.enabled {
        button("plugin-off", "Turn off", "Keep it installed, but don't load it", Action::Disable, cx)
    } else {
        button("plugin-on", "Turn on", "Load it in new sessions", Action::Enable, cx)
    };
    let update = button("plugin-update", "Update", "Update it to the marketplace's latest version", Action::Update, cx);
    let remove = {
        let plugin = p.clone();
        outline_button(("plugin-remove", n), if confirming { "Uninstall?" } else { "Uninstall" }, cx)
            .when(confirming, |b| b.text_color(t.error))
            .when(!idle, |b| b.opacity(0.5))
            .tooltip(Tooltip::text(if confirming { "Click again to uninstall it" } else { "Uninstall it" }))
            .on_click(cx.listener(move |this, _, _, cx| this.uninstall_plugin(&plugin, cx)))
    };
    card(t)
        .items_start()
        .child(div().mt(px(6.)).size(px(7.)).flex_none().rounded_full().bg(color))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap_0p5()
                .child(
                    div()
                        .flex()
                        .items_baseline()
                        .gap_2()
                        .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(p.name().to_string()))
                        .child(div().flex_1())
                        .child(div().flex_none().text_xs().text_color(color).child(state)),
                )
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .text_xs()
                        .text_color(t.text_muted)
                        .child(chip(scope_label(&p.scope), t))
                        .child(p.marketplace().to_string())
                        .when(!p.version.is_empty(), |d| {
                            d.child(div().font_family(mono()).child(short_version(&p.version)))
                        }),
                )
                .child(div().text_xs().text_color(t.text_muted).child(if adds.is_empty() {
                    "Adds nothing Kynex can see.".to_string()
                } else {
                    format!("Adds {adds}")
                }))
                .child(div().flex().flex_wrap().gap_2().pt_1().child(toggle).child(update).child(remove)),
        )
        .into_any_element()
}

/// A marketplace's command, shown in full before it runs.
fn render_confirm(p: PendingCommand, t: &Palette, cx: &mut Context<KynexApp>) -> AnyElement {
    let name = p.id.split('@').next().unwrap_or(&p.id).to_string();
    let run = {
        let p = p.clone();
        primary_button("plugin-accept", "Run it and install", cx).on_click(cx.listener(move |this, _, _, cx| {
            this.change_plugin(p.action, p.id.clone(), p.scope.clone(), Some(p.sha256.clone()), cx)
        }))
    };
    let cancel = outline_button("plugin-decline", "Cancel", cx).on_click(cx.listener(|this, _, _, cx| {
        if let Some(v) = &mut this.extensions {
            v.confirm_command = None;
        }
        cx.notify();
    }));
    div()
        .flex()
        .flex_col()
        .gap_2()
        .p_3()
        .rounded_md()
        .border_1()
        .border_color(t.warning)
        .bg(t.panel)
        .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(format!("{name} runs a command to install")))
        .child(
            div()
                .text_xs()
                .text_color(t.text_muted)
                .child("Its marketplace runs this on your computer. Only go ahead if you trust it."),
        )
        .child(
            div()
                .px_2()
                .py_1()
                .rounded_sm()
                .bg(t.editor)
                .text_xs()
                .font_family(mono())
                .whitespace_normal()
                .child(p.command.clone()),
        )
        .child(div().flex().gap_2().child(run).child(cancel))
        .into_any_element()
}

fn scope_label(scope: &str) -> String {
    match scope {
        "user" => "User".into(),
        "project" => "Project".into(),
        "local" => "Local".into(),
        other => other.into(),
    }
}

/// 1266133 → "1,266,133".
fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Git-hash versions are long; the first 7 characters are enough.
fn short_version(v: &str) -> String {
    if v.len() > 10 && v.chars().all(|c| c.is_ascii_hexdigit()) { v[..7].into() } else { v.into() }
}
