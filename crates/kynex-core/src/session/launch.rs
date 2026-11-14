//! Starting a session's process off the UI thread.
//!
//! Everything a start does can block: reading settings, finding the CLI on
//! `PATH`, and the process itself (long on a first launch, or under an
//! antivirus scan). [`begin`] does all of it on a thread and hands back a
//! [`Started`]; the session takes it in on its next poll, and what it wrote
//! meanwhile goes out then.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;

use serde_json::Value;

use super::{Event, Kind, Wake, claude_command, spawn_readers};
use crate::project;
use crate::provider::{Launch, Provider};

/// What a start needs from its session, taken so it can run on a thread.
pub(super) struct Spec {
    /// The session's uid, which a `knx` run inside it says it comes from.
    pub uid: u64,
    pub kind: Kind,
    pub cwd: PathBuf,
    /// Whose Kynex settings it runs under: its project's, for a session in one of its contexts.
    pub settings_dir: PathBuf,
    pub model: String,
    pub provider: Launch,
    pub mode: project::Mode,
    pub resume: Option<String>,
    pub resume_at: Option<String>,
    pub wake: Wake,
}

/// A finished start: the process, or why there is none. A process that is
/// never taken (the session stopped it meanwhile) is killed when this drops.
pub(super) struct Started {
    pub child: Option<Child>,
    /// Lines for its stdin ([`spawn_writer`]).
    pub stdin: Option<Sender<String>>,
    pub error: Option<String>,
    pub launched: Option<project::Launched>,
    /// The model it runs on.
    pub model: String,
}

impl Drop for Started {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Starts `spec` on a thread. Its events go to `events`; the result comes on
/// the returned receiver, and `spec.wake` rings once it's there.
pub(super) fn begin(spec: Spec, events: Sender<Event>) -> Receiver<Started> {
    let (tx, rx) = channel();
    thread::spawn(move || {
        let wake = spec.wake.clone();
        // A send fails if the session stopped meanwhile; the process drops with it.
        let _ = tx.send(run(spec, events));
        wake();
    });
    rx
}

/// Builds the command and starts it. Blocks.
fn run(mut spec: Spec, events: Sender<Event>) -> Started {
    let mut started = Started { child: None, stdin: None, error: None, launched: None, model: spec.model.clone() };
    let built = command(&mut spec, &mut started);
    started.model = spec.model;
    let (mut cmd, what) = match built {
        Ok(c) => c,
        Err(e) => {
            started.error = Some(e);
            return started;
        }
    };
    // A session with tools can call `knx`; a plain chat has none.
    if spec.kind == Kind::Code {
        kynex_env(&mut cmd, spec.uid, &spec.cwd);
    }
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    match crate::process::dies_with_parent(&mut cmd).spawn() {
        Ok(mut child) => {
            let (stdout, stderr) = (child.stdout.take().unwrap(), child.stderr.take().unwrap());
            let stdin = child.stdin.take().unwrap();
            started.stdin = Some(spawn_writer(stdin, events.clone(), spec.wake.clone()));
            spawn_readers(stdout, stderr, events, spec.wake);
            started.child = Some(child);
        }
        Err(e) => started.error = Some(format!("failed to start {what}: {e}")),
    }
    started
}

/// Tells the process it runs in Kynex: who it is, where the app listens, and a `knx` for
/// this Kynex first on its `PATH` (the one it would get otherwise, Windows' fresh one included).
fn kynex_env(cmd: &mut Command, uid: u64, cwd: &Path) {
    use crate::control::agent;
    for (name, value) in agent::session_env(uid, cwd, agent::control_address().as_deref()) {
        cmd.env(name, value);
    }
    if let Some(dir) = agent::ensure_shims() {
        let set =
            cmd.get_envs().find(|(k, _)| k.eq_ignore_ascii_case("PATH")).and_then(|(_, v)| v.map(|v| v.to_owned()));
        let path = set.or_else(|| std::env::var_os("PATH"));
        cmd.env("PATH", agent::path_with(dir, path.as_deref()));
    }
}

/// Writes the lines sent on the returned sender to `stdin`, on a thread: one
/// the process isn't reading yet blocks there, not the caller. Stops when the
/// sender drops (closing `stdin`), or, saying [`Event::Lost`], when a write fails.
fn spawn_writer(mut stdin: ChildStdin, events: Sender<Event>, wake: Wake) -> Sender<String> {
    let (tx, rx) = channel::<String>();
    thread::spawn(move || {
        for line in rx {
            if stdin.write_all(line.as_bytes()).and_then(|_| stdin.flush()).is_err() {
                let _ = events.send(Event::Lost);
                wake();
                return;
            }
        }
    });
    tx
}

/// The command for `spec` and what it starts (for an error). Fills in what
/// `started` reports besides the process.
fn command(spec: &mut Spec, started: &mut Started) -> Result<(Command, &'static str), String> {
    // Grok: its agent over ACP; the session speaks the protocol.
    if spec.provider.provider() == Provider::Grok {
        return Ok((crate::grok::command(&spec.cwd), "`grok`"));
    }
    // OpenRouter chat: Kynex's own harness in the CLI's place, same protocol.
    if let Launch::OpenRouterChat { key, settings } = spec.provider.with_model(&spec.model) {
        let at = spec.resume_at.take();
        let cmd = crate::chat_api::command(&spec.cwd, &key, &settings, spec.resume.as_deref(), at.as_deref())?;
        return Ok((cmd, "the chat"));
    }
    let mut cmd = claude_command(&spec.cwd, &spec.provider.with_model(&spec.model));
    // Checkpoints of the files Claude edits, so a rewind can restore them.
    cmd.env(crate::rewind::CHECKPOINTS_ENV, "1");
    // It runs in Kynex and has `knx` (see `kynex_env`). Grok's `agent stdio` takes no rules.
    if spec.kind == Kind::Code {
        cmd.args(["--append-system-prompt", crate::control::agent::NOTE]);
    }
    cmd.args([
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        "--include-partial-messages",
        "--permission-prompt-tool",
        "stdio",
    ]);
    // One `--settings`: the project's rules, and the provider's variables,
    // which outrank an env block in the user's own settings.
    let launch = spec.provider.with_model(&spec.model);
    let mut settings = serde_json::Map::new();
    if let Some(env) = launch.settings_env() {
        settings.insert("env".into(), env.into());
    }
    match spec.kind {
        Kind::Chat => {
            cmd.args(["--tools", ""]);
            // A plain chat has no tools, so none of the configured MCP
            // servers either: each would start for nothing, and a
            // browser one is one more client of the extension (#267).
            cmd.arg("--strict-mcp-config");
        }
        // Read fresh on every start, so edits in Claude settings apply to the next process.
        Kind::Code => {
            let perms = project::Permissions::load(&spec.settings_dir);
            let claude = project::ClaudeSettings::load(&spec.settings_dir).for_provider(spec.provider.provider());
            if let Some(Value::Object(rules)) =
                perms.as_cli_settings().and_then(|s| serde_json::from_str::<Value>(&s).ok())
            {
                settings.extend(rules);
            }
            cmd.args(["--permission-mode", spec.mode.cli()]);
            // Routine work to Kynex's subagents, unless the project or app turned it off.
            if spec.provider.provider() == Provider::Anthropic
                && crate::delegate::enabled(claude.delegate, crate::delegate::default_on())
            {
                let agents = crate::delegate::load();
                cmd.args(crate::delegate::cli_args(&agents, &claude.agents_off, &spec.model, &claude.delegate_rule));
            }
            cmd.args(claude.cli_args());
            started.launched = Some(project::Launched::new(&perms, &claude));
            // The browser picked in Settings, in place of a configured one of the same name.
            if let Some(b) = crate::browser::current().browser() {
                cmd.args(["--mcp-config", &crate::browser::mcp_config(b)]);
            }
        }
    }
    if !settings.is_empty() {
        cmd.args(["--settings", &Value::Object(settings).to_string()]);
    }
    if !spec.model.is_empty() {
        cmd.args(["--model", &spec.model]);
    }
    if let Some(id) = &spec.resume {
        cmd.args(["--resume", id]);
        // After a rewind: continue from there, branching off what came after.
        if let Some(at) = spec.resume_at.take() {
            cmd.args(["--resume-session-at", &at]);
        }
    }
    Ok((cmd, "`claude`"))
}
