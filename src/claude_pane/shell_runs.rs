//! What Enter does in the composer's `!` and `#` modes: a shell command run in the
//! session's folder and shown in the transcript (`kynex_core::claude::input::shell`),
//! and a note added to a memory file picked from a menu
//! (`kynex_core::claude::input::memory`).

use std::path::PathBuf;

use crate::ui::px;
use gpui::{AnyElement, Context, div, prelude::*};
use kynex_core::claude::input::input_mode::{InputMode, Submit};
use kynex_core::claude::input::memory::{self, Target};
use kynex_core::claude::input::shell::{self, Run};
use kynex_core::toasts::Toast;

use super::ClaudePane;
use super::Key;
use super::menus::Menu;
use crate::theme::{mono, theme};
use crate::ui::selectable::{Spot, selectable};
use crate::ui::{Tooltip, menu_item, spinner};

impl ClaudePane {
    /// Runs `command` in session `key`'s folder; it shows in the transcript, and
    /// its output goes to Claude with the next message.
    pub(super) fn run_shell(&mut self, key: Option<Key>, command: String, cx: &mut Context<Self>) {
        let Some((key, s)) = key.and_then(|k| Some((k, self.session_for(k)?))) else { return };
        let cwd = s.cwd.clone();
        let id = s.start_shell(command.clone());
        let task = cx.background_spawn(async move { shell::run(&cwd, &command) });
        cx.spawn(async move |this, cx| {
            let output = task.await;
            this.update(cx, |this, cx| {
                if let Some(s) = this.session_for(key) {
                    s.finish_shell(id, output);
                }
                this.follow_tail(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
        self.composer.update(cx, |c, cx| c.set_text("", cx));
        self.reset_input_mode();
        self.follow_tail(cx);
        cx.notify();
    }

    /// Enter in memory mode: asks which file the note goes to.
    pub(super) fn ask_memory(&mut self, cx: &mut Context<Self>) {
        self.open_menu_at_composer(Menu::Memory, cx);
    }

    /// The memory menu: where the composer's note can go.
    pub(super) fn memory_items(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let folder = self.session.cwd.clone();
        Target::ALL
            .into_iter()
            .enumerate()
            .map(|(n, target)| {
                let path = target.file(&folder).unwrap_or_default();
                let folder = folder.clone();
                menu_item(("memory", n), target.label(), false, cx)
                    .tooltip(Tooltip::text(path.display().to_string()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.menu = None;
                        this.remember(target, folder.clone(), cx);
                    }))
                    .into_any_element()
            })
            .collect()
    }

    /// Adds the composer's note to `target`'s file for `folder`, and says where.
    fn remember(&mut self, target: Target, folder: PathBuf, cx: &mut Context<Self>) {
        let raw = self.composer.read(cx).text().to_string();
        let (_, Submit::Memory(note)) = self.take_submit(&raw) else { return };
        self.remember_sent(&InputMode::Memory.prefixed(&note), cx);
        let task = cx.background_spawn(async move { memory::add(target, &folder, &note) });
        cx.spawn(async move |this, cx| {
            let added = task.await;
            this.update(cx, |this, cx| {
                match added {
                    Ok(path) => {
                        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                        this.toast(Toast::success(format!("Added to {name}")).detail(path.display().to_string()), cx);
                        if this.composer.read(cx).text() == raw {
                            this.composer.update(cx, |c, cx| c.set_text("", cx));
                            this.reset_input_mode();
                        }
                    }
                    Err(e) => {
                        this.toast(Toast::error(e), cx);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// A `!` command in the transcript: the command, then what it printed.
    pub(super) fn render_shell_run(&self, key: Key, ix: usize, run: &Run, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx).clone();
        let running = run.output.is_none() && !run.sent;
        let failed = run.output.as_ref().is_some_and(|o| o.failed());
        let head = div()
            .flex()
            .items_start()
            .gap_2()
            .font_family(mono())
            .text_sm()
            .child(div().flex_none().text_color(t.warning).child("!"))
            .child(div().flex_1().min_w_0().child(selectable(
                key,
                Spot::new(ix, 0),
                run.command.clone(),
                t.text_selection,
            )))
            .when(running, |d| d.child(spinner(("shell-run", ix), t.warning, px(12.))))
            .when_some(run.output.as_ref().and_then(|o| o.code).filter(|_| failed), |d, code| {
                d.child(div().flex_none().text_xs().text_color(t.error).child(format!("exit {code}")))
            });
        let output = run.output.as_ref().filter(|o| !o.stdout.is_empty() || !o.stderr.is_empty()).map(|o| {
            div()
                .id(("shell-output", ix))
                .max_h(px(320.))
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap_1()
                .font_family(mono())
                .text_xs()
                .when(!o.stdout.is_empty(), |d| {
                    d.child(div().text_color(t.text_muted).child(selectable(
                        key,
                        Spot::new(ix, 1),
                        o.stdout.clone(),
                        t.text_selection,
                    )))
                })
                .when(!o.stderr.is_empty(), |d| {
                    d.child(div().text_color(if failed { t.error } else { t.text_muted }).child(selectable(
                        key,
                        Spot::new(ix, 2),
                        o.stderr.clone(),
                        t.text_selection,
                    )))
                })
        });
        div()
            .flex()
            .flex_col()
            .gap_2()
            .px_3()
            .py_2()
            .rounded_lg()
            .border_1()
            .border_color(t.border_variant)
            .bg(t.element)
            .child(head)
            .children(output)
            .into_any_element()
    }
}
