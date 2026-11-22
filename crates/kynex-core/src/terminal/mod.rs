//! An interactive terminal: a shell on a pseudo-terminal (ConPTY on Windows),
//! parsed by `alacritty_terminal` into a grid of cells. The app draws
//! [`Snapshot`]s of it and sends keys; this knows nothing about drawing.
//!
//! The shell inherits Kynex's own rights, so when Kynex runs as administrator
//! its terminals do too ([`elevated`] says which).

pub mod hooks;
mod keys;
pub(crate) mod kitty;
mod links;
pub mod marker;
mod paste;
mod search;
mod shell;
mod snapshot;

use std::borrow::Cow;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
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

pub use keys::{Key, KeyMode, MouseBtn, MouseEventKind, key_bytes, mouse_bytes, paste_bytes};
pub use kitty::{KittyImageData, PlacedImage};
pub use links::{LinkSpan, LinkTarget, TerminalLink, find_urls};
use marker::Mark;
pub use paste::{format_paths_for_paste, is_image_path, pasted_dir, save_pasted_image, save_pasted_image_in};
pub use search::{CopyMotion, Match};
pub use shell::{ShellKind, ShellSpec, elevated, resolve as resolve_shell, which};
pub use snapshot::{Cursor, CursorStyle, Palette, Rgb, Row, Run, Snapshot};

/// Callback invoked from a background thread when new terminal output or events arrive.
pub type Wake = Arc<dyn Fn() + Send + Sync>;

/// Internal trampoline flag used by [`Terminal::spawn`] when inherited environment variables
/// must be unset before executing a CLI inside a PTY (only invoked when conflicting vars exist).
const PTY_EXEC_FLAG: &str = "--pty-exec";

/// Runs the requested program with specified env vars removed if invoked with `--pty-exec`.
/// Call at the start of `main()`. Returns `None` for normal app invocations.
pub fn pty_exec_if_asked() -> Option<i32> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) != Some(PTY_EXEC_FLAG) {
        return None;
    }
    let mut unset: Vec<String> = Vec::new();
    let mut i = 1;
    while i < args.len() {
        if args[i] == "--unset"
            && let Some(name) = args.get(i + 1)
        {
            unset.push(name.clone());
            i += 2;
        } else if args[i] == "--" {
            i += 1;
            break;
        } else {
            break;
        }
    }
    let Some(program) = args.get(i) else { return Some(1) };
    let rest = &args[i + 1..];
    let mut cmd = std::process::Command::new(program);
    cmd.args(rest);
    for name in &unset {
        cmd.env_remove(name);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = cmd.exec();
        eprintln!("kynex: failed to exec {program}: {err}");
        Some(127)
    }
    #[cfg(not(unix))]
    {
        match cmd.status() {
            Ok(status) => Some(status.code().unwrap_or(1)),
            Err(err) => {
                eprintln!("kynex: failed to start {program}: {err}");
                Some(127)
            }
        }
    }
}

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
struct Shared {
    title: Mutex<Option<String>>,
    exited: AtomicBool,
    /// Text the program asked to copy (OSC 52), for the app to put on the clipboard.
    clipboard: Mutex<Option<String>>,
    /// Exit codes of marked commands that finished ([`marker`]), by marker.
    done: Mutex<HashMap<u64, i32>>,
    /// What a hooked shell said last ([`marker`]).
    marks: Mutex<ShellMarks>,
    /// Kitty graphics image cache and placements.
    kitty: Arc<Mutex<kitty::KittyStore>>,
    /// Pending Kitty cursor-position probe sequence numbers awaiting their `CSI row;colR` reply.
    pending_cpr: Mutex<VecDeque<u64>>,
}

impl Shared {
    fn new(kitty: Arc<Mutex<kitty::KittyStore>>) -> Self {
        Self {
            title: Mutex::new(None),
            exited: AtomicBool::new(false),
            clipboard: Mutex::new(None),
            done: Mutex::new(HashMap::new()),
            marks: Mutex::new(ShellMarks::default()),
            kitty,
            pending_cpr: Mutex::new(VecDeque::new()),
        }
    }
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
            Event::Title(t) => {
                if let Some(seq) = t.strip_prefix(kitty::PROBE_PREFIX).and_then(|s| s.parse().ok()) {
                    self.shared.pending_cpr.lock().unwrap().push_back(seq);
                    return;
                }
                match marker::parse(&t) {
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
                }
            }
            Event::ResetTitle => *self.shared.title.lock().unwrap() = None,
            // Answers to the program's queries (cursor position, colours…), plus our own
            // internal Kitty cursor-position probes (`knx-kg:<seq>` + `CSI 6n`).
            Event::PtyWrite(text) => {
                if let Some((line, col)) = kitty::parse_cpr(&text)
                    && let Some(seq) = self.shared.pending_cpr.lock().unwrap().pop_front()
                {
                    self.shared.kitty.lock().unwrap().resolve_cursor(seq, line, col);
                } else if let Some(pty) = self.pty.get() {
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
    /// The matches of the last find, tinted in snapshots.
    find: Mutex<Option<search::FindState>>,
    size: Size,
    cell_px: Arc<AtomicU32>,
    term_cols: Arc<AtomicU32>,
    pub shell: ShellSpec,
    pub cwd: PathBuf,
}

impl Terminal {
    /// Starts `shell` in `cwd`. `wake` rings (from another thread) whenever
    /// there's new output to draw.
    pub fn spawn(shell: ShellSpec, cwd: &Path, size: Size, wake: Wake) -> Result<Self, String> {
        let cell_px = Arc::new(AtomicU32::new((8 << 16) | 16));
        let term_cols = Arc::new(AtomicU32::new(size.columns.max(2) as u32));
        let kitty = Arc::new(Mutex::new(kitty::KittyStore::new(cell_px.clone(), term_cols.clone())));
        let shared = Arc::new(Shared::new(kitty.clone()));
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
        // Replace existing keys case-insensitively on Windows so `"PATH"` cleanly overrides
        // `"Path"` instead of leaving duplicate entries in the environment block.
        merge_env(&mut env, &shell.env);
        // Check whether any variable in `env_remove` is actually set in the environment we'd
        // inherit (excluding variables we explicitly re-set in `shell.env`).
        let conflicts: Vec<String> = shell
            .env_remove
            .iter()
            .filter(|name| {
                let overridden = shell.env.iter().any(|(k, _)| same_env_name(k, name));
                if overridden {
                    return false;
                }
                let in_fresh = env.keys().any(|k| same_env_name(k, name));
                let in_parent = std::env::vars_os().any(|(k, _)| k.to_str().is_some_and(|s| same_env_name(s, name)));
                in_fresh || in_parent
            })
            .cloned()
            .collect();
        for name in &conflicts {
            env.retain(|k, _| !same_env_name(k, name));
        }
        let tty_shell = if !conflicts.is_empty()
            && let Ok(exe) = std::env::current_exe()
        {
            let mut argv = vec![PTY_EXEC_FLAG.to_string()];
            for name in &conflicts {
                argv.push("--unset".into());
                argv.push(name.clone());
            }
            argv.push("--".into());
            argv.push(shell.program.to_string_lossy().into_owned());
            argv.extend(shell.args.iter().cloned());
            tty::Shell::new(exe.to_string_lossy().into_owned(), argv)
        } else {
            tty::Shell::new(shell.program.to_string_lossy().into_owned(), shell.args.clone())
        };
        let options = tty::Options {
            shell: Some(tty_shell),
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
        let reply_pty = pty_slot.clone();
        let reply: kitty::ReplyFn = Arc::new(move |bytes| {
            if let Some(tx) = reply_pty.get() {
                let _ = tx.send(Msg::Input(Cow::Owned(bytes)));
            }
        });
        let pty = kitty::InterceptPty::new(pty, kitty, reply, wake.clone());
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
        Ok(Self {
            term,
            pty: sender,
            shared,
            find: Mutex::new(None),
            size,
            cell_px,
            term_cols,
            shell,
            cwd: cwd.to_path_buf(),
        })
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

    /// Keyboard encoding modes currently requested by the program (app cursor, Kitty protocol).
    pub fn key_mode(&self) -> KeyMode {
        let mode = *self.term.lock().mode();
        use alacritty_terminal::term::TermMode;
        KeyMode {
            app_cursor: mode.contains(TermMode::APP_CURSOR),
            kitty_disambiguate: mode.contains(TermMode::DISAMBIGUATE_ESC_CODES),
            kitty_report_all: mode.contains(TermMode::REPORT_ALL_KEYS_AS_ESC),
            kitty_associated_text: mode.contains(TermMode::REPORT_ASSOCIATED_TEXT),
        }
    }

    /// Application-cursor mode (vim, less…): arrows send `ESC O` sequences.
    pub fn app_cursor(&self) -> bool {
        self.term.lock().mode().contains(alacritty_terminal::term::TermMode::APP_CURSOR)
    }

    /// The running program asked for mouse events (`?1000h` / `?1002h` / `?1003h`).
    pub fn mouse_mode(&self) -> bool {
        self.term.lock().mode().intersects(alacritty_terminal::term::TermMode::MOUSE_MODE)
    }

    /// Sends a mouse event to the program if it asked for mouse reporting.
    pub fn send_mouse(&self, kind: MouseEventKind, line: usize, col: usize, shift: bool, alt: bool, ctrl: bool) {
        let mode = *self.term.lock().mode();
        use alacritty_terminal::term::TermMode;
        if !mode.intersects(TermMode::MOUSE_MODE) {
            return;
        }
        if matches!(kind, MouseEventKind::Drag(_)) && !mode.intersects(TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION) {
            return;
        }
        let sgr = mode.contains(TermMode::SGR_MOUSE);
        if let Some(bytes) = mouse_bytes(kind, line, col, shift, alt, ctrl, sgr) {
            let _ = self.pty.send(Msg::Input(Cow::Owned(bytes)));
        }
    }

    /// Whether the alternate screen buffer is active (full-screen TUIs, pagers).
    pub fn alt_screen(&self) -> bool {
        self.term.lock().mode().contains(alacritty_terminal::term::TermMode::ALT_SCREEN)
    }

    /// Whether wheel scroll on the alternate screen should translate to Up/Down arrows.
    pub fn alternate_scroll(&self) -> bool {
        self.term.lock().mode().contains(alacritty_terminal::term::TermMode::ALTERNATE_SCROLL)
    }

    /// Notifies the program of focus gain/loss when it requested DECSET 1004 (`FOCUS_IN_OUT`).
    pub fn focus_changed(&self, focused: bool) {
        if self.term.lock().mode().contains(alacritty_terminal::term::TermMode::FOCUS_IN_OUT) {
            let seq: &'static [u8] = if focused { b"\x1b[I" } else { b"\x1b[O" };
            let _ = self.pty.send(Msg::Input(Cow::Borrowed(seq)));
        }
    }

    /// Finds a clickable URL, OSC 8 hyperlink, or local file path under visible `(line, column)`.
    pub fn link_at(&self, line: usize, column: usize) -> Option<TerminalLink> {
        links::link_at(&self.term.lock(), &self.cwd, line, column)
    }

    pub fn size(&self) -> Size {
        self.size
    }

    pub fn resize(&mut self, size: Size) {
        self.resize_with_cell(size, 8, 16);
    }

    pub fn resize_with_cell(&mut self, size: Size, cell_width: u16, cell_height: u16) {
        let cw = cell_width.max(1);
        let ch = cell_height.max(1);
        self.cell_px.store((u32::from(cw) << 16) | u32::from(ch), Ordering::Relaxed);
        if size.columns >= 2 {
            self.term_cols.store(size.columns as u32, Ordering::Relaxed);
        }
        if size == self.size || size.columns < 2 || size.lines < 1 {
            return;
        }
        self.size = size;
        self.term.lock().resize(size);
        let _ = self.pty.send(Msg::Resize(WindowSize {
            num_lines: size.lines as u16,
            num_cols: size.columns as u16,
            cell_width: cw,
            cell_height: ch,
        }));
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
        snapshot::snapshot(&self.term.lock(), palette, self.find.lock().unwrap().as_ref(), Some(&self.shared.kitty))
    }

    /// Searches the scrollback and screen for `query` (ignoring case), tints the matches and
    /// makes the newest one current, scrolled into view. Returns how many there are. Matches
    /// are of the text as it is now: search again once more output has come.
    pub fn find(&self, query: &str) -> usize {
        let mut term = self.term.lock();
        let matches = search::find(&term, query);
        let count = matches.len();
        let current = count.saturating_sub(1);
        if let Some(m) = matches.get(current) {
            search::reveal(&mut term, m);
        }
        *self.find.lock().unwrap() = (count > 0).then_some(search::FindState { matches, current });
        count
    }

    /// Makes the next (or previous) match current, wrapping around, and scrolls to it.
    /// Returns its 1-based position and how many there are; `None` with no matches.
    pub fn find_step(&self, forward: bool) -> Option<(usize, usize)> {
        let mut term = self.term.lock();
        let mut guard = self.find.lock().unwrap();
        let state = guard.as_mut()?;
        let n = state.matches.len();
        state.current = if forward { (state.current + 1) % n } else { (state.current + n - 1) % n };
        search::reveal(&mut term, &state.matches[state.current]);
        Some((state.current + 1, n))
    }

    /// Drops the find's tint.
    pub fn find_clear(&self) {
        *self.find.lock().unwrap() = None;
    }

    /// The matches of the last find.
    pub fn find_matches(&self) -> Vec<Match> {
        self.find.lock().unwrap().as_ref().map(|f| f.matches.clone()).unwrap_or_default()
    }

    /// In copy mode: keys move a cursor over the scrollback instead of going to the shell.
    pub fn copy_mode(&self) -> bool {
        search::copy_mode(&self.term.lock())
    }

    /// Enters copy mode, or leaves it (dropping the selection).
    pub fn toggle_copy_mode(&self) {
        search::toggle_copy_mode(&mut self.term.lock());
    }

    pub fn copy_motion(&self, motion: CopyMotion) {
        search::copy_motion(&mut self.term.lock(), motion);
    }

    /// Starts a selection at the copy-mode cursor (cell, word or line), or ends it.
    pub fn copy_select(&self, by: SelectBy) {
        search::copy_select(&mut self.term.lock(), by);
    }

    /// Leaves copy mode, returning what was selected, if anything.
    pub fn copy_yank(&self) -> Option<String> {
        let mut term = self.term.lock();
        let text = term.selection_to_string().filter(|s| !s.is_empty());
        if search::copy_mode(&term) {
            search::toggle_copy_mode(&mut term);
        }
        text
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

/// Variable names compare without case on Windows, exactly elsewhere.
fn same_env_name(a: &str, b: &str) -> bool {
    if cfg!(windows) { a.eq_ignore_ascii_case(b) } else { a == b }
}

/// Inserts `overrides` into `env`, removing any existing key that matches [`same_env_name`]
/// so `"PATH"` cleanly replaces Windows `"Path"` instead of producing duplicate entries.
fn merge_env(env: &mut HashMap<String, String>, overrides: &[(String, String)]) {
    for (k, v) in overrides {
        env.retain(|existing, _| !same_env_name(existing, k));
        env.insert(k.clone(), v.clone());
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

    #[test]
    fn merge_env_replaces_case_insensitively_on_windows() {
        let mut env = HashMap::from([("Path".into(), "old".into()), ("KEEP".into(), "1".into())]);
        merge_env(&mut env, &[("PATH".into(), "new".into())]);
        assert_eq!(env.get("PATH").map(String::as_str), Some("new"));
        assert_eq!(env.get("KEEP").map(String::as_str), Some("1"));
        if cfg!(windows) {
            assert!(!env.contains_key("Path"));
        }
    }

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

    #[cfg_attr(not(unix), allow(unused_mut))]
    fn spawn(mut spec: ShellSpec) -> Terminal {
        // On CI's Linux, .NET can't make its shared-memory folder under the runtime folder
        // (`/run/user/<uid>`), so PowerShell's line editor fails to start (its history mutex
        // lives there) and the shell falls back to a plain reader that shows typed lines
        // differently. Each shell gets a runtime and temp folder of its own (tests run at once).
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!("kynex-test-runtime-{}-{n}", std::process::id()));
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
            let dir = dir.display().to_string();
            spec.env.push(("XDG_RUNTIME_DIR".into(), dir.clone()));
            spec.env.push(("TMPDIR".into(), dir));
        }
        Terminal::spawn(spec, &std::env::temp_dir(), Size { columns: 120, lines: 24 }, Arc::new(|| {})).unwrap()
    }

    /// A shell with the hooks says when each command ends, how, and what it printed; the tab
    /// title is left alone.
    #[test]
    fn a_hooked_shell_tells_how_each_command_ended() {
        for (spec, powershell) in shells() {
            let bash = spec.program.file_stem().is_some_and(|s| s.eq_ignore_ascii_case("bash"));
            // PowerShell off Windows loses an Enter typed before its line editor reads (#475).
            if (!powershell && !bash) || (powershell && !cfg!(windows)) {
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
            // bash before 4.4 (macOS's /bin/bash) has no PS0: no start mark, so no output to read.
            if n > 0 {
                assert_eq!(term.output(n).unwrap().trim_end(), "one\ntwo", "{}", term.screen_text());
            }
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
                // The mark comes as the prompt starts; its text is drawn just after.
                let deadline = Instant::now() + Duration::from_secs(10);
                while !term.screen_text().contains("ok=False >") {
                    assert!(Instant::now() < deadline, "{}", term.screen_text());
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
            assert!(!term.title().unwrap_or_default().contains("knx-"));
        }
    }
    /// A marked command tells when it finished and its exit code, and the tab title is left alone.
    #[test]
    fn a_marked_command_tells_its_exit_code() {
        for (mut spec, powershell) in shells() {
            // PowerShell off Windows loses an Enter typed before its line editor reads (#475).
            if powershell && !cfg!(windows) {
                continue;
            }
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
