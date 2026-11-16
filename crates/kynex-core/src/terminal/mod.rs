//! An interactive terminal: a shell on a pseudo-terminal (ConPTY on Windows),
//! parsed by `alacritty_terminal` into a grid of cells. The app draws
//! [`Snapshot`]s of it and sends keys; this knows nothing about drawing.
//!
//! The shell inherits Kynex's own rights, so when Kynex runs as administrator
//! its terminals do too ([`elevated`] says which).

pub mod hooks;
mod keys;
pub mod marker;
mod shell;
mod snapshot;

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use alacritty_terminal::Term;
use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::Config;
use alacritty_terminal::tty;

pub use keys::{Key, key_bytes, paste_bytes};
use marker::Mark;
pub use shell::{ShellKind, ShellSpec, elevated, resolve as resolve_shell, which};
pub use snapshot::{Cursor, CursorStyle, Palette, Rgb, Row, Run, Snapshot};

use crate::session::Wake;

/// Lines kept above the screen.
const SCROLLBACK: usize = 10_000;

/// A grid size, in cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Size {
    pub columns: usize,
    pub lines: usize,
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.lines
    }

    fn screen_lines(&self) -> usize {
        self.lines
    }

    fn columns(&self) -> usize {
        self.columns
    }
}

/// How a mouse drag selects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectBy {
    Cell,
    Word,
    Line,
}

/// What the terminal reports back, shared with its listener thread.
#[derive(Default)]
struct Shared {
    title: Mutex<Option<String>>,
    exited: AtomicBool,
    /// Text the program asked to copy (OSC 52), for the app to put on the clipboard.
    clipboard: Mutex<Option<String>>,
    /// Exit codes of marked commands that finished ([`marker`]), by marker.
    done: Mutex<HashMap<u64, i32>>,
    /// What a hooked shell said last ([`marker`]).
    marks: Mutex<ShellMarks>,
}

/// What a shell with Kynex's hooks said about its commands ([`marker`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ShellMarks {
    /// It has the hooks: it said so at a prompt.
    pub hooked: bool,
    /// Prompts shown so far.
    pub prompts: u64,
    /// The last command that ran and its exit code, once one has.
    pub last: Option<(u64, i32)>,
    /// The command running now, if it says when one starts (PowerShell with PSReadLine,
    /// bash 4.4 and later).
    pub running: Option<u64>,
}

#[derive(Clone)]
struct Listener {
    wake: Wake,
    shared: Arc<Shared>,
    pty: Arc<OnceLock<EventLoopSender>>,
}

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        match event {
            Event::Wakeup => {}
            Event::Title(t) => match marker::parse(&t) {
                Some(Mark::Done { n, code }) => {
                    self.shared.done.lock().unwrap().insert(n, code);
                }
                Some(Mark::Prompt { p, n, code }) => {
                    let mut m = self.shared.marks.lock().unwrap();
                    m.hooked = true;
                    m.prompts = p;
                    m.running = None;
                    // `n` stays 0 in a shell that doesn't say when commands start.
                    m.last = Some((n, code));
                }
                Some(Mark::Exec { n }) => self.shared.marks.lock().unwrap().running = Some(n),
                None => *self.shared.title.lock().unwrap() = Some(t),
            },
            Event::ResetTitle => *self.shared.title.lock().unwrap() = None,
            // Answers to the program's queries (cursor position, colours…).
            Event::PtyWrite(text) => {
                if let Some(pty) = self.pty.get() {
                    let _ = pty.send(Msg::Input(Cow::Owned(text.into_bytes())));
                }
            }
            Event::ClipboardStore(_, text) => *self.shared.clipboard.lock().unwrap() = Some(text),
            Event::ChildExit(_) | Event::Exit => self.shared.exited.store(true, Ordering::SeqCst),
            _ => return,
        }
        (self.wake)();
    }
}

pub struct Terminal {
    term: Arc<FairMutex<Term<Listener>>>,
    pty: EventLoopSender,
    shared: Arc<Shared>,
    size: Size,
    pub shell: ShellSpec,
    pub cwd: PathBuf,
}

impl Terminal {
    /// Starts `shell` in `cwd`. `wake` rings (from another thread) whenever
    /// there's new output to draw.
    pub fn spawn(shell: ShellSpec, cwd: &Path, size: Size, wake: Wake) -> Result<Self, String> {
        let shared = Arc::new(Shared::default());
        let pty_slot = Arc::new(OnceLock::new());
        let listener = Listener { wake: wake.clone(), shared: shared.clone(), pty: pty_slot.clone() };
        let term = Arc::new(FairMutex::new(Term::new(
            Config { scrolling_history: SCROLLBACK, ..Config::default() },
            &size,
            listener.clone(),
        )));
        // Tools installed since Kynex started, then the terminal's own variables.
        let mut env: HashMap<String, String> = crate::fresh_env::vars()
            .into_iter()
            .filter_map(|(n, v)| Some((n.into_string().ok()?, v.into_string().ok()?)))
            .collect();
        env.extend([
            ("TERM".to_string(), "xterm-256color".to_string()),
            ("COLORTERM".to_string(), "truecolor".to_string()),
            ("TERM_PROGRAM".to_string(), "kynex".to_string()),
        ]);
        env.extend(shell.env.iter().cloned());
        let options = tty::Options {
            shell: Some(tty::Shell::new(shell.program.to_string_lossy().into_owned(), shell.args.clone())),
            working_directory: Some(cwd.to_path_buf()),
            drain_on_exit: true,
            env,
            #[cfg(windows)]
            escape_args: true,
        };
        // Last time's scrollback first: the shell's screen clears it up into the history.
        if let Some(text) = shell.history.as_deref() {
            let mut parser: alacritty_terminal::vte::ansi::Processor = Default::default();
            let mut bytes = crate::scrollback::replay(text);
            // ConPTY paints its own (empty) screen as it starts, over whatever is on ours: the
            // text goes up into the scrollback first, where it stays.
            if cfg!(windows) {
                bytes.extend(std::iter::repeat_n(b'\n', size.lines));
            }
            parser.advance(&mut *term.lock(), &bytes);
        }
        let pty = tty::new(&options, window_size(size), 0)
            .map_err(|e| format!("Couldn't start {}: {e}", shell.program.display()))?;
        let event_loop = EventLoop::new(term.clone(), listener, pty, true, false)
            .map_err(|e| format!("Couldn't start the terminal: {e}"))?;
        let sender = event_loop.channel();
        let _ = pty_slot.set(sender.clone());
        // The loop reads the PTY into `term` and rings the listener on new output.
        let wake_on_output = wake;
        std::thread::spawn(move || {
            let handle = event_loop.spawn();
            let _ = handle.join();
            wake_on_output();
        });
        Ok(Self { term, pty: sender, shared, size, shell, cwd: cwd.to_path_buf() })
    }

    /// Sends raw bytes to the shell, as if typed. Jumps back to the bottom.
    pub fn write(&self, bytes: impl Into<Cow<'static, [u8]>>) {
        self.scroll_to_bottom();
        let _ = self.pty.send(Msg::Input(bytes.into()));
    }

    /// Types `command` and presses Enter.
    pub fn run(&self, command: &str) {
        let mut text = command.trim_end().replace("\r\n", "\n").replace('\n', "\r");
        text.push('\r');
        self.write(text.into_bytes());
    }

    /// Types `command` with marker `n` and presses Enter; [`Self::take_done`] says when it
    /// finished and how. False (and nothing typed) for a shell [`marker`] doesn't know.
    pub fn run_marked(&self, command: &str, n: u64) -> bool {
        let Some(text) = marker::wrap(&self.shell.program, command, n) else { return false };
        self.run(&text);
        true
    }

    /// The exit code of the command run with marker `n`, once it has finished.
    pub fn take_done(&self, n: u64) -> Option<i32> {
        self.shared.done.lock().unwrap().remove(&n)
    }

    /// What its shell said last about its commands, when it has Kynex's hooks.
    pub fn marks(&self) -> ShellMarks {
        *self.shared.marks.lock().unwrap()
    }

    /// What a hooked shell's command `n` printed, as far as the scrollback still has it.
    pub fn output(&self, n: u64) -> Option<String> {
        snapshot::linked_text(&self.term.lock(), &marker::output_link(n))
    }

    /// What the command run with marker `n` printed, as far as the scrollback still has it.
    pub fn wrapped_output(&self, n: u64) -> Option<String> {
        let linked = snapshot::linked_text(&self.term.lock(), &marker::wrapped_link(n))?;
        Some(marker::wrapped_output(&linked, n))
    }

    pub fn paste(&self, text: &str) {
        let bracketed = self.term.lock().mode().contains(alacritty_terminal::term::TermMode::BRACKETED_PASTE);
        self.write(paste_bytes(text, bracketed));
    }

    /// Application-cursor mode (vim, less…): arrows send `ESC O` sequences.
    pub fn app_cursor(&self) -> bool {
        self.term.lock().mode().contains(alacritty_terminal::term::TermMode::APP_CURSOR)
    }

    pub fn size(&self) -> Size {
        self.size
    }

    pub fn resize(&mut self, size: Size) {
        if size == self.size || size.columns < 2 || size.lines < 1 {
            return;
        }
        self.size = size;
        self.term.lock().resize(size);
        let _ = self.pty.send(Msg::Resize(window_size(size)));
    }

    /// Scrolls through history: positive is up.
    pub fn scroll(&self, lines: i32) {
        self.term.lock().scroll_display(Scroll::Delta(lines));
    }

    pub fn scroll_to_bottom(&self) {
        let mut term = self.term.lock();
        if term.grid().display_offset() != 0 {
            term.scroll_display(Scroll::Bottom);
        }
    }

    /// Everything it holds as text, scrollback and screen, for keeping across a restart;
    /// `None` while a full-screen program is up.
    pub fn history_text(&self) -> Option<String> {
        snapshot::history_text(&self.term.lock())
    }

    /// The screen's text, as the program last drew it.
    pub fn screen_text(&self) -> String {
        snapshot::screen_text(&self.term.lock())
    }

    /// The last `n` lines of scrollback and screen (only the screen while a full-screen
    /// program is up).
    pub fn last_lines(&self, n: usize) -> String {
        marker::tail(&self.history_text().unwrap_or_else(|| self.screen_text()), n)
    }

    pub fn snapshot(&self, palette: &Palette) -> Snapshot {
        snapshot::snapshot(&self.term.lock(), palette)
    }

    /// The title the shell set, if any.
    pub fn title(&self) -> Option<String> {
        self.shared.title.lock().unwrap().clone()
    }

    /// The shell has exited.
    pub fn exited(&self) -> bool {
        self.shared.exited.load(Ordering::SeqCst)
    }

    /// Text a program asked to copy, once.
    pub fn take_clipboard(&self) -> Option<String> {
        self.shared.clipboard.lock().unwrap().take()
    }

    /// Starts a selection at a visible cell; `right` is which half of it was hit.
    pub fn select_start(&self, line: usize, column: usize, right: bool, by: SelectBy) {
        let mut term = self.term.lock();
        let point = grid_point(&term, line, column);
        let ty = match by {
            SelectBy::Cell => SelectionType::Simple,
            SelectBy::Word => SelectionType::Semantic,
            SelectBy::Line => SelectionType::Lines,
        };
        term.selection = Some(Selection::new(ty, point, side(right)));
    }

    pub fn select_to(&self, line: usize, column: usize, right: bool) {
        let mut term = self.term.lock();
        let point = grid_point(&term, line, column);
        if let Some(sel) = term.selection.as_mut() {
            sel.update(point, side(right));
        }
    }

    pub fn clear_selection(&self) {
        self.term.lock().selection = None;
    }

    /// The selected text, if anything non-empty is selected.
    pub fn selection_text(&self) -> Option<String> {
        self.term.lock().selection_to_string().filter(|s| !s.is_empty())
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = self.pty.send(Msg::Shutdown);
    }
}

fn window_size(size: Size) -> WindowSize {
    WindowSize { num_lines: size.lines as u16, num_cols: size.columns as u16, cell_width: 1, cell_height: 1 }
}

fn side(right: bool) -> Side {
    if right { Side::Right } else { Side::Left }
}

/// A visible cell as a grid point (history lines are negative).
fn grid_point<T>(term: &Term<T>, line: usize, column: usize) -> Point {
    let offset = term.grid().display_offset() as i32;
    let line = (line as i32 - offset).min(term.screen_lines() as i32 - 1);
    Point::new(Line(line), Column(column.min(term.columns().saturating_sub(1))))
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    /// Starts the real default shell, runs a command and waits for its output.
    #[test]
    fn runs_a_command_in_a_real_shell() {
        let spec = resolve_shell(ShellKind::default_for_os()).unwrap();
        let dir = std::env::temp_dir();
        let term = Terminal::spawn(spec, &dir, Size { columns: 80, lines: 24 }, Arc::new(|| {})).unwrap();
        term.run("echo kynex-$((40+2))");
        let palette = snapshot::tests::test_palette();
        let deadline = Instant::now() + Duration::from_secs(20);
        // Both bash and PowerShell expand this; only the output reads 42.
        loop {
            let text: String = term.snapshot(&palette).rows.iter().map(|r| r.text.clone() + "\n").collect();
            if text.contains("kynex-42") {
                break;
            }
            assert!(Instant::now() < deadline, "no output:\n{text}");
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// The two shells the tests run in, where this machine has them: (spec, is PowerShell).
    fn shells() -> Vec<(ShellSpec, bool)> {
        let mut all: Vec<ShellSpec> =
            [ShellKind::PowerShell, ShellKind::Bash].into_iter().filter_map(|k| resolve_shell(k).ok()).collect();
        // Windows PowerShell 5.1 too, where there's pwsh and it.
        if cfg!(windows)
            && let (Some(ps), Some(old)) = (all.first().cloned(), which("powershell"))
            && ps.program != old
        {
            all.push(ShellSpec { program: old, ..ps });
        }
        all.into_iter()
            .map(|s| {
                let stem = s.program.file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
                let powershell = stem.contains("pwsh") || stem.contains("powershell");
                (s, powershell)
            })
            .collect()
    }

    /// A command that fails with 3.
    fn fails_with_3(powershell: bool) -> &'static str {
        if powershell && cfg!(windows) { "cmd /c exit 3" } else { "sh -c 'exit 3'" }
    }

    fn spawn(spec: ShellSpec) -> Terminal {
        Terminal::spawn(spec, &std::env::temp_dir(), Size { columns: 120, lines: 24 }, Arc::new(|| {})).unwrap()
    }

    /// A shell with the hooks says when each command ends, how, and what it printed; the tab
    /// title is left alone.
    #[test]
    fn a_hooked_shell_tells_how_each_command_ended() {
        for (spec, powershell) in shells() {
            let bash = spec.program.file_stem().is_some_and(|s| s.eq_ignore_ascii_case("bash"));
            if !powershell && !bash {
                continue;
            }
            let term = spawn(spec);
            let after = |p0: u64| {
                let deadline = Instant::now() + Duration::from_secs(20);
                loop {
                    let m = term.marks();
                    if m.prompts > p0 {
                        return m;
                    }
                    assert!(Instant::now() < deadline, "no prompt after {p0}:\n{}", term.screen_text());
                    std::thread::sleep(Duration::from_millis(50));
                }
            };
            let first = after(0);
            assert!(first.hooked && hooks::has_hooks(&term.shell));
            term.run("echo one; echo two # a comment");
            let m = after(first.prompts);
            let (n, code) = m.last.unwrap();
            assert_eq!(code, 0);
            assert_eq!(term.output(n).unwrap().trim_end(), "one\ntwo", "{}", term.screen_text());
            term.run(fails_with_3(powershell));
            let m = after(m.prompts);
            assert_eq!(m.last.unwrap().1, 3);
            if powershell {
                // A failed cmdlet after the native 3 is 1, not that 3 again; the same native
                // failure twice is 3 both times.
                term.run("Get-Item no-such-kynex-file");
                let m = after(m.prompts);
                assert_eq!(m.last.unwrap().1, 1);
                term.run(fails_with_3(powershell));
                let m = after(m.prompts);
                assert_eq!(m.last.unwrap().1, 3);
                term.run(fails_with_3(powershell));
                let m = after(m.prompts);
                assert_eq!(m.last.unwrap().1, 3);
                // The prompt the hooks wrap sees the command's failure.
                term.run("$global:__kynex.prompt = { \"ok=$? > \" }");
                let m = after(m.prompts);
                term.run("Get-Item no-such-kynex-file");
                after(m.prompts);
                assert!(term.screen_text().contains("ok=False >"), "{}", term.screen_text());
            }
            assert!(!term.title().unwrap_or_default().contains("knx-"));
        }
    }
    /// A marked command tells when it finished and its exit code, and the tab title is left alone.
    #[test]
    fn a_marked_command_tells_its_exit_code() {
        for (mut spec, powershell) in shells() {
            // As without Kynex's hooks, which would set titles over the marker's.
            spec.env.clear();
            spec.args = if powershell { vec!["-NoLogo".into()] } else { vec!["--login".into(), "-i".into()] };
            assert!(!hooks::has_hooks(&spec));
            marked_commands_in(spec, powershell);
        }
    }

    fn marked_commands_in(spec: ShellSpec, powershell: bool) {
        let fail = fails_with_3(powershell);
        let term = spawn(spec);
        let exit = |n: u64| {
            let deadline = Instant::now() + Duration::from_secs(20);
            loop {
                if let Some(code) = term.take_done(n) {
                    return code;
                }
                assert!(Instant::now() < deadline, "marker {n} never came:\n{}", term.screen_text());
                std::thread::sleep(Duration::from_millis(100));
            }
        };
        assert!(term.run_marked("echo hi # a comment", 1));
        assert_eq!(exit(1), 0);
        assert_eq!(term.wrapped_output(1).unwrap().trim_end(), "hi", "{}", term.screen_text());
        term.run_marked(fail, 2);
        assert_eq!(exit(2), 3);
        if powershell {
            // A failed cmdlet is a failure too, and the native command's 3 doesn't stand in for it.
            term.run_marked("Get-Item no-such-kynex-file", 3);
            assert_eq!(exit(3), 1);
        }
        term.run_marked("echo again;", 4);
        assert_eq!(exit(4), 0);
        assert!(!term.title().unwrap_or_default().contains("knx-done"));
    }
}
