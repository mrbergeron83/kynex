//! Custom commands (`kynex_core::user_commands`): a key or the palette runs a program in
//! the selected workspace's folder, with `KYNEX_WORKSPACE_DIR`, `KYNEX_BRANCH` and
//! `KYNEX_SESSION_ID` set. It shows in a popup over the window that closes when the program
//! exits, in a terminal pane, or nowhere (only a failure is told).

use std::path::{Path, PathBuf};

use gpui::{AnyElement, App, Context, Focusable, KeyBinding, MouseButton, Window, div, prelude::*};
use kynex_core::layout::{Direction, PaneId};
use kynex_core::terminal::ShellSpec;
use kynex_core::tiling::{TermPane, Tool};
use kynex_core::toasts::Toast;
use kynex_core::user_commands::{self, Placement, UserCommand};
use kynex_core::workspace::same_folder;

use super::KynexApp;
use super::keys::RunCommand;
use crate::terminal::{TerminalEvent, TerminalView};
use crate::theme::theme;
use crate::ui::px;

/// Binds the keys of the user's commands and those of the projects in `projects`, as they
/// are on disk now. A key runs its command in whichever workspace is selected. Blocks on
/// the files (once, at startup).
pub fn bind_command_keys(projects: &[PathBuf], cx: &mut App) {
    let mut all = user_commands::load(&user_commands::user_file()).unwrap_or_default();
    for p in projects {
        all.extend(user_commands::load(&user_commands::project_file(p)).unwrap_or_default());
    }
    let mut bindings = Vec::new();
    for c in all {
        let Some(key) = c.key else { continue };
        let keys = crate::ui::keys(&key);
        if keys.split_whitespace().any(|k| gpui::Keystroke::parse(k).is_err()) {
            eprintln!("kynex: commands.json: `{key}` isn't a key for {}", c.name);
            continue;
        }
        bindings.push(KeyBinding::new(keys, RunCommand(c.name), Some("Workspace")));
    }
    cx.bind_keys(bindings);
}

impl KynexApp {
    /// The project whose commands apply: the selected one, none in chat.
    fn command_project(&self) -> Option<PathBuf> {
        self.active_project().map(|p| p.path.clone())
    }

    /// Reads the commands again for the palette, in the background.
    pub(super) fn refresh_user_commands(&mut self, cx: &mut Context<Self>) {
        let project = self.command_project();
        cx.spawn(async move |this, cx| {
            let (list, _) = cx.background_spawn(async move { user_commands::for_project(project.as_deref()) }).await;
            this.update(cx, |this, cx| {
                if this.user_commands != list {
                    this.user_commands = list;
                    this.rebuild_palette_if_open(cx);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// A command's key: runs the command of that name.
    pub(super) fn run_command_action(&mut self, a: &RunCommand, window: &mut Window, cx: &mut Context<Self>) {
        self.run_user_command(a.0.clone(), window, cx);
    }

    /// Runs custom command `name` in the selected workspace, read fresh from the files.
    pub(super) fn run_user_command(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ws) = self.workspace_cwd() else {
            self.toast(Toast::error("Custom commands run in a project's workspace; open one first.".to_string()));
            cx.notify();
            return;
        };
        let session = self.focused_key().and_then(|k| self.session_by_key(k)).and_then(|s| s.session_id.clone());
        let found = self.resolve_command(name, &ws, session, cx);
        cx.spawn_in(window, async move |this, cx| {
            let found = found.await;
            this.update_in(cx, |this, window, cx| {
                match found {
                    Ok((c, spec)) => this.start_command(c, spec, ws, window, cx),
                    Err(e) => {
                        this.toast(Toast::error(e));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Command `name` and its program, looked up in the background.
    fn resolve_command(
        &self,
        name: String,
        ws: &Path,
        session: Option<String>,
        cx: &mut Context<Self>,
    ) -> gpui::Task<Result<(UserCommand, ShellSpec), String>> {
        let project = self.command_project();
        let branch = self.branch_of(ws, cx);
        let ws = ws.to_path_buf();
        cx.background_spawn(async move {
            let (list, errors) = user_commands::for_project(project.as_deref());
            let Some(c) = list.into_iter().find(|c| c.name == name) else {
                return Err(errors
                    .into_iter()
                    .next()
                    .unwrap_or_else(|| format!("There's no command \"{name}\" here.")));
            };
            // The workspaces list may not be read yet: ask git.
            let branch =
                branch.or_else(|| kynex_core::git::status(&ws).ok().and_then(|s| s.head.branch().map(str::to_string)));
            let env = user_commands::env(&ws, branch.as_deref(), session.as_deref());
            let spec = user_commands::spec(&c, &ws, env)?;
            Ok((c, spec))
        })
    }

    /// The branch workspace `ws` is on, as the workspaces list knows it.
    fn branch_of(&self, ws: &Path, cx: &App) -> Option<String> {
        let project = self.command_project()?;
        let w = self.worktrees.get(&project)?;
        let list = kynex_core::workspace::from_worktrees(&project, &w.read(cx).list);
        list.into_iter().find(|c| same_folder(&c.path, ws)).and_then(|c| c.branch)
    }

    fn start_command(
        &mut self,
        c: UserCommand,
        spec: ShellSpec,
        ws: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match c.placement {
            Placement::Popup => {
                let view = cx.new(|cx| TerminalView::new(spec, ws, cx));
                cx.subscribe_in(&view, window, |this, view, _: &TerminalEvent, window, cx| {
                    if this.command_popup.as_ref() == Some(view) && view.read(cx).exited() {
                        // It didn't start: say why rather than just vanish.
                        if let Some(e) = view.read(cx).error() {
                            this.toast(Toast::error(e.to_string()));
                        }
                        this.command_popup = None;
                        this.focus_input(window, cx);
                        cx.notify();
                    }
                })
                .detach();
                view.read(cx).focus_handle(cx).focus(window, cx);
                self.command_popup = Some(view);
            }
            Placement::Pane => self.open_command_pane(c.name, spec, window, cx),
            Placement::Background => run_in_background(c.name, spec, ws, cx),
        }
        cx.notify();
    }

    /// A terminal pane running the command, on the right of the session pane.
    fn open_command_pane(&mut self, name: String, spec: ShellSpec, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, ws, _)) = self.tiling_scope() else {
            self.toast(Toast::error("A command pane opens in a project.".to_string()));
            return;
        };
        let Some(target) = self.tiles.get(&ws).and_then(|t| t.session_focus()) else { return };
        let id = self.next_pane;
        let bounds = self.layout_bounds(cx);
        let tool = Tool::Terminal(TermPane { command: Some(name), ..TermPane::default() });
        let Some(t) = self.tiles.get_mut(&ws) else { return };
        if !t.split_tool(target, Direction::Right, id, tool, 0.5, bounds) {
            self.toast(Toast::error("No room to split here: the panes would get too small.".to_string()));
            return;
        }
        self.next_pane += 1;
        let panel = Self::new_term_panel(&ws, window, cx);
        self.term_panes.insert(id, panel.clone());
        panel.update(cx, |p, cx| p.open_spec(spec, window, cx));
        self.show_focused_pane(window, cx);
    }

    /// Command pane `id` from last time, drawn with no program yet: starts command `name`
    /// in it again, or says why it can't.
    pub(super) fn restart_command_pane(
        &mut self,
        id: PaneId,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((_, ws, _)) = self.tiling_scope() else { return };
        if !self.command_starting.insert(id) {
            return;
        }
        let session = self.focused_key().and_then(|k| self.session_by_key(k)).and_then(|s| s.session_id.clone());
        let found = self.resolve_command(name, &ws, session, cx);
        cx.spawn_in(window, async move |this, cx| {
            let found = found.await;
            this.update_in(cx, |this, window, cx| {
                let Some(panel) = this.term_panes.get(&id).cloned() else { return };
                match found {
                    Ok((_, spec)) => {
                        this.command_starting.remove(&id);
                        panel.update(cx, |p, cx| p.open_spec(spec, window, cx));
                    }
                    Err(e) => panel.update(cx, |p, cx| p.show_error(e, cx)),
                }
            })
            .ok();
        })
        .detach();
    }

    /// The popup: the program over the window, closing when it exits.
    pub(super) fn render_command_popup(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let view = self.command_popup.clone()?;
        let t = theme(cx).clone();
        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::black().opacity(0.35))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .w(gpui::relative(0.85))
                        .h(gpui::relative(0.85))
                        .rounded(px(6.))
                        .border_1()
                        .border_color(t.border)
                        .overflow_hidden()
                        .bg(t.editor)
                        .child(view),
                )
                .into_any_element(),
        )
    }
}

/// Runs `spec` with no window, telling only of a failure.
fn run_in_background(name: String, spec: ShellSpec, ws: PathBuf, cx: &mut Context<KynexApp>) {
    cx.spawn(async move |this, cx| {
        let failed = cx
            .background_spawn(async move {
                // A thread of its own as the parent: on Linux the child dies with the thread
                // that started it, and pool threads come and go.
                let run = std::thread::spawn(move || {
                    let mut cmd = user_commands::background(&spec, &ws);
                    kynex_core::process::dies_with_parent(&mut cmd);
                    cmd.output()
                });
                let output = run.join().unwrap_or_else(|_| Err(std::io::Error::other("it panicked")));
                match output {
                    Ok(o) if o.status.success() => None,
                    Ok(o) => {
                        let err = String::from_utf8_lossy(&o.stderr);
                        let last = err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string();
                        Some(if last.is_empty() {
                            format!("{name} failed ({}).", o.status)
                        } else {
                            format!("{name}: {last}")
                        })
                    }
                    Err(e) => Some(format!("Couldn't run {name}: {e}")),
                }
            })
            .await;
        if let Some(e) = failed {
            this.update(cx, |this, cx| {
                this.toast(Toast::error(e));
                cx.notify();
            })
            .ok();
        }
    })
    .detach();
}
