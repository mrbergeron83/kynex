//! The Extensions modal's MCP servers: each with its status, its tools, what
//! it last logged when it failed, and what to do about it: restart it in the
//! open session, sign in (OAuth, through `claude mcp login`), try a browser
//! for real, or remove it.

use crate::ui::px;
use gpui::{AnyElement, Context, FontWeight, div, prelude::*};
use kynex_core::extensions::{self, Approval, Row, Source};
use kynex_core::mcp::Status;
use kynex_core::mcp_config::{self, Scope};
use kynex_core::plugins;

use super::KynexApp;
use super::extensions::{card, chip, status_lines};
use super::settings::heading;
use crate::theme::{Palette, mono};
use crate::ui::{Tooltip, outline_button, spinner};

/// Where claude.ai connectors are connected and authorised.
const CONNECTORS_URL: &str = "https://claude.ai/settings/connectors";

/// What a row needs besides the row itself.
struct RowCtx<'a> {
    confirming: bool,
    /// The open session runs in this folder.
    has_session: bool,
    tools: Option<&'a Vec<String>>,
    tools_open: bool,
    log: Option<&'a String>,
    working: bool,
    result: Option<&'a Result<String, String>>,
}

impl KynexApp {
    fn remove_server(&mut self, name: String, scope: Scope, cx: &mut Context<Self>) {
        let Some(view) = &mut self.extensions else { return };
        if view.confirm_remove.as_ref() != Some(&name) {
            view.confirm_remove = Some(name);
            cx.notify();
            return;
        }
        let busy = format!("Removing {name}…");
        self.change_servers(busy, move |cwd| mcp_config::remove(cwd, &name, scope), cx);
    }

    /// Restarts server `name` in the open session, or checks every server
    /// again when no session runs in the folder.
    fn restart_server(&mut self, name: String, cx: &mut Context<Self>) {
        let Some(cwd) = self.extensions.as_ref().map(|v| v.cwd.clone()) else { return };
        match self.session_mut().filter(|s| s.cwd == cwd) {
            Some(s) => {
                s.reconnect_mcp(&name);
                if let Some(view) = &mut self.extensions {
                    view.results.insert(name, Ok("Restarting it in the open session…".into()));
                }
            }
            None => self.check_servers(cx),
        }
        cx.notify();
    }

    /// Runs `job` for server `name` off the UI thread, showing it as working,
    /// and keeps how it went; `then` runs after a success.
    fn server_job(
        &mut self,
        name: String,
        job: impl FnOnce(&std::path::Path) -> Result<String, String> + Send + 'static,
        then: fn(&mut Self, &str, &mut Context<Self>),
        cx: &mut Context<Self>,
    ) {
        let Some(view) = self.extensions.as_mut().filter(|v| !v.working.contains(&name)) else { return };
        view.working.insert(name.clone());
        view.results.remove(&name);
        let cwd = view.cwd.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn({
                    let cwd = cwd.clone();
                    async move { job(&cwd) }
                })
                .await;
            this.update(cx, |this, cx| {
                let Some(view) = this.extensions.as_mut().filter(|v| v.cwd == cwd) else { return };
                view.working.remove(&name);
                let ok = result.is_ok();
                view.results.insert(name.clone(), result);
                if ok {
                    then(this, &name, cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Signs in to a server that needs it: the CLI opens the browser.
    fn sign_in_server(&mut self, name: String, cli_name: String, cx: &mut Context<Self>) {
        let job = move |cwd: &std::path::Path| {
            plugins::mcp_login(cwd, &cli_name).map(|()| "Signed in. Sessions use it when they start.".into())
        };
        // The open session picks the sign-in up on a restart of the server.
        let then = |this: &mut Self, name: &str, cx: &mut Context<Self>| {
            let cwd = this.extensions.as_ref().map(|v| v.cwd.clone());
            if let Some(s) = this.session_mut().filter(|s| Some(&s.cwd) == cwd.as_ref()) {
                s.reconnect_mcp(name);
            }
            this.check_servers(cx);
        };
        self.server_job(name, job, then, cx);
    }

    /// Has Claude open a page with browser server `name`, for real.
    fn test_browser(&mut self, name: String, cx: &mut Context<Self>) {
        let server = name.clone();
        self.server_job(name, move |cwd| mcp_config::test_browser(cwd, &server), |_, _, _| {}, cx);
    }

    pub(super) fn render_mcp_servers(&self, t: &Palette, cx: &mut Context<Self>) -> gpui::Div {
        let Some(view) = &self.extensions else { return div() };
        let (Some(config), Some(extra)) = (&view.config, &view.extra) else { return div() };
        // The open session's servers if it runs in this folder, then what a check found.
        let session = self.active_session().filter(|s| s.cwd == view.cwd && !s.mcp.is_empty());
        let mut live = session.map(|s| s.mcp.clone());
        let has_session = live.is_some();
        for s in view.checked.iter().flatten() {
            let list = live.get_or_insert_default();
            if !list.iter().any(|l| l.name == s.name) {
                list.push(s.clone());
            }
        }
        let rows = extensions::rows(&config.servers, &live.unwrap_or_default());
        let help = if has_session {
            "What sessions in this folder start with, and how the open session's are doing. \
             Sessions pick up changes when they start."
        } else {
            "What sessions in this folder start with. Check status to start each one and see how it's doing."
        };
        let check = if view.checking {
            div()
                .flex()
                .items_center()
                .gap_1()
                .text_xs()
                .text_color(t.text_muted)
                .child(spinner("extensions-checking", t.accent, px(12.)))
                .child("Checking…")
                .into_any_element()
        } else {
            outline_button("extensions-check", "Check status", cx)
                .tooltip(Tooltip::text("Start every server with `claude mcp list` and see how it's doing"))
                .on_click(cx.listener(|this, _, _, cx| this.check_servers(cx)))
                .into_any_element()
        };
        let buttons = div()
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .child(check)
            .child(
                outline_button("extensions-reload", "Reload", cx)
                    .tooltip(Tooltip::text("Read the config files again"))
                    .on_click(cx.listener(|this, _, _, cx| this.load_extensions(cx))),
            )
            .when(view.adding.is_none(), |d| {
                d.child(
                    outline_button("extensions-add", "Add server", cx)
                        .on_click(cx.listener(|this, _, _, cx| this.open_add_server(cx))),
                )
            });
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap_2()
                    .child(div().flex_1().min_w_0().child(heading("MCP servers", help, t)))
                    .child(buttons),
            )
            .children(status_lines(view, t))
            .children(view.adding.as_ref().map(|form| self.render_add_server(form, t, cx)))
            .when(rows.is_empty(), |d| {
                d.child(
                    div().text_sm().text_color(t.text_muted).child("No MCP servers are configured for this folder."),
                )
            })
            .children(rows.iter().enumerate().map(|(n, r)| {
                let rc = RowCtx {
                    confirming: view.confirm_remove.as_ref() == Some(&r.name),
                    has_session,
                    tools: session.and_then(|s| s.mcp_tools.get(&r.name)),
                    tools_open: view.open_tools.contains(&r.name),
                    log: extra.logs.get(&r.name),
                    working: view.working.contains(&r.name),
                    result: view.results.get(&r.name),
                };
                server_row(n, r, rc, t, cx)
            }))
    }
}

/// One server: a status dot, its name, where it's configured, what it runs or
/// where it connects, what's in the way when it isn't working, and its tools.
fn server_row(n: usize, r: &Row, rc: RowCtx, t: &Palette, cx: &mut Context<KynexApp>) -> AnyElement {
    let (color, state) = match (&r.status, r.approval) {
        (_, Approval::Rejected) => (t.text_placeholder, "turned off here".to_string()),
        (None, Approval::Pending) => (t.warning, "waiting for you to allow it".to_string()),
        (None, _) => (t.text_placeholder, "not started".to_string()),
        (Some(s), _) => (status_color(s, t), s.label().to_string()),
    };
    let detail = r.transport.as_ref().map(|tr| (tr.label().to_string(), tr.detail()));
    let browser = r.transport.as_ref().is_some_and(|tr| tr.detail().contains("playwright"))
        || rc.tools.is_some_and(|tools| tools.iter().any(|t| t == "browser_navigate"));
    let extension = r.transport.as_ref().is_some_and(|tr| tr.detail().contains("--extension"));
    let failed = matches!(r.status, Some(Status::Failed));
    let note = match (&r.status, r.approval, &r.source) {
        (Some(Status::NeedsAuth), _, Source::ClaudeAi) => {
            Some("Connect it on claude.ai; sessions pick it up when they start.")
        }
        (Some(Status::NeedsAuth), _, _) => Some("It needs you to sign in."),
        (Some(Status::Failed), _, _) if extension => Some(
            "It didn't start. Check that Node.js is installed; the browser's Playwright extension is only \
             needed once Claude uses it.",
        ),
        (Some(Status::Failed), _, _) if rc.log.is_none() => {
            Some("It didn't start. Check its command, or that the program it runs is installed.")
        }
        (Some(Status::Connected), _, _) if extension => {
            Some("Uses your own browser: the Playwright extension asks you to allow each connection.")
        }
        (_, Approval::Pending, _) => Some("A project server: Claude Code asks you to allow it when a session starts."),
        _ => None,
    };
    // What it said when it failed, from the CLI's log.
    let log = rc.log.filter(|_| failed).map(|e| {
        div()
            .mt_1()
            .px_2()
            .py_1()
            .rounded_sm()
            .bg(t.editor)
            .text_xs()
            .font_family(mono())
            .text_color(t.error)
            .whitespace_normal()
            .child(e.clone())
    });
    let result = rc.result.map(|r| match r {
        Ok(m) => div().text_xs().text_color(t.success).child(m.clone()),
        Err(e) => div().text_xs().text_color(t.error).whitespace_normal().child(e.clone()),
    });
    let tools = rc.tools.filter(|_| rc.tools_open).map(|tools| {
        div()
            .flex()
            .flex_wrap()
            .gap_1()
            .mt_1()
            .text_xs()
            .font_family(mono())
            .children(tools.iter().map(|tool| chip(tool.clone(), t)))
    });

    let mut actions: Vec<AnyElement> = Vec::new();
    if rc.working {
        actions.push(spinner(("extensions-working", n), t.accent, px(12.)).into_any_element());
    }
    if let Some(tools) = rc.tools.filter(|v| !v.is_empty()) {
        let name = r.name.clone();
        let label = if tools.len() == 1 { "1 tool".to_string() } else { format!("{} tools", tools.len()) };
        actions.push(
            outline_button(("extensions-tools", n), label, cx)
                .when(rc.tools_open, |b| b.bg(t.active))
                .tooltip(Tooltip::text("What Claude can call on this server"))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(v) = &mut this.extensions
                        && !v.open_tools.remove(&name)
                    {
                        v.open_tools.insert(name.clone());
                    }
                    cx.notify();
                }))
                .into_any_element(),
        );
    }
    // Without a session, only a failed server is worth starting again.
    let restartable = if rc.has_session {
        matches!(r.status, Some(Status::Failed | Status::Connected | Status::Pending))
    } else {
        failed
    };
    if restartable && r.approval != Approval::Rejected {
        let name = r.name.clone();
        let (label, tip) = if rc.has_session {
            ("Restart", "Restart it in the open session")
        } else {
            ("Try again", "Start every server again with `claude mcp list`")
        };
        actions.push(
            outline_button(("extensions-restart", n), label, cx)
                .tooltip(Tooltip::text(tip))
                .on_click(cx.listener(move |this, _, _, cx| this.restart_server(name.clone(), cx)))
                .into_any_element(),
        );
    }
    match (&r.status, &r.source, &r.cli_name) {
        (Some(Status::NeedsAuth), Source::ClaudeAi, _) => actions.push(
            outline_button(("extensions-connect", n), "Open claude.ai", cx)
                .on_click(|_, _, cx| cx.open_url(CONNECTORS_URL))
                .into_any_element(),
        ),
        (Some(Status::NeedsAuth), _, Some(cli)) if !rc.working => {
            let (name, cli) = (r.name.clone(), cli.clone());
            actions.push(
                outline_button(("extensions-sign-in", n), "Sign in", cx)
                    .tooltip(Tooltip::text("Sign in in your browser"))
                    .on_click(cx.listener(move |this, _, _, cx| this.sign_in_server(name.clone(), cli.clone(), cx)))
                    .into_any_element(),
            );
        }
        _ => {}
    }
    if browser && !rc.working && r.approval != Approval::Rejected {
        let name = r.name.clone();
        actions.push(
            outline_button(("extensions-test", n), "Test", cx)
                .tooltip(Tooltip::text("Have Claude (Haiku) open a page with it: tens of seconds, a little usage"))
                .on_click(cx.listener(move |this, _, _, cx| this.test_browser(name.clone(), cx)))
                .into_any_element(),
        );
    }
    if let Some(scope) = Scope::of(&r.source) {
        let name = r.name.clone();
        let label = if rc.confirming { "Remove?" } else { "Remove" };
        let tip = format!("Remove it from {}", scope.label().to_lowercase());
        actions.push(
            outline_button(("extensions-remove", n), label, cx)
                .when(rc.confirming, |b| b.text_color(t.error))
                .tooltip(Tooltip::text(if rc.confirming { "Click again to remove it".to_string() } else { tip }))
                .on_click(cx.listener(move |this, _, _, cx| this.remove_server(name.clone(), scope, cx)))
                .into_any_element(),
        );
    }

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
                        .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(r.label()))
                        .when(r.label() != r.name, |d| {
                            d.child(
                                div()
                                    .text_xs()
                                    .font_family(mono())
                                    .text_color(t.text_placeholder)
                                    .child(r.name.clone()),
                            )
                        })
                        .child(div().flex_1())
                        .child(div().flex_none().text_xs().text_color(color).child(state)),
                )
                .child(
                    div().flex().gap_2().text_xs().text_color(t.text_muted).child(chip(r.source.label(), t)).children(
                        detail.map(|(kind, what)| {
                            div()
                                .flex()
                                .min_w_0()
                                .gap_2()
                                .child(chip(kind, t))
                                .child(div().min_w_0().truncate().font_family(mono()).child(what))
                        }),
                    ),
                )
                .children(note.map(|n| div().text_xs().text_color(t.text_muted).child(n)))
                .children(log)
                .children(result)
                .when(!actions.is_empty(), |d| {
                    d.child(div().flex().flex_wrap().items_center().gap_2().pt_1().children(actions))
                })
                .children(tools),
        )
        .into_any_element()
}

fn status_color(s: &Status, t: &Palette) -> gpui::Hsla {
    match s {
        Status::Connected => t.success,
        Status::Pending | Status::NeedsAuth => t.warning,
        Status::Failed | Status::Other(_) => t.error,
        Status::Disabled => t.text_placeholder,
    }
}
