//! An interactive terminal: a shell on a pseudo-terminal (ConPTY on Windows),
//! parsed by `alacritty_terminal` into a grid of cells. The app draws
//! [`Snapshot`]s of it and sends keys; this knows nothing about drawing.
//!
//! The shell inherits Kynex's own rights, so when Kynex runs as administrator
//! its terminals do too ([`elevated`] says which).

mod keys;
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
pub use shell::{ShellKind, ShellSpec, elevated, resolve as resolve_shell};
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
            Event::Title(t) => *self.shared.title.lock().unwrap() = Some(t),
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
        let env = HashMap::from([
            ("TERM".to_string(), "xterm-256color".to_string()),
            ("COLORTERM".to_string(), "truecolor".to_string()),
            ("TERM_PROGRAM".to_string(), "kynex".to_string()),
        ]);
        let options = tty::Options {
            shell: Some(tty::Shell::new(shell.program.to_string_lossy().into_owned(), shell.args.clone())),
            working_directory: Some(cwd.to_path_buf()),
            drain_on_exit: true,
            env,
            #[cfg(windows)]
            escape_args: true,
        };
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
}
