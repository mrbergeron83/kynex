//! The About Kynex modal, from the Kynex menu in the title bar: the version and
//! build, where Kynex lives on disk, the Claude Code it drives, licences, and the
//! update controls for both (check now, update, restart).

use std::path::PathBuf;

use gpui::{AnyElement, Context, FontWeight, MouseButton, SharedString, Window, div, prelude::*, px};
use kynex_core::cli::{self, UpdateState};
use kynex_core::fmt::ago;
use kynex_core::open::Opener;
use kynex_core::{paths, update};

use super::KynexApp;
use super::app_update::VERSION;
use crate::theme::{Palette, mono, theme};
use crate::ui::mark::mark;
use crate::ui::motion::{self, fade_in, pop_in};
use crate::ui::{
    Icon, Lamp, Tooltip, ghost_button, icon, icon_button, outline_button, primary_button, section_label, spinner,
};

/// The commit the build came from (build.rs), "dev" outside git.
const COMMIT: &str = env!("KYNEX_COMMIT");

impl KynexApp {
    pub(super) fn open_about(&mut self, cx: &mut Context<Self>) {
        self.about_open = true;
        self.update_toast = false;
        if self.claude_version.is_none() {
            cx.spawn(async move |this, cx| {
                let v = cx.background_spawn(async { cli::installed_version() }).await;
                this.update(cx, |this, cx| {
                    this.claude_version = Some(v.unwrap_or_else(|_| "not found".into()));
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
        cx.notify();
    }

    pub(super) fn render_about(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.about_open {
            return None;
        }
        let t = theme(cx).clone();
        let vp = window.viewport_size();
        let exe_dir = std::env::current_exe().ok().and_then(|e| e.parent().map(PathBuf::from));
        let data = paths::data_dir();
        let build = format!(
            "{} {}{}",
            std::env::consts::OS,
            std::env::consts::ARCH,
            if cfg!(debug_assertions) { ", development build" } else { "" }
        );

        let header = div().flex().items_center().gap_4().child(mark("about-mark", px(56.), Lamp::Idle, cx)).child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(section_label("Kynex", cx).text_size(px(20.)).text_color(t.text).font_weight(FontWeight::BLACK))
                .child(div().text_sm().text_color(t.text_muted).child("A native workspace for Claude Code"))
                .child(
                    div()
                        .text_xs()
                        .text_color(t.text_placeholder)
                        .font_family(mono())
                        .child(format!("v{VERSION} · {COMMIT}")),
                ),
        );

        let rows = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(info_row("Build", build.into(), None, &t, cx))
            .child(
                info_row("Claude Code", self.claude_version.clone().unwrap_or_else(|| "…".into()).into(), None, &t, cx)
                    .children(self.claude_version.clone().map(|v| {
                        icon_button("about-copy-cli", Icon::Copy, cx)
                            .tooltip(Tooltip::text("Copy, for bug reports"))
                            .on_click(move |_, _, cx| {
                                cx.write_to_clipboard(gpui::ClipboardItem::new_string(format!("Claude Code {v}")))
                            })
                    })),
            )
            .child(info_row(
                "Installed in",
                exe_dir.as_ref().map_or("unknown".into(), |d| d.display().to_string()).into(),
                exe_dir,
                &t,
                cx,
            ))
            .child(info_row("Data", data.display().to_string().into(), Some(data), &t, cx));

        let links = div().flex().flex_wrap().gap_1().children(
            [
                ("GitHub", format!("https://github.com/{}", update::REPO)),
                ("Release notes", format!("https://github.com/{}/releases", update::REPO)),
                ("Licence", format!("https://github.com/{}/blob/main/LICENSE", update::REPO)),
                ("Third-party notices", format!("https://github.com/{}/blob/main/NOTICE.md", update::REPO)),
            ]
            .into_iter()
            .enumerate()
            .map(|(n, (label, url))| {
                ghost_button(("about-link", n), cx)
                    .text_xs()
                    .text_color(t.accent)
                    .child(label)
                    .on_click(move |_, _, cx| cx.open_url(&url))
            }),
        );

        let card = div()
            .id("about-card")
            .occlude()
            .flex()
            .flex_col()
            .gap_5()
            .w(px(520.).min(vp.width - px(80.)))
            .max_h(vp.height - px(100.))
            .p_6()
            .rounded_lg()
            .border_1()
            .border_color(t.border)
            .bg(t.elevated)
            .shadow_lg()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(div().flex().items_start().child(div().flex_1().child(header)).child(
                icon_button("close-about", Icon::Close, cx).tooltip(Tooltip::with_meta("Close", "Esc")).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.about_open = false;
                        cx.notify();
                    }),
                ),
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(self.render_update_box(&t, cx))
                    .child(self.render_cli_box(&t, cx)),
            )
            .child(rows)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .pt_3()
                    .border_t_1()
                    .border_color(t.border_variant)
                    .child(div().text_xs().text_color(t.text_muted).child(
                        "Free software under the GNU General Public License v3.0 or later. \
                         © 2026 Michael Bergeron.",
                    ))
                    .child(links),
            );

        Some(
            fade_in(
                "about-fade",
                div()
                    .id("about-backdrop")
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(gpui::black().opacity(0.45))
                    .occlude()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.about_open = false;
                            cx.notify();
                        }),
                    )
                    .child(pop_in("about-in", card, px(8.))),
                motion::QUICK,
            )
            .into_any_element(),
        )
    }

    /// Where updating stands, with the one action that fits.
    fn render_update_box(&mut self, t: &Palette, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let installed = update::installed_exe().is_ok();
        let checked = self.update_checked.map(|c| format!("Checked {}", ago(c)));
        let check_now = || {
            outline_button("about-check", "Check now", cx)
                .on_click(cx.listener(|this, _, _, cx| this.check_for_update(cx)))
                .into_any_element()
        };
        let (lead, title, detail, action): (AnyElement, String, Option<String>, Option<AnyElement>) =
            match &self.app_update {
                _ if self.update_checking => (
                    spinner("about-check-spin", t.accent, px(14.)).into_any_element(),
                    "Checking for updates…".into(),
                    None,
                    None,
                ),
                UpdateState::Idle => (
                    icon(Icon::Check, t.success).size_3p5().into_any_element(),
                    if checked.is_some() { "Kynex is up to date".into() } else { "Not checked yet".into() },
                    checked,
                    Some(check_now()),
                ),
                UpdateState::Available { installed: have, latest } => (
                    icon(Icon::ArrowUp, t.accent).size_3p5().into_any_element(),
                    format!("Kynex {latest} is available"),
                    Some(if installed {
                        format!("You have {have}. Kynex keeps running until you restart.")
                    } else {
                        format!("You have {have}. Development builds don't update themselves.")
                    }),
                    installed.then(|| {
                        primary_button("about-update", "Update", cx)
                            .on_click(cx.listener(|this, _, _, cx| this.update_app(cx)))
                            .into_any_element()
                    }),
                ),
                UpdateState::Updating { latest } => (
                    spinner("about-update-spin", t.accent, px(14.)).into_any_element(),
                    format!("Downloading Kynex {latest}…"),
                    None,
                    None,
                ),
                UpdateState::Updated { version } => (
                    icon(Icon::Check, t.success).size_3p5().into_any_element(),
                    format!("Kynex {version} is installed"),
                    Some("Restart to use it; your tabs and sessions come back.".into()),
                    Some(
                        primary_button("about-restart", "Restart", cx)
                            .tooltip(Tooltip::text("Running turns stop; sessions resume after the restart"))
                            .on_click(cx.listener(|this, _, _, cx| this.restart_app(cx)))
                            .into_any_element(),
                    ),
                ),
                UpdateState::Failed(e) => (
                    icon(Icon::Dot, t.error).size_3p5().into_any_element(),
                    "Couldn't check for updates".into(),
                    Some(e.clone()),
                    Some(check_now()),
                ),
            };
        status_box(lead, title, detail, action, self.update_waiting(), t)
    }

    /// The Claude Code Kynex drives: its version, a check on demand, and the update.
    fn render_cli_box(&mut self, t: &Palette, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let version = self.claude_version.clone().unwrap_or_else(|| "…".into());
        let check_now = || {
            outline_button("about-cli-check", "Check now", cx)
                .on_click(cx.listener(|this, _, _, cx| this.check_cli_now(cx)))
                .into_any_element()
        };
        let ok = || icon(Icon::Check, t.success).size_3p5().into_any_element();
        let failed = || icon(Icon::Dot, t.error).size_3p5().into_any_element();
        let (lead, title, detail, action): (AnyElement, String, Option<String>, Option<AnyElement>) =
            match (&self.cli_update, &self.cli_check) {
                (UpdateState::Updating { latest }, _) => (
                    spinner("about-cli-update-spin", t.accent, px(14.)).into_any_element(),
                    format!("Updating Claude Code to {latest}…"),
                    None,
                    None,
                ),
                (_, c) if c.checking => (
                    spinner("about-cli-check-spin", t.accent, px(14.)).into_any_element(),
                    "Checking for Claude Code updates…".into(),
                    None,
                    None,
                ),
                (UpdateState::Available { installed, latest }, _) => (
                    icon(Icon::ArrowUp, t.accent).size_3p5().into_any_element(),
                    format!("Claude Code {installed} → {latest} available"),
                    Some("Idle sessions restart on it; busy ones once they finish.".into()),
                    Some(
                        primary_button("about-cli-update", "Update", cx)
                            .tooltip(Tooltip::text("Runs `claude update`"))
                            .on_click(cx.listener(|this, _, _, cx| this.update_cli(cx)))
                            .into_any_element(),
                    ),
                ),
                (UpdateState::Updated { version }, _) => (
                    ok(),
                    format!("Claude Code {version} is installed"),
                    Some("Sessions pick it up with their next message.".into()),
                    None,
                ),
                (UpdateState::Failed(e), _) => {
                    (failed(), "Couldn't update Claude Code".into(), Some(e.clone()), Some(check_now()))
                }
                (_, c) if c.error.is_some() => {
                    (failed(), "Couldn't check for Claude Code updates".into(), c.error.clone(), Some(check_now()))
                }
                (_, c) => match c.at {
                    Some(at) => (
                        ok(),
                        format!("Claude Code {version} is up to date"),
                        Some(format!("Checked {}", ago(at))),
                        Some(check_now()),
                    ),
                    None => (ok(), format!("Claude Code {version}"), Some("Not checked yet".into()), Some(check_now())),
                },
            };
        let waiting = matches!(self.cli_update, UpdateState::Available { .. });
        status_box(lead, title, detail, action, waiting, t)
    }
}

/// Where an update stands: a lead icon, a line, an optional detail, and the action that fits.
fn status_box(
    lead: AnyElement,
    title: String,
    detail: Option<String>,
    action: Option<AnyElement>,
    highlight: bool,
    t: &Palette,
) -> impl IntoElement + use<> {
    div()
        .flex()
        .items_center()
        .gap_3()
        .px_3()
        .py_2p5()
        .rounded_md()
        .border_1()
        .border_color(if highlight { t.accent } else { t.border_variant })
        .bg(t.panel)
        .child(lead)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(div().text_sm().child(title))
                .children(detail.map(|d| div().text_xs().text_color(t.text_muted).child(d))),
        )
        .children(action)
}

/// One fact: a label, its value, and an "open folder" button for paths.
fn info_row(
    label: &'static str,
    value: SharedString,
    folder: Option<PathBuf>,
    t: &Palette,
    cx: &mut Context<KynexApp>,
) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .gap_3()
        .h(px(24.))
        .child(div().w(px(96.)).flex_none().text_xs().text_color(t.text_muted).child(label))
        .child(div().flex_1().min_w_0().truncate().text_xs().font_family(mono()).child(value))
        .children(folder.map(|path| {
            icon_button(SharedString::from(format!("about-open-{label}")), Icon::Folder, cx)
                .tooltip(Tooltip::text("Open folder"))
                .on_click(cx.listener(move |this, _, _, _| this.open_in(Opener::Explorer, &path)))
        }))
}
