//! `!` commands: run in the session's folder with the user's shell, shown in the
//! transcript, and sent to Claude with the next message the way the CLI records them
//! in its log: `<bash-input>` with the command, then `<bash-stdout>` / `<bash-stderr>`
//! (and `<bash-exit-code>`, as the CLI's headless runs add). A resumed session shows them again.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::terminal::{ShellKind, resolve_shell as resolve};

/// Longest a command may run before it is stopped.
pub const TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// Most characters of each stream kept; the CLI's Bash tool keeps as many.
pub const MAX_OUTPUT: usize = 30_000;

/// What a command printed, and how it ended.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Output {
    pub stdout: String,
    pub stderr: String,
    /// Its exit code; `None` when that isn't known (a log of the CLI's own `!`).
    pub code: Option<i32>,
}

impl Output {
    pub fn failed(&self) -> bool {
        self.code.is_some_and(|c| c != 0)
    }
}

/// A `!` command in a transcript.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    /// Tells this run apart while it's running.
    pub id: u64,
    pub command: String,
    /// `None` while it runs.
    pub output: Option<Output>,
    /// It went to Claude already (with a message, or it came from the log).
    pub sent: bool,
}

/// A piece of a logged user message that is a `!` command or its output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Part {
    Input(String),
    Output(Output),
}

/// The blocks that tell Claude about `run`, in the CLI's tags: nothing while it runs.
pub fn blocks(run: &Run) -> Vec<String> {
    let Some(out) = &run.output else { return Vec::new() };
    let mut result = format!("<bash-stdout>{}</bash-stdout><bash-stderr>{}</bash-stderr>", out.stdout, out.stderr);
    if let Some(code) = out.code {
        result.push_str(&format!("<bash-exit-code>{code}</bash-exit-code>"));
    }
    vec![format!("<bash-input>{}</bash-input>", run.command), result]
}

/// The `!` command or output `text` (one block of a logged user message) holds, if any.
pub fn parse(text: &str) -> Option<Part> {
    let t = text.trim();
    if let Some(cmd) = between(t, "bash-input").filter(|_| t.starts_with("<bash-input>")) {
        return Some(Part::Input(cmd.to_string()));
    }
    if !(t.starts_with("<bash-stdout>") || t.starts_with("<bash-stderr>")) {
        return None;
    }
    let stdout = between(t, "bash-stdout").unwrap_or_default().to_string();
    let stderr = between(t, "bash-stderr").unwrap_or_default().to_string();
    let code = between(t, "bash-exit-code").and_then(|c| c.trim().parse().ok());
    Some(Part::Output(Output { stdout, stderr, code }))
}

/// The text inside the first `<tag>…</tag>` in `t`.
fn between<'a>(t: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let start = t.find(&open)? + open.len();
    let end = t[start..].find(&format!("</{tag}>"))? + start;
    Some(&t[start..end])
}

/// The command that runs `line` with `kind`'s shell, found at `program`.
pub fn command(kind: ShellKind, program: &Path, line: &str) -> Command {
    let mut cmd = Command::new(program);
    match kind {
        ShellKind::PowerShell => {
            // Output as UTF-8, whatever the console's code page.
            let line = format!("[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false); {line}");
            cmd.args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", &line]);
        }
        ShellKind::Bash => {
            cmd.args(["-lc", line]);
        }
    }
    cmd
}

/// Runs `line` in `cwd` with the shell the user picked for terminals, and waits for it
/// (stopped after [`TIMEOUT`]). Blocks. A shell that won't start is reported in
/// `stderr`, as the CLI does.
pub fn run(cwd: &Path, line: &str) -> Output {
    let kind = ShellKind::default_shell();
    match resolve(kind) {
        Ok(spec) => run_with(command(kind, &spec.program, line), cwd, TIMEOUT),
        Err(e) => Output { stdout: String::new(), stderr: format!("Command failed: {e}"), code: None },
    }
}

fn run_with(mut cmd: Command, cwd: &Path, timeout: Duration) -> Output {
    cmd.current_dir(cwd).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = match crate::process::dies_with_parent(&mut cmd).spawn() {
        Ok(c) => c,
        Err(e) => return Output { stdout: String::new(), stderr: format!("Command failed: {e}"), code: None },
    };
    let read = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut p) = pipe {
                let _ = p.read_to_end(&mut buf);
            }
            buf
        })
    };
    let out = read(child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let err = read(child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let started = Instant::now();
    let (code, timed_out) = loop {
        match child.try_wait() {
            Ok(Some(status)) => break (status.code(), false),
            Ok(None) if started.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                break (Some(124), true);
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => break (None, false),
        }
    };
    let text = |h: std::thread::JoinHandle<Vec<u8>>| clean(&String::from_utf8_lossy(&h.join().unwrap_or_default()));
    let (stdout, mut stderr) = (text(out), text(err));
    if timed_out {
        let after = match timeout.as_secs() {
            s if s >= 60 => format!("{} minutes", s / 60),
            s => format!("{s} seconds"),
        };
        stderr.push_str(&format!("\nCommand timed out after {after}"));
    }
    Output { stdout, stderr, code }
}

/// Output as it's shown and sent: no colour codes or carriage returns, no trailing
/// blank lines, and at most [`MAX_OUTPUT`] characters.
pub fn clean(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            // CSI (`ESC [ … final`) and OSC (`ESC ] … BEL`) sequences.
            '\u{1b}' if chars.peek() == Some(&'[') => {
                chars.next();
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            '\u{1b}' if chars.peek() == Some(&']') => {
                for c in chars.by_ref() {
                    if c == '\u{7}' {
                        break;
                    }
                }
            }
            '\u{1b}' | '\r' => {}
            c => out.push(c),
        }
    }
    let out = out.trim_end();
    match out.char_indices().nth(MAX_OUTPUT) {
        Some((cut, _)) => {
            let more = out[cut..].chars().count();
            format!("{} [output truncated: {more} more chars]", &out[..cut])
        }
        None => out.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_of(command: &str, output: Option<Output>) -> Run {
        Run { id: 1, command: command.into(), output, sent: false }
    }

    #[test]
    fn blocks_use_the_cli_tags_and_parse_back() {
        let out = Output { stdout: "a\nb".into(), stderr: String::new(), code: Some(0) };
        let b = blocks(&run_of("ls", Some(out.clone())));
        assert_eq!(
            b,
            [
                "<bash-input>ls</bash-input>",
                "<bash-stdout>a\nb</bash-stdout><bash-stderr></bash-stderr><bash-exit-code>0</bash-exit-code>"
            ]
        );
        assert_eq!(parse(&b[0]), Some(Part::Input("ls".into())));
        assert_eq!(parse(&b[1]), Some(Part::Output(out)));
        // A failure carries its exit code.
        let failed = Output { stdout: String::new(), stderr: "no".into(), code: Some(2) };
        let b = blocks(&run_of("false", Some(failed.clone())));
        assert!(b[1].ends_with("<bash-exit-code>2</bash-exit-code>"));
        assert_eq!(parse(&b[1]), Some(Part::Output(failed)));
        // Nothing to send while it runs.
        assert!(blocks(&run_of("sleep 9", None)).is_empty());
    }

    #[test]
    fn parses_what_the_cli_logs() {
        assert_eq!(
            parse("<bash-stderr>Command failed: not found</bash-stderr>"),
            Some(Part::Output(Output {
                stdout: String::new(),
                stderr: "Command failed: not found".into(),
                code: None
            }))
        );
        assert_eq!(parse("check <bash-input>ls</bash-input>"), None);
        assert_eq!(parse("plain text"), None);
        assert_eq!(parse("<bash-input>unclosed"), None);
    }

    #[test]
    fn clean_strips_colours_and_caps_length() {
        assert_eq!(clean("\u{1b}[31mred\u{1b}[0m\r\n\n"), "red");
        assert_eq!(clean("\u{1b}]0;title\u{7}ok"), "ok");
        let long = "x".repeat(MAX_OUTPUT + 5);
        assert_eq!(clean(&long), format!("{} [output truncated: 5 more chars]", "x".repeat(MAX_OUTPUT)));
    }

    #[test]
    fn runs_a_command_in_the_folder() {
        let dir = std::env::temp_dir();
        let kind = ShellKind::default_for_os();
        let Ok(spec) = resolve(kind) else { return };
        let out = run_with(command(kind, &spec.program, "echo hello"), &dir, TIMEOUT);
        assert_eq!(out.stdout, "hello");
        assert_eq!(out.code, Some(0));
        let out = run_with(command(kind, &spec.program, "exit 3"), &dir, TIMEOUT);
        assert_eq!(out.code, Some(3));
    }

    #[test]
    fn a_command_that_runs_too_long_is_stopped() {
        let kind = ShellKind::default_for_os();
        let Ok(spec) = resolve(kind) else { return };
        let sleep = if kind == ShellKind::PowerShell { "Start-Sleep 30" } else { "sleep 30" };
        let out = run_with(command(kind, &spec.program, sleep), &std::env::temp_dir(), Duration::from_millis(300));
        assert_eq!(out.code, Some(124));
        assert!(out.stderr.contains("timed out"));
    }
}
