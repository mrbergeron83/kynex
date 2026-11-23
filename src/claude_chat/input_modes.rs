//! The composer's input features from the CLI, as the old chat had them: the `!` /
//! `#` / `?` modes (rules in `kynex_core::claude::input::input_mode`), `!` commands
//! run in the workspace and shown in the transcript (their output goes to Claude
//! with the next message), `#` notes added to a memory file picked from a menu, the
//! `?` card of shortcuts, the thinking switch (Alt+T), the `ultrathink` highlight
//! and Ctrl+G to write the message in your editor.

use gpui::{AnyElement, AppContext, Context, FontWeight, SharedString, div, prelude::*};
use kynex_core::claude::input::input_mode::{InputMode, Submit};
use kynex_core::claude::input::memory::{self, Target};
use kynex_core::claude::input::shell;
use kynex_core::claude::input::thinking;

use super::markdown_view::Sel;
use super::state::LogRow;
use super::{ClaudeChatView, centered_column};
use crate::theme::{Palette, mono, theme};
use crate::ui::input::Mark;
use crate::ui::{Tooltip, ghost_button, menu_item, popover, px, spinner};

/// The colour that says which mode the composer is in.
fn mode_colour(mode: InputMode, t: &Palette) -> Option<gpui::Hsla> {
    match mode {
        InputMode::Prompt => None,
        InputMode::Shell => Some(t.warning),
        InputMode::Memory => Some(t.info),
        InputMode::Shortcuts => Some(t.accent),
    }
}

/// The composer's keys, for the `?` card: (keys, what).
fn composer_shortcuts() -> Vec<(&'static str, &'static str)> {
    let mac = cfg!(target_os = "macos");
    vec![
        ("!", "Shell mode: run a command"),
        ("#", "Memory: add a line to CLAUDE.md"),
        ("/", "Commands"),
        ("@", "Mention a file"),
        ("Enter", "Send"),
        ("Shift+Enter", "New line"),
        (if mac { "⌃Enter" } else { "Ctrl+Enter" }, "Send now, with the queue"),
        ("Shift+Tab", "Next permission mode"),
        ("Alt+T", "Thinking on / off"),
        ("↑ / ↓", "Earlier messages"),
        ("Ctrl+R", "Search what you've sent"),
        ("Ctrl+S", "Stash the draft"),
        ("Ctrl+G", "Edit in your editor"),
        ("Esc", "Stop Claude"),
        ("Esc Esc", "Go back to an earlier message"),
    ]
}

impl ClaudeChatView {
    /// The composer's text changed: a leading `!`, `#` or `?` switches its mode.
    pub(super) fn composer_mode_changed(&mut self, cx: &mut Context<Self>) {
        let text = self.composer_input.read(cx).text().to_string();
        let prev = std::mem::replace(&mut self.last_text, text.clone());
        if prev == text {
            return;
        }
        if let Some(switch) = self.input_mode.on_change(&prev, &text, true) {
            self.input_mode = switch.mode;
            self.last_text = switch.text.clone();
            self.composer_input.update(cx, |c, cx| c.set_text(switch.text, cx));
        }
    }

    /// Backspace or Esc in the empty composer leaves its mode; says whether it did.
    pub(super) fn leave_input_mode(&mut self, cx: &mut Context<Self>) -> bool {
        let text = self.composer_input.read(cx).text().to_string();
        match self.input_mode.on_backspace(&text) {
            Some(mode) => {
                self.input_mode = mode;
                self.memory_menu = false;
                cx.notify();
                true
            }
            None => false,
        }
    }

    /// Back to an empty prompt after a send.
    pub(super) fn reset_input_mode(&mut self) {
        self.input_mode = InputMode::Prompt;
        self.memory_menu = false;
        self.last_text.clear();
    }

    /// Enter in a mode other than a prompt: runs the `!` command or asks where the
    /// `#` note goes. True if it took the Enter.
    pub(super) fn submit_in_mode(&mut self, cx: &mut Context<Self>) -> bool {
        let raw = self.composer_input.read(cx).text().to_string();
        let expanded = self.paste_fold.expand(&raw);
        match self.input_mode.submit(&expanded) {
            Submit::Send(_) => false,
            Submit::Nothing => true,
            Submit::Shell(command) => {
                self.remember_sent(&InputMode::Shell.prefixed(&command), cx);
                self.run_shell(command, cx);
                true
            }
            Submit::Memory(_) => {
                self.memory_menu = true;
                cx.notify();
                true
            }
        }
    }

    /// Runs `command` in the workspace; it shows in the transcript, and its output
    /// goes to Claude with the next message.
    fn run_shell(&mut self, command: String, cx: &mut Context<Self>) {
        let id = self.next_id();
        self.push_row(LogRow::Shell { id, command: command.clone(), output: None, sent: false });
        let cwd = self.config.workspace.clone();
        cx.spawn(async move |this, cx| {
            let output = cx.background_spawn(async move { shell::run(&cwd, &command) }).await;
            this.update(cx, |this, cx| {
                if let Some(ix) = this.rows.iter().position(|r| matches!(r, LogRow::Shell { id: i, .. } if *i == id)) {
                    if let LogRow::Shell { output: slot, .. } = &mut this.rows[ix] {
                        *slot = Some(output);
                    }
                    this.remeasure_row(ix);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        self.composer_input.update(cx, |c, cx| c.set_text("", cx));
        self.paste_fold.clear();
        self.reset_input_mode();
        self.scroll_to_bottom();
        cx.notify();
    }

    /// The finished `!` runs not sent yet, as the blocks that tell Claude about them;
    /// marks them sent.
    pub(super) fn take_shell_blocks(&mut self) -> Vec<String> {
        let mut out = Vec::new();
        for row in &mut self.rows {
            if let LogRow::Shell { command, output: Some(output), sent, .. } = row
                && !*sent
            {
                *sent = true;
                let run = shell::Run { id: 0, command: command.clone(), output: Some(output.clone()), sent: false };
                out.extend(shell::blocks(&run));
            }
        }
        out
    }

    /// Adds the composer's `#` note to `target`'s file, and says where.
    fn remember(&mut self, target: Target, cx: &mut Context<Self>) {
        self.memory_menu = false;
        let raw = self.composer_input.read(cx).text().to_string();
        let Submit::Memory(note) = InputMode::Memory.submit(&self.paste_fold.expand(&raw)) else { return };
        self.remember_sent(&InputMode::Memory.prefixed(&note), cx);
        let folder = self.config.workspace.clone();
        cx.spawn(async move |this, cx| {
            let added = cx.background_spawn(async move { memory::add(target, &folder, &note) }).await;
            this.update(cx, |this, cx| {
                match added {
                    Ok(path) => {
                        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                        this.push_notice(format!("Added to {name} ({})", path.display()), false, cx);
                        if this.composer_input.read(cx).text() == raw {
                            this.composer_input.update(cx, |c, cx| c.set_text("", cx));
                            this.reset_input_mode();
                        }
                    }
                    Err(e) => this.push_notice(e, true, cx),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Alt+T: thinking on or off for the rest of the session.
    pub(super) fn toggle_thinking(&mut self, cx: &mut Context<Self>) {
        self.thinking_on = !self.thinking_on;
        if let Some(client) = &self.client {
            let _ = client.set_thinking(self.thinking_on);
        }
        cx.notify();
    }

    /// Ctrl+G: the draft opens in your editor and comes back when it closes.
    pub(super) fn edit_in_editor(&mut self, cx: &mut Context<Self>) {
        if self.editing_outside {
            return;
        }
        let text = self.composer_input.read(cx).text().to_string();
        let sent = text.clone();
        self.editing_outside = true;
        self.push_notice("Editing in your editor; close it to bring the message back.", false, cx);
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { kynex_core::open::edit_text(&text) }).await;
            this.update(cx, |this, cx| {
                this.editing_outside = false;
                match result {
                    Ok(text) => {
                        // Typed here meanwhile: kept, after the editor's text.
                        let now = this.composer_input.read(cx).text().to_string();
                        let text = match now.strip_prefix(sent.as_str()) {
                            _ if now == sent => text,
                            Some(added) => format!("{text}{added}"),
                            None => format!("{text}\n\n{now}"),
                        };
                        this.set_composer(text, cx);
                    }
                    Err(e) => this.push_notice(e, true, cx),
                }
            })
            .ok();
        })
        .detach();
    }

    /// The `ultrathink` highlight for the composer's marks, while the CLI acts on it.
    pub(super) fn ultrathink_marks(&self, text: &str, t: &Palette) -> Vec<Mark> {
        if !self.ultrathink || self.input_mode != InputMode::Prompt {
            return Vec::new();
        }
        thinking::keyword_ranges(text)
            .into_iter()
            .map(|range| Mark { range, color: t.info, background: None, atomic: false })
            .collect()
    }

    /// The mode's label above the field: its character and name in its colour.
    pub(super) fn render_mode_badge(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let mode = self.input_mode;
        let t = theme(cx).clone();
        let colour = mode_colour(mode, &t)?;
        let prefix = mode.prefix()?;
        Some(
            div()
                .flex()
                .items_center()
                .gap_1p5()
                .px_3()
                .pt_2()
                .text_xs()
                .text_color(colour)
                .child(div().font_family(mono()).font_weight(FontWeight::BOLD).child(prefix.to_string()))
                .child(mode.label())
                .child(div().text_color(t.text_placeholder).child("· Backspace to leave"))
                .into_any_element(),
        )
    }

    /// The thinking chip, beside effort: on or off, click to switch.
    pub(super) fn render_thinking_chip(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx).clone();
        let on = self.thinking_on;
        let tip = if on {
            "Thinking on: Claude reasons before it answers. Click to turn it off (some models always think)"
        } else {
            "Thinking off: Claude answers straight away. Click to turn it on"
        };
        ghost_button("comp-thinking", cx)
            .h(px(22.))
            .px_1p5()
            .text_xs()
            .text_color(if on { t.text_muted } else { t.text_placeholder })
            .child(if on { "Thinking" } else { "No thinking" })
            .tooltip(Tooltip::with_meta(tip, "Alt+T"))
            .on_click(cx.listener(|this, _, _, cx| this.toggle_thinking(cx)))
            .into_any_element()
    }

    /// Above the composer: the `?` card, or the menu of where a `#` note goes.
    pub(super) fn render_mode_popup(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let t = theme(cx).clone();
        let body = if self.memory_menu {
            let folder = self.config.workspace.clone();
            let items: Vec<AnyElement> = Target::ALL
                .into_iter()
                .enumerate()
                .map(|(n, target)| {
                    let path = target.file(&folder).unwrap_or_default();
                    menu_item(("memory", n), target.label(), false, cx)
                        .tooltip(Tooltip::text(path.display().to_string()))
                        .on_click(cx.listener(move |this, _, _, cx| this.remember(target, cx)))
                        .into_any_element()
                })
                .collect();
            popover(cx)
                .w(px(280.))
                .p_1()
                .child(div().px_2().py_1().text_xs().text_color(t.text_placeholder).child("Add the note to"))
                .children(items)
        } else if self.input_mode == InputMode::Shortcuts {
            let rows = composer_shortcuts().into_iter().map(|(keys, what)| {
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .py_0p5()
                    .child(div().w(px(110.)).flex_none().font_family(mono()).text_xs().text_color(t.text).child(keys))
                    .child(div().text_xs().text_color(t.text_muted).child(what))
            });
            let (left, right): (Vec<_>, Vec<_>) = rows.enumerate().partition(|(n, _)| n % 2 == 0);
            let column = |rows: Vec<(usize, gpui::Div)>| {
                div().flex_1().flex().flex_col().children(rows.into_iter().map(|r| r.1))
            };
            popover(cx).w_full().p_3().child(div().flex().gap_6().child(column(left)).child(column(right)))
        } else {
            return None;
        };
        Some(div().flex_none().px_4().mb_1().child(centered_column(body)).into_any_element())
    }

    /// A `!` command in the transcript: the command, then what it printed.
    pub(super) fn render_shell_run(
        &self,
        id: u64,
        command: &str,
        output: Option<&shell::Output>,
        sent: bool,
        sel: &Sel,
        t: &Palette,
    ) -> AnyElement {
        let running = output.is_none() && !sent;
        let failed = output.is_some_and(shell::Output::failed);
        let head = div()
            .flex()
            .items_start()
            .gap_2()
            .font_family(mono())
            .text_sm()
            .child(div().flex_none().text_color(t.warning).child("!"))
            .child(div().flex_1().min_w_0().child(sel.text(command.to_string(), t)))
            .when(running, |d| d.child(spinner(("shell-run", id), t.warning, px(12.))))
            .when_some(output.and_then(|o| o.code).filter(|_| failed), |d, code| {
                d.child(div().flex_none().text_xs().text_color(t.error).child(format!("exit {code}")))
            });
        let body = output.filter(|o| !o.stdout.is_empty() || !o.stderr.is_empty()).map(|o| {
            div()
                .id(SharedString::from(format!("shell-output-{id}")))
                .max_h(px(320.))
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap_1()
                .font_family(mono())
                .text_xs()
                .when(!o.stdout.is_empty(), |d| {
                    d.child(div().text_color(t.text_muted).child(sel.text(o.stdout.clone(), t)))
                })
                .when(!o.stderr.is_empty(), |d| {
                    d.child(
                        div()
                            .text_color(if failed { t.error } else { t.text_muted })
                            .child(sel.text(o.stderr.clone(), t)),
                    )
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
            .children(body)
            .into_any_element()
    }
}
