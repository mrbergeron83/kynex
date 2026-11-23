//! Non-blocking subprocess manager for the native `claude` CLI over
//! bidirectional NDJSON `stream-json`.
//!
//! Spawns dedicated writer, bounded stdout reader (32 MiB/line), and stderr
//! collector (64 KiB cap) threads. Delivers parsed [`ClaudeEvent`]s through an
//! `mpsc` channel and fires an optional [`Wake`] callback for zero-polling UI
//! reactivity.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde_json::Value;

use super::protocol::{self, StreamParseState};
pub use super::types::Wake;
use super::types::*;

/// Maximum single NDJSON line accepted from stdout (32 MiB — large enough for
/// base64 screenshots/images while preventing unbounded memory growth).
const MAX_STDOUT_LINE_BYTES: usize = 32 * 1024 * 1024;
/// Maximum stderr bytes retained for exit diagnostics.
const MAX_STDERR_BYTES: usize = 64 * 1024;
/// Maximum time [`ClaudeClient::close`] waits for a graceful stdin-EOF exit
/// before terminating the subprocess.
const GRACEFUL_CLOSE_WAIT: Duration = Duration::from_millis(400);

/// Live handle to one running `claude` CLI subprocess.
pub struct ClaudeClient {
    writer_tx: Mutex<Option<mpsc::Sender<String>>>,
    events_rx: Mutex<mpsc::Receiver<ClaudeEvent>>,
    session_id: Arc<Mutex<Option<String>>>,
    pending_inputs: Arc<Mutex<HashMap<String, Value>>>,
    pending_mcp_requests: Arc<Mutex<Vec<String>>>,
    next_req_id: AtomicU64,
    closed: AtomicBool,
    child: Arc<Mutex<Child>>,
    writer_thread: Mutex<Option<JoinHandle<()>>>,
    reader_thread: Mutex<Option<JoinHandle<()>>>,
}

impl ClaudeClient {
    /// Spawns the user's `claude` CLI configured per `config` and immediately
    /// queues the `initialize` handshake frame.
    pub fn spawn(config: SpawnConfig) -> Result<Self, String> {
        let cmd = build_command(&config);
        Self::spawn_command(cmd, config.on_wake)
    }

    /// Spawns a pre-configured `Command` speaking the `claude` `stream-json`
    /// protocol. Used by [`Self::spawn`] and cross-platform mock unit tests.
    pub(crate) fn spawn_command(mut cmd: Command, on_wake: Option<Wake>) -> Result<Self, String> {
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        crate::process::dies_with_parent(&mut cmd);

        let mut child = cmd.spawn().map_err(|e| format!("Failed to start `claude`: {e}"))?;
        let stdin = child.stdin.take().ok_or("Failed to open `claude` stdin")?;
        let stdout = child.stdout.take().ok_or("Failed to open `claude` stdout")?;
        let stderr = child.stderr.take().ok_or("Failed to open `claude` stderr")?;

        let child = Arc::new(Mutex::new(child));
        let session_id = Arc::new(Mutex::new(None));
        let pending_inputs: Arc<Mutex<HashMap<String, Value>>> = Arc::new(Mutex::new(HashMap::new()));
        let pending_mcp_requests: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

        let (writer_tx, writer_rx) = mpsc::channel::<String>();
        let (events_tx, events_rx) = mpsc::channel::<ClaudeEvent>();

        let stderr_buf = Arc::new(Mutex::new(String::new()));
        let stderr_clone = Arc::clone(&stderr_buf);
        let stderr_thread = thread::Builder::new()
            .name("claude-stderr".into())
            .spawn(move || collect_stderr(stderr, stderr_clone))
            .map_err(|e| format!("Failed to spawn stderr thread: {e}"))?;

        let writer_thread = thread::Builder::new()
            .name("claude-writer".into())
            .spawn(move || run_writer(stdin, writer_rx))
            .map_err(|e| format!("Failed to spawn writer thread: {e}"))?;

        let reader_child = Arc::clone(&child);
        let reader_sid = Arc::clone(&session_id);
        let reader_inputs = Arc::clone(&pending_inputs);
        let reader_mcp = Arc::clone(&pending_mcp_requests);
        let reader_thread = thread::Builder::new()
            .name("claude-reader".into())
            .spawn(move || {
                run_reader(
                    stdout,
                    reader_child,
                    stderr_thread,
                    stderr_buf,
                    reader_sid,
                    reader_inputs,
                    reader_mcp,
                    events_tx,
                    on_wake,
                );
            })
            .map_err(|e| format!("Failed to spawn reader thread: {e}"))?;

        let client = Self {
            writer_tx: Mutex::new(Some(writer_tx)),
            events_rx: Mutex::new(events_rx),
            session_id,
            pending_inputs,
            pending_mcp_requests,
            next_req_id: AtomicU64::new(1),
            closed: AtomicBool::new(false),
            child,
            writer_thread: Mutex::new(Some(writer_thread)),
            reader_thread: Mutex::new(Some(reader_thread)),
        };

        // Immediately send the initialize handshake (zero API call).
        let _ = client.send_raw(protocol::build_initialize("init-1"));
        Ok(client)
    }

    /// Drains all currently queued events without blocking.
    pub fn drain_events(&self) -> Vec<ClaudeEvent> {
        let mut out = Vec::new();
        if let Ok(rx) = self.events_rx.lock() {
            while let Ok(ev) = rx.try_recv() {
                out.push(ev);
            }
        }
        out
    }

    /// The active Claude session ID once learned from `system/init` or `result`.
    pub fn session_id(&self) -> Option<String> {
        self.session_id.lock().ok().and_then(|s| s.clone())
    }

    /// Sends a user prompt turn (text, base64 images, and/or file context blocks).
    pub fn send_prompt(&self, blocks: Vec<PromptBlock>) -> Result<(), String> {
        self.send_raw(protocol::build_user_message(&blocks))
    }

    /// Sends a mid-turn steering message while Claude is actively working.
    pub fn steer(&self, blocks: Vec<PromptBlock>) -> Result<(), String> {
        self.send_raw(protocol::build_user_message(&blocks))
    }

    /// Answers an incoming `can_use_tool` / `AskUserQuestion` / `ExitPlanMode`
    /// permission card. For [`PermissionAnswer::ApprovePlan`], also sends
    /// `set_permission_mode(next_mode)` immediately after allowing the tool.
    pub fn reply_permission(&self, request_id: &str, answer: PermissionAnswer) -> Result<(), String> {
        let orig_input = self.pending_inputs.lock().ok().and_then(|mut m| m.remove(request_id));
        let frame = protocol::build_permission_response(request_id, &answer, orig_input.as_ref());
        self.send_raw(frame)?;
        if let PermissionAnswer::ApprovePlan { next_mode } = answer {
            let _ = self.set_permission_mode(next_mode)?;
        }
        Ok(())
    }

    pub fn set_permission_mode(&self, mode: PermissionMode) -> Result<String, String> {
        let id = self.next_request_id("mode");
        self.send_raw(protocol::build_set_permission_mode(&id, mode))?;
        Ok(id)
    }

    pub fn set_model(&self, model: &str) -> Result<String, String> {
        let id = self.next_request_id("model");
        self.send_raw(protocol::build_set_model(&id, model))?;
        Ok(id)
    }

    pub fn set_effort(&self, effort: EffortLevel) -> Result<String, String> {
        let id = self.next_request_id("effort");
        self.send_raw(protocol::build_set_effort(&id, effort))?;
        Ok(id)
    }

    /// Alt+T: thinking on or off for the rest of the session.
    pub fn set_thinking(&self, on: bool) -> Result<String, String> {
        let id = self.next_request_id("think");
        self.send_raw(protocol::build_set_thinking(&id, on))?;
        Ok(id)
    }

    pub fn set_fast_mode(&self, enabled: bool) -> Result<String, String> {
        let id = self.next_request_id("fast");
        self.send_raw(protocol::build_set_fast_mode(&id, enabled))?;
        Ok(id)
    }

    pub fn apply_flag_settings(&self, settings: Value) -> Result<String, String> {
        let id = self.next_request_id("flag");
        self.send_raw(protocol::build_apply_flag_settings(&id, settings))?;
        Ok(id)
    }

    pub fn interrupt(&self) -> Result<String, String> {
        let id = self.next_request_id("intr");
        self.send_raw(protocol::build_interrupt(&id))?;
        Ok(id)
    }

    pub fn stop_task(&self, task_id: &str) -> Result<String, String> {
        let id = self.next_request_id("stop");
        self.send_raw(protocol::build_stop_task(&id, task_id))?;
        Ok(id)
    }

    pub fn rewind_files(&self, user_message_id: &str) -> Result<String, String> {
        let id = self.next_request_id("rewind");
        self.send_raw(protocol::build_rewind_files(&id, user_message_id))?;
        Ok(id)
    }

    pub fn mcp_status(&self) -> Result<String, String> {
        let id = self.next_request_id("mcp");
        if let Ok(mut pending) = self.pending_mcp_requests.lock() {
            pending.push(id.clone());
        }
        self.send_raw(protocol::build_mcp_status(&id))?;
        Ok(id)
    }

    pub fn mcp_reconnect(&self, server_name: &str) -> Result<String, String> {
        let id = self.next_request_id("mcp-reconn");
        self.send_raw(protocol::build_mcp_reconnect(&id, server_name))?;
        Ok(id)
    }

    pub fn mcp_toggle(&self, server_name: &str, enabled: bool) -> Result<String, String> {
        let id = self.next_request_id("mcp-toggle");
        self.send_raw(protocol::build_mcp_toggle(&id, server_name, enabled))?;
        Ok(id)
    }

    /// Gracefully closes stdin, waits briefly for exit, then terminates the
    /// child process if it is still running. Idempotent.
    pub fn close(&self) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        // Dropping the sender closes stdin on the writer thread.
        if let Ok(mut tx) = self.writer_tx.lock() {
            tx.take();
        }
        if let Ok(mut h) = self.writer_thread.lock()
            && let Some(handle) = h.take()
        {
            let _ = handle.join();
        }

        let deadline = Instant::now() + GRACEFUL_CLOSE_WAIT;
        loop {
            let exited = self.child.lock().is_ok_and(|mut c| !matches!(c.try_wait(), Ok(None)));
            if exited {
                break;
            }
            if Instant::now() >= deadline {
                if let Ok(mut c) = self.child.lock() {
                    let _ = c.kill();
                    let _ = c.wait();
                }
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }

        if let Ok(mut h) = self.reader_thread.lock()
            && let Some(handle) = h.take()
        {
            let _ = handle.join();
        }
    }

    fn next_request_id(&self, prefix: &str) -> String {
        let n = self.next_req_id.fetch_add(1, Ordering::Relaxed);
        format!("{prefix}-{n}")
    }

    fn send_raw(&self, frame: String) -> Result<(), String> {
        let guard = self.writer_tx.lock().map_err(|_| "Claude writer lock poisoned".to_string())?;
        let tx = guard.as_ref().ok_or_else(|| "Claude session is closed".to_string())?;
        tx.send(frame).map_err(|_| "Claude process stdin is closed".to_string())
    }
}

impl Drop for ClaudeClient {
    fn drop(&mut self) {
        self.close();
    }
}

/// Builds the OS-agnostic `Command` for launching `claude` in bidirectional
/// `stream-json` mode.
pub(crate) fn build_command(config: &SpawnConfig) -> Command {
    let mut cmd = Command::new(crate::cli::program());
    cmd.args([
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        "--permission-prompt-tool",
        "stdio",
        "--include-partial-messages",
        "--replay-user-messages",
        "--forward-subagent-text",
        "--prompt-suggestions",
        "true",
        "--allow-dangerously-skip-permissions",
    ]);

    match &config.session {
        SessionStart::New => {}
        SessionStart::Resume(id) if !id.trim().is_empty() => {
            cmd.args(["--resume", id.trim()]);
        }
        SessionStart::ResumeAt { id, at } if !id.trim().is_empty() && !at.trim().is_empty() => {
            cmd.args(["--resume", id.trim(), "--resume-session-at", at.trim()]);
        }
        SessionStart::Fork(id) if !id.trim().is_empty() => {
            cmd.args(["--resume", id.trim(), "--fork-session"]);
        }
        _ => {}
    }

    if let Some(model) = config.model.as_deref().map(str::trim).filter(|m| !m.is_empty()) {
        cmd.args(["--model", model]);
    }
    if let Some(effort) = config.effort {
        cmd.args(["--effort", effort.as_str()]);
    }
    if let Some(mode) = config.permission_mode {
        cmd.args(["--permission-mode", mode.as_str()]);
    }
    if let Some(settings) = merged_settings_json(&config.launch, config.settings_json.as_deref()) {
        cmd.args(["--settings", &settings]);
    }
    cmd.args(["--append-system-prompt", crate::control::agent::NOTE]);

    if !config.cwd.as_os_str().is_empty() {
        cmd.current_dir(&config.cwd);
    }

    let fresh = crate::fresh_env::vars();
    for (k, v) in &fresh {
        cmd.env(k, v);
    }
    if !config.extra_env.iter().any(|(k, _)| k.eq_ignore_ascii_case("PATH"))
        && let Some(shim_dir) = crate::control::agent::ensure_shims()
    {
        let base_path = fresh
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("PATH"))
            .map(|(_, v)| v.clone())
            .or_else(|| std::env::var_os("PATH"));
        cmd.env("PATH", crate::control::agent::path_with(shim_dir, base_path.as_deref()));
    }

    config.launch.apply_over(&mut cmd, &fresh);
    cmd.env("CLAUDE_CODE_ENABLE_SDK_FILE_CHECKPOINTING", "1");
    for (k, v) in &config.extra_env {
        cmd.env(k, v);
    }

    cmd
}

/// Combines `Launch::settings_env()` (e.g. OpenRouter overrides) with an
/// optional caller-supplied `--settings` JSON string.
fn merged_settings_json(launch: &crate::provider::Launch, extra: Option<&str>) -> Option<String> {
    let env_map = launch.settings_env();
    let extra = extra.map(str::trim).filter(|s| !s.is_empty());
    if env_map.is_none() && extra.is_none() {
        return None;
    }

    let mut root = extra
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();

    if let Some(vars) = env_map {
        let mut merged_env = root.remove("env").and_then(|v| v.as_object().cloned()).unwrap_or_default();
        for (k, v) in vars {
            merged_env.insert(k, v);
        }
        root.insert("env".into(), Value::Object(merged_env));
    } else if root.is_empty() {
        // Caller passed a raw settings string (e.g. a file path); pass it through unchanged.
        return extra.map(str::to_string);
    }

    Some(Value::Object(root).to_string())
}

fn run_writer(mut stdin: std::process::ChildStdin, rx: mpsc::Receiver<String>) {
    while let Ok(frame) = rx.recv() {
        if stdin.write_all(frame.as_bytes()).is_err() || stdin.write_all(b"\n").is_err() || stdin.flush().is_err() {
            break;
        }
    }
}

fn collect_stderr(mut stderr: std::process::ChildStderr, out: Arc<Mutex<String>>) {
    let mut buf = [0u8; 4096];
    let mut collected = Vec::new();
    while let Ok(n) = stderr.read(&mut buf) {
        if n == 0 {
            break;
        }
        let room = MAX_STDERR_BYTES.saturating_sub(collected.len());
        if room > 0 {
            collected.extend_from_slice(&buf[..n.min(room)]);
        }
    }
    if let Ok(mut guard) = out.lock() {
        *guard = String::from_utf8_lossy(&collected).into_owned();
    }
}

#[allow(clippy::too_many_arguments)]
fn run_reader(
    stdout: std::process::ChildStdout,
    child: Arc<Mutex<Child>>,
    stderr_thread: JoinHandle<()>,
    stderr_buf: Arc<Mutex<String>>,
    session_id: Arc<Mutex<Option<String>>>,
    pending_inputs: Arc<Mutex<HashMap<String, Value>>>,
    pending_mcp_requests: Arc<Mutex<Vec<String>>>,
    events_tx: mpsc::Sender<ClaudeEvent>,
    on_wake: Option<Wake>,
) {
    let mut reader = BufReader::new(stdout);
    let mut state = StreamParseState::default();
    let mut raw_line = Vec::new();

    while let Some(overflowed) = read_line_bounded(&mut reader, &mut raw_line, MAX_STDOUT_LINE_BYTES) {
        if overflowed {
            let _ = events_tx.send(ClaudeEvent::Error(format!(
                "Skipped an oversized stdout line (> {} MiB)",
                MAX_STDOUT_LINE_BYTES / (1024 * 1024)
            )));
            if let Some(wake) = &on_wake {
                wake();
            }
            continue;
        }

        if let Ok(mut pending) = pending_mcp_requests.lock() {
            for id in pending.drain(..) {
                state.register_mcp_status_request(id);
            }
        }

        let line = String::from_utf8_lossy(&raw_line);
        let events = protocol::parse_line(&line, &mut state);
        if events.is_empty() {
            continue;
        }

        for ev in events {
            match &ev {
                ClaudeEvent::SystemInit(info) if !info.session_id.is_empty() => {
                    if let Ok(mut sid) = session_id.lock() {
                        *sid = Some(info.session_id.clone());
                    }
                }
                ClaudeEvent::TurnFinished(tr) => {
                    if let Some(sid) = tr.session_id.as_deref().filter(|s| !s.is_empty())
                        && let Ok(mut guard) = session_id.lock()
                    {
                        *guard = Some(sid.to_string());
                    }
                }
                ClaudeEvent::PermissionRequested(req) => {
                    let orig = match &req.kind {
                        PermissionKind::ToolApproval { input, .. } => input.clone(),
                        PermissionKind::AskUserQuestion { raw_input, .. } => raw_input.clone(),
                        PermissionKind::ExitPlanMode { raw_input, .. } => raw_input.clone(),
                    };
                    if let Ok(mut map) = pending_inputs.lock() {
                        map.insert(req.request_id.clone(), orig);
                    }
                }
                _ => {}
            }
            if events_tx.send(ev).is_err() {
                break;
            }
        }

        if let Some(wake) = &on_wake {
            wake();
        }
    }

    let _ = stderr_thread.join();
    let stderr = stderr_buf.lock().ok().map(|s| s.clone()).unwrap_or_default();
    let code = child.lock().ok().and_then(|mut c| c.wait().ok()).and_then(|s| s.code());

    let _ = events_tx.send(ClaudeEvent::Exited { code, stderr });
    if let Some(wake) = &on_wake {
        wake();
    }
}

/// Reads a single `\n`-delimited line into `buf`, discarding any excess bytes
/// past `max_bytes` so a single huge line cannot OOM the process.
///
/// Returns `None` on EOF with zero bytes read, `Some(false)` for a normal line,
/// or `Some(true)` if the line exceeded `max_bytes` and was skipped.
fn read_line_bounded<R: BufRead>(reader: &mut R, buf: &mut Vec<u8>, max_bytes: usize) -> Option<bool> {
    buf.clear();
    let mut overflowed = false;
    let mut saw_any = false;

    loop {
        let available = match reader.fill_buf() {
            Ok([]) => break,
            Ok(chunk) => chunk,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        saw_any = true;

        let (take, found_nl) = match available.iter().position(|&b| b == b'\n') {
            Some(pos) => (pos + 1, true),
            None => (available.len(), false),
        };

        if !overflowed {
            if buf.len() + take <= max_bytes {
                buf.extend_from_slice(&available[..take]);
            } else {
                overflowed = true;
                buf.clear();
            }
        }

        reader.consume(take);
        if found_nl {
            break;
        }
    }

    saw_any.then_some(overflowed)
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    use std::sync::atomic::AtomicUsize;

    use serde_json::{Map, json};

    use super::*;

    #[test]
    fn bounded_line_reader_handles_normal_and_oversized_lines() {
        let data = b"short\n0123456789abcdef\nafter\n";
        let mut cursor = Cursor::new(&data[..]);
        let mut buf = Vec::new();
        assert_eq!(read_line_bounded(&mut cursor, &mut buf, 10), Some(false));
        assert_eq!(buf, b"short\n");
        assert_eq!(read_line_bounded(&mut cursor, &mut buf, 10), Some(true));
        assert_eq!(read_line_bounded(&mut cursor, &mut buf, 10), Some(false));
        assert_eq!(buf, b"after\n");
        assert_eq!(read_line_bounded(&mut cursor, &mut buf, 10), None);
    }

    #[test]
    fn build_command_applies_flags_and_env() {
        let cfg = SpawnConfig {
            cwd: std::env::temp_dir(),
            session: SessionStart::Fork("sess-123".into()),
            launch: crate::provider::Launch::OpenRouter {
                key: "sk-or-test".into(),
                settings: crate::provider::OpenRouterSettings {
                    model: "qwen/qwen3-coder".into(),
                    max_output_tokens: 8000,
                },
            },
            model: Some("opus".into()),
            effort: Some(EffortLevel::High),
            permission_mode: Some(PermissionMode::Plan),
            settings_json: None,
            extra_env: vec![("KYNEX_TEST".into(), "1".into())],
            on_wake: None,
        };
        let cmd = build_command(&cfg);
        let args: Vec<String> = cmd.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        for flag in [
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--permission-prompt-tool",
            "stdio",
            "--include-partial-messages",
            "--replay-user-messages",
            "--forward-subagent-text",
            "--prompt-suggestions",
            "true",
            "--allow-dangerously-skip-permissions",
            "--resume",
            "sess-123",
            "--fork-session",
            "--model",
            "opus",
            "--effort",
            "high",
            "--permission-mode",
            "plan",
            "--settings",
            "--append-system-prompt",
        ] {
            assert!(args.iter().any(|a| a == flag), "missing flag {flag} in {args:?}");
        }
        let envs: Vec<(String, Option<String>)> = cmd
            .get_envs()
            .map(|(k, v)| (k.to_string_lossy().into_owned(), v.map(|s| s.to_string_lossy().into_owned())))
            .collect();
        assert!(
            envs.iter().any(|(k, v)| k == "CLAUDE_CODE_ENABLE_SDK_FILE_CHECKPOINTING" && v.as_deref() == Some("1"))
        );
        assert!(envs.iter().any(|(k, v)| k == "ANTHROPIC_AUTH_TOKEN" && v.as_deref() == Some("sk-or-test")));
        assert!(envs.iter().any(|(k, v)| k == "KYNEX_TEST" && v.as_deref() == Some("1")));
    }

    fn mock_claude_command() -> Command {
        #[cfg(windows)]
        {
            let script = r#"
$ErrorActionPreference = 'Stop'
while ($null -ne ($line = [Console]::In.ReadLine())) {
    if ($line -match '"subtype":"initialize"') {
        [Console]::Out.WriteLine('{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"models":[{"value":"opus","displayName":"Opus","supportsEffort":true,"supportedEffortLevels":["low","medium","high"]}],"commands":[{"name":"compact","description":"Compact","builtin":true}],"current_permission_mode":"default","claude_code_version":"2.1.330"}}}')
    } elseif ($line -match '"type":"user"') {
        [Console]::Out.WriteLine('{"type":"system","subtype":"init","session_id":"mock-sess-1","model":"opus","permissionMode":"default"}')
        [Console]::Out.WriteLine('{"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}}')
        [Console]::Out.WriteLine('{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Mock reply"}}}')
        [Console]::Out.WriteLine('{"type":"control_request","request_id":"perm-1","request":{"subtype":"can_use_tool","tool_name":"AskUserQuestion","input":{"questions":[{"question":"Proceed?","header":"Confirm","multiSelect":false,"options":[{"label":"Yes","description":"Go"}]}]}}}')
    } elseif ($line -match '"request_id":"perm-1"' -and $line -match '"Yes"') {
        [Console]::Out.WriteLine('{"type":"result","subtype":"success","session_id":"mock-sess-1","total_cost_usd":0.05,"duration_ms":42,"num_turns":1,"is_error":false,"usage":{"input_tokens":10,"output_tokens":5},"modelUsage":{"opus":{"contextWindow":200000,"costUSD":0.05}}}')
    } elseif ($line -match '"subtype":"interrupt"') {
        [Console]::Out.WriteLine('{"type":"control_response","response":{"subtype":"success","request_id":"intr-1","response":{}}}')
    }
    [Console]::Out.Flush()
}
"#;
            let mut cmd = Command::new("powershell");
            cmd.args(["-NoProfile", "-NonInteractive", "-Command", script]);
            cmd
        }
        #[cfg(not(windows))]
        {
            let script = r#"
while IFS= read -r line; do
  case "$line" in
    *'"subtype":"initialize"'*)
      printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"models":[{"value":"opus","displayName":"Opus","supportsEffort":true,"supportedEffortLevels":["low","medium","high"]}],"commands":[{"name":"compact","description":"Compact","builtin":true}],"current_permission_mode":"default","claude_code_version":"2.1.330"}}}'
      ;;
    *'"type":"user"'*)
      printf '%s\n' '{"type":"system","subtype":"init","session_id":"mock-sess-1","model":"opus","permissionMode":"default"}'
      printf '%s\n' '{"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}}'
      printf '%s\n' '{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Mock reply"}}}'
      printf '%s\n' '{"type":"control_request","request_id":"perm-1","request":{"subtype":"can_use_tool","tool_name":"AskUserQuestion","input":{"questions":[{"question":"Proceed?","header":"Confirm","multiSelect":false,"options":[{"label":"Yes","description":"Go"}]}]}}}'
      ;;
    *'"request_id":"perm-1"'*'"Yes"'*)
      printf '%s\n' '{"type":"result","subtype":"success","session_id":"mock-sess-1","total_cost_usd":0.05,"duration_ms":42,"num_turns":1,"is_error":false,"usage":{"input_tokens":10,"output_tokens":5},"modelUsage":{"opus":{"contextWindow":200000,"costUSD":0.05}}}'
      ;;
    *'"subtype":"interrupt"'*)
      printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"intr-1","response":{}}}'
      ;;
  esac
done
"#;
            let mut cmd = Command::new("sh");
            cmd.args(["-c", script]);
            cmd
        }
    }

    fn wait_for_event(
        client: &ClaudeClient,
        timeout: Duration,
        mut pred: impl FnMut(&ClaudeEvent) -> bool,
    ) -> Vec<ClaudeEvent> {
        let deadline = Instant::now() + timeout;
        let mut seen = Vec::new();
        while Instant::now() < deadline {
            let batch = client.drain_events();
            for ev in batch {
                let matched = pred(&ev);
                seen.push(ev);
                if matched {
                    return seen;
                }
            }
            thread::sleep(Duration::from_millis(15));
        }
        seen
    }

    #[test]
    fn mock_process_full_round_trip() {
        let wakes = Arc::new(AtomicUsize::new(0));
        let wakes_c = Arc::clone(&wakes);
        let wake: Wake = Arc::new(move || {
            wakes_c.fetch_add(1, Ordering::Relaxed);
        });
        let client = ClaudeClient::spawn_command(mock_claude_command(), Some(wake)).unwrap();

        let init_evs = wait_for_event(&client, Duration::from_secs(10), |e| matches!(e, ClaudeEvent::Initialized(_)));
        assert!(
            init_evs.iter().any(
                |e| matches!(e, ClaudeEvent::Initialized(i) if i.claude_code_version.as_deref() == Some("2.1.330"))
            )
        );
        assert!(wakes.load(Ordering::Relaxed) >= 1);

        client.send_prompt(vec![PromptBlock::Text("hello".into())]).unwrap();
        let turn_evs =
            wait_for_event(&client, Duration::from_secs(10), |e| matches!(e, ClaudeEvent::PermissionRequested(_)));
        assert!(turn_evs.iter().any(|e| matches!(e, ClaudeEvent::TextDelta(t) if t == "Mock reply")));
        assert_eq!(client.session_id().as_deref(), Some("mock-sess-1"));

        let mut answers = Map::new();
        answers.insert("Proceed?".into(), json!("Yes"));
        client.reply_permission("perm-1", PermissionAnswer::Questions(answers)).unwrap();

        let fin_evs = wait_for_event(&client, Duration::from_secs(10), |e| matches!(e, ClaudeEvent::TurnFinished(_)));
        assert!(
            fin_evs.iter().any(|e| matches!(e, ClaudeEvent::TurnFinished(tr) if tr.context_window == Some(200_000)))
        );

        client.close();
        let exit_evs = client.drain_events();
        assert!(exit_evs.iter().any(|e| matches!(e, ClaudeEvent::Exited { .. })));
    }

    #[test]
    #[ignore = "spawns the real installed claude CLI (zero LLM/API call)"]
    fn live_initialize() {
        let client = ClaudeClient::spawn(SpawnConfig { cwd: std::env::current_dir().unwrap(), ..Default::default() })
            .expect("spawn claude");
        let evs = wait_for_event(&client, Duration::from_secs(15), |e| matches!(e, ClaudeEvent::Initialized(_)));
        client.close();
        let init = evs.iter().find_map(|e| match e {
            ClaudeEvent::Initialized(info) => Some(info),
            _ => None,
        });
        let info = init.expect("expected Initialized event from live claude CLI");
        assert!(!info.models.is_empty(), "expected non-empty models list");
        assert!(info.claude_code_version.is_some(), "expected claude_code_version");
    }
}
