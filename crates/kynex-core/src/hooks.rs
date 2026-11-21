//! Native CLI lifecycle hooks for reporting agent state and session IDs back to Kynex.
//!
//! Supported CLIs (`claude`, `agy`) invoke `knx hook <provider> <event>` with a JSON payload
//! on stdin. Kynex parses the payload with [`parse_payload`], sends a lightweight
//! [`crate::control::Request::Hook`] over the local socket, and exits 0 immediately so a hook
//! never blocks or breaks the CLI.

use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

/// The lifecycle state a CLI hook reports for its `agent_cli` pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HookState {
    Working,
    NeedsYou,
    Done,
    Idle,
}

/// What a single CLI hook invocation tells Kynex.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HookReport {
    pub state: Option<HookState>,
    pub session_id: Option<String>,
    pub detail: Option<String>,
    /// True when the CLI explicitly indicates this hook came from a child/subagent session.
    pub subagent: bool,
    /// True when a `Stop` hook ended a turn while background tasks (such as subagents) are still
    /// running (`fullyIdle: false`).
    pub background: bool,
}

/// Parses the JSON payload a CLI wrote to stdin when running `knx hook <provider> [event]`.
/// Never panics, even on malformed or truncated input.
pub fn parse_payload(provider: &str, event_hint: Option<&str>, stdin: &str) -> HookReport {
    let v: Value = serde_json::from_str(stdin.trim()).unwrap_or(Value::Null);
    match provider.trim().to_ascii_lowercase().as_str() {
        "claude" | "claude-openrouter" => parse_claude(event_hint, &v),
        "agy" | "agy-openrouter" => parse_agy(event_hint, &v),
        _ => parse_generic(event_hint, &v),
    }
}

fn parse_claude(event_hint: Option<&str>, v: &Value) -> HookReport {
    let event = v["hook_event_name"].as_str().or(event_hint).unwrap_or("");
    let session_id = clean_id(v["session_id"].as_str());
    let tool = clean_text(v["tool_name"].as_str());
    let (state, detail) = match event {
        "SessionStart" => (Some(HookState::Idle), None),
        "UserPromptSubmit" => (Some(HookState::Working), Some("Thinking".into())),
        "PreToolUse" => (Some(HookState::Working), tool.map(|t| format!("Using {}", short_tool(&t)))),
        "PostToolUse" | "PostToolUseFailure" => (Some(HookState::Working), None),
        "PermissionRequest" => (
            Some(HookState::NeedsYou),
            Some(tool.map_or_else(|| "Needs permission".into(), |t| format!("Asks to use {}", short_tool(&t)))),
        ),
        "Notification" => {
            let kind = v["notification_type"].as_str().unwrap_or("");
            let msg = clean_text(v["message"].as_str());
            match kind {
                "permission_prompt" => (Some(HookState::NeedsYou), msg.or_else(|| Some("Needs permission".into()))),
                "elicitation_dialog" => (Some(HookState::NeedsYou), msg.or_else(|| Some("Asks a question".into()))),
                "idle_prompt" => (Some(HookState::Done), msg),
                _ => (None, msg),
            }
        }
        "Stop" => (Some(HookState::Done), Some("Finished".into())),
        "SessionEnd" => (Some(HookState::Idle), None),
        _ => (None, None),
    };
    HookReport { state, session_id, detail, subagent: false, background: false }
}

fn parse_agy(event_hint: Option<&str>, v: &Value) -> HookReport {
    let event = v["hookEventName"].as_str().or(event_hint).unwrap_or("");
    let session_id = clean_id(v["conversationId"].as_str());
    let tool = clean_text(v["toolCall"]["name"].as_str());
    let subagent = is_subagent_payload(v);
    let mut background = false;
    let (state, detail) = match event {
        "SessionStart" => (Some(HookState::Idle), None),
        "PreInvocation" => (Some(HookState::Working), Some("Thinking".into())),
        "PostToolUse" => (Some(HookState::Working), tool.map(|t| format!("Used {}", short_tool(&t)))),
        "PostInvocation" => (Some(HookState::Working), None),
        "Stop" => {
            let reason = v["terminationReason"].as_str().unwrap_or("");
            if v["fullyIdle"].as_bool() == Some(false) && !matches!(reason, "ERROR" | "USER_CANCELED") {
                background = true;
                (Some(HookState::Working), Some("Working in background".into()))
            } else {
                let detail = match reason {
                    "ERROR" => "Stopped on an error",
                    "USER_CANCELED" => "Stopped",
                    _ => "Finished",
                };
                (Some(HookState::Done), Some(detail.into()))
            }
        }
        _ => (None, None),
    };
    HookReport { state, session_id, detail, subagent, background }
}

fn parse_generic(event_hint: Option<&str>, v: &Value) -> HookReport {
    let event = v["hook_event_name"].as_str().or_else(|| v["hookEventName"].as_str()).or(event_hint).unwrap_or("");
    let session_id = clean_id(v["session_id"].as_str().or_else(|| v["conversationId"].as_str()));
    let state = match event.to_ascii_lowercase().as_str() {
        "working" | "userpromptsubmit" | "preinvocation" | "pretooluse" => Some(HookState::Working),
        "needs_you" | "waiting" | "blocked" | "permissionrequest" => Some(HookState::NeedsYou),
        "done" | "stop" | "finished" => Some(HookState::Done),
        "idle" | "sessionstart" | "sessionend" => Some(HookState::Idle),
        _ => None,
    };
    let detail = clean_text(v["detail"].as_str().or_else(|| v["message"].as_str()));
    HookReport { state, session_id, detail, subagent: is_subagent_payload(v), background: false }
}

fn is_subagent_payload(v: &Value) -> bool {
    v["isSubagent"].as_bool() == Some(true)
        || v["is_subagent"].as_bool() == Some(true)
        || v["nestingDepth"].as_u64().is_some_and(|d| d > 0)
        || v["nesting_depth"].as_u64().is_some_and(|d| d > 0)
        || v["parentConversationId"].as_str().is_some_and(|s| !s.trim().is_empty())
        || v["parent_conversation_id"].as_str().is_some_and(|s| !s.trim().is_empty())
}

fn clean_id(s: Option<&str>) -> Option<String> {
    let id = s?.trim();
    (!id.is_empty() && !id.contains(['/', '\\', '\0', '\n', '\r'])).then(|| id.to_string())
}

fn clean_text(s: Option<&str>) -> Option<String> {
    let line = s?.lines().next()?.trim();
    if line.is_empty() {
        return None;
    }
    const MAX: usize = 60;
    if line.chars().count() > MAX {
        Some(format!("{}…", line.chars().take(MAX).collect::<String>()))
    } else {
        Some(line.to_string())
    }
}

/// Strips `mcp__<server>__` prefix from MCP tool names so status text stays concise.
fn short_tool(tool: &str) -> &str {
    match tool.strip_prefix("mcp__") {
        Some(rest) => rest.split_once("__").map_or(rest, |(_, t)| t),
        None => tool,
    }
}

/// The stdout JSON a CLI expects from a hook command (`agy` requires `{}` on stdout).
pub fn stdout_response(provider: &str) -> Option<&'static str> {
    match provider.trim().to_ascii_lowercase().as_str() {
        "agy" | "agy-openrouter" => Some("{}"),
        _ => None,
    }
}

/// Builds the session-only `--settings` JSON object for Claude Code: lifecycle hooks plus
/// optional non-secret `env` overrides (e.g. OpenRouter URL/model/cap, never API keys).
pub fn claude_settings_json(provider_id: &str, env_overrides: Option<Map<String, Value>>) -> String {
    let hook = |event: &str, with_matcher: bool| -> Value {
        let cmd = format!("knx hook {provider_id} {event}");
        let entry = json!({ "type": "command", "command": cmd });
        if with_matcher { json!([{ "matcher": "*", "hooks": [entry] }]) } else { json!([{ "hooks": [entry] }]) }
    };
    let mut root = Map::new();
    root.insert(
        "hooks".into(),
        json!({
            "SessionStart": hook("SessionStart", false),
            "UserPromptSubmit": hook("UserPromptSubmit", false),
            "PreToolUse": hook("PreToolUse", true),
            "PermissionRequest": hook("PermissionRequest", true),
            "Notification": hook("Notification", false),
            "Stop": hook("Stop", false),
            "SessionEnd": hook("SessionEnd", false),
        }),
    );
    if let Some(env) = env_overrides.filter(|m| !m.is_empty()) {
        root.insert("env".into(), Value::Object(env));
    }
    Value::Object(root).to_string()
}

/// Writes `claude_settings_json` to a content-addressed file under `<data_dir>/hooks/` so
/// Claude Code can load it via `--settings <path>` without hitting Windows command-line
/// quoting limits. Blocks on file write only the first time a given config is needed.
pub fn write_claude_settings(provider_id: &str, env_overrides: Option<Map<String, Value>>) -> Option<PathBuf> {
    let text = claude_settings_json(provider_id, env_overrides);
    write_cached("claude", &text)
}

/// The shell command for one Antigravity (`agy`) hook event: runs `"%KYNEX_HOOK_BIN%" hook agy <event>`
/// only when `KYNEX_HOOK_BIN` is set (inside a hook-capable Kynex pane), and immediately prints `{}`
/// otherwise so standalone `agy`, Antigravity 2.0/IDE, and older Kynex versions pay zero overhead
/// and never spawn an extra Kynex window.
fn agy_hook_command(event: &str, windows: bool) -> String {
    if windows {
        format!("if defined KYNEX_HOOK_BIN (\"%KYNEX_HOOK_BIN%\" hook agy {event} 2>nul || echo {{}}) else (echo {{}})")
    } else {
        format!(
            "if [ -n \"$KYNEX_HOOK_BIN\" ]; then \"$KYNEX_HOOK_BIN\" hook agy {event} 2>/dev/null || printf '{{}}\\n'; else printf '{{}}\\n'; fi"
        )
    }
}

/// The `"kynex-status"` entry installed into `~/.gemini/config/hooks.json` for Antigravity.
/// Deliberately omits `PreToolUse` so `agy`'s native permission decisions are never intercepted.
fn agy_hooks_entry(windows: bool) -> Value {
    let flat = |event: &str| {
        json!([{
            "type": "command",
            "command": agy_hook_command(event, windows),
            "timeout": 5
        }])
    };
    let grouped = |event: &str| {
        json!([{
            "matcher": "*",
            "hooks": [{
                "type": "command",
                "command": agy_hook_command(event, windows),
                "timeout": 5
            }]
        }])
    };
    json!({
        "SessionStart": flat("SessionStart"),
        "PreInvocation": flat("PreInvocation"),
        "PostToolUse": grouped("PostToolUse"),
        "Stop": flat("Stop"),
    })
}

/// Builds the full `hooks.json` object for Antigravity (`agy`).
pub fn agy_hooks_json() -> String {
    json!({ "kynex-status": agy_hooks_entry(cfg!(windows)) }).to_string()
}

/// Global Antigravity hooks file (`~/.gemini/config/hooks.json`).
fn agy_hooks_path() -> Option<PathBuf> {
    Some(dirs::home_dir()?.join(".gemini").join("config").join("hooks.json"))
}

/// Ensures `"kynex-status"` is merged into `~/.gemini/config/hooks.json` without disturbing any
/// user-defined hooks (and without overwriting the file if it already matches or has invalid JSON).
/// Blocks on file I/O; call off the UI thread.
pub fn ensure_agy_hooks() {
    let Some(path) = agy_hooks_path() else { return };
    if let Err(e) = merge_agy_hooks_file(&path, agy_hooks_entry(cfg!(windows))) {
        eprintln!("kynex: agy hooks: {e}");
    }
}

fn merge_agy_hooks_file(path: &Path, entry: Value) -> Result<(), String> {
    let mut map = match std::fs::read_to_string(path) {
        Ok(raw) if raw.trim().is_empty() => Map::new(),
        Ok(raw) => match serde_json::from_str::<Value>(&raw) {
            Ok(Value::Object(m)) => m,
            Ok(_) => return Err(format!("{} is not a JSON object; leaving it untouched", path.display())),
            Err(e) => return Err(format!("couldn't parse {}: {e}; leaving it untouched", path.display())),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Map::new(),
        Err(e) => return Err(format!("couldn't read {}: {e}", path.display())),
    };
    if map.get("kynex-status") == Some(&entry) {
        return Ok(());
    }
    map.insert("kynex-status".into(), entry);
    let text = serde_json::to_string_pretty(&Value::Object(map)).map_err(|e| e.to_string())?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("couldn't create {}: {e}", parent.display()))?;
    }
    let part = path.with_extension(format!("json.{}", std::process::id()));
    std::fs::write(&part, &text).map_err(|e| format!("couldn't write {}: {e}", part.display()))?;
    if let Err(e) = std::fs::rename(&part, path) {
        let _ = std::fs::remove_file(&part);
        // Windows `rename` fails if the target already exists or is briefly locked.
        std::fs::write(path, &text).map_err(|w| format!("couldn't update {}: {e}; {w}", path.display()))?;
    }
    Ok(())
}

/// Writes `text` into `<data_dir>/hooks/<prefix>-<hash>.json` atomically.
fn write_cached(prefix: &str, text: &str) -> Option<PathBuf> {
    let hash: String = Sha256::digest(text.as_bytes()).iter().take(6).map(|b| format!("{b:02x}")).collect();
    let dir = crate::paths::data_dir().join("hooks");
    let file = dir.join(format!("{prefix}-{hash}.json"));
    if std::fs::read_to_string(&file).is_ok_and(|t| t == text) {
        return Some(file);
    }
    std::fs::create_dir_all(&dir).ok()?;
    let part = dir.join(format!("{prefix}-{hash}.json.{}", std::process::id()));
    std::fs::write(&part, text).ok()?;
    if std::fs::rename(&part, &file).is_err() {
        let _ = std::fs::remove_file(&part);
        return std::fs::read_to_string(&file).is_ok_and(|t| t == text).then_some(file);
    }
    Some(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_claude_lifecycle_payloads() {
        let start = parse_payload("claude", None, r#"{"hook_event_name":"SessionStart","session_id":"abc-123"}"#);
        assert_eq!(
            start,
            HookReport {
                state: Some(HookState::Idle),
                session_id: Some("abc-123".into()),
                detail: None,
                subagent: false,
                background: false,
            }
        );

        let prompt = parse_payload("claude", None, r#"{"hook_event_name":"UserPromptSubmit","session_id":"abc-123"}"#);
        assert_eq!(prompt.state, Some(HookState::Working));

        let tool = parse_payload(
            "claude-openrouter",
            None,
            r#"{"hook_event_name":"PreToolUse","session_id":"abc-123","tool_name":"mcp__playwright__browser_click"}"#,
        );
        assert_eq!(tool.state, Some(HookState::Working));
        assert_eq!(tool.detail.as_deref(), Some("Using browser_click"));

        let perm = parse_payload("claude", None, r#"{"hook_event_name":"PermissionRequest","tool_name":"Bash"}"#);
        assert_eq!(perm.state, Some(HookState::NeedsYou));
        assert_eq!(perm.detail.as_deref(), Some("Asks to use Bash"));

        let notif = parse_payload(
            "claude",
            None,
            r#"{"hook_event_name":"Notification","notification_type":"elicitation_dialog","message":"Pick one"}"#,
        );
        assert_eq!(notif.state, Some(HookState::NeedsYou));
        assert_eq!(notif.detail.as_deref(), Some("Pick one"));

        let stop = parse_payload("claude", Some("Stop"), r#"{"session_id":"abc-123"}"#);
        assert_eq!(stop.state, Some(HookState::Done));
        assert_eq!(stop.session_id.as_deref(), Some("abc-123"));
    }

    #[test]
    fn parses_agy_lifecycle_payloads() {
        let start = parse_payload("agy", None, r#"{"hookEventName":"SessionStart","conversationId":"conv-9"}"#);
        assert_eq!(start.state, Some(HookState::Idle));
        assert_eq!(start.session_id.as_deref(), Some("conv-9"));

        let pre = parse_payload("agy-openrouter", Some("PreInvocation"), r#"{"conversationId":"conv-9"}"#);
        assert_eq!(pre.state, Some(HookState::Working));

        let post = parse_payload(
            "agy",
            None,
            r#"{"hookEventName":"PostToolUse","conversationId":"conv-9","toolCall":{"name":"run_command"}}"#,
        );
        assert_eq!(post.state, Some(HookState::Working));
        assert_eq!(post.detail.as_deref(), Some("Used run_command"));

        let stop = parse_payload(
            "agy",
            None,
            r#"{"hookEventName":"Stop","conversationId":"conv-9","terminationReason":"NO_TOOL_CALL"}"#,
        );
        assert_eq!(stop.state, Some(HookState::Done));
        assert_eq!(stop.detail.as_deref(), Some("Finished"));
        assert!(!stop.background);

        let bg_stop = parse_payload(
            "agy",
            None,
            r#"{"hookEventName":"Stop","conversationId":"conv-9","terminationReason":"NO_TOOL_CALL","fullyIdle":false}"#,
        );
        assert_eq!(bg_stop.state, Some(HookState::Working));
        assert_eq!(bg_stop.detail.as_deref(), Some("Working in background"));
        assert!(bg_stop.background);

        let canceled_stop = parse_payload(
            "agy",
            None,
            r#"{"hookEventName":"Stop","conversationId":"conv-9","terminationReason":"USER_CANCELED","fullyIdle":false}"#,
        );
        assert_eq!(canceled_stop.state, Some(HookState::Done));
        assert_eq!(canceled_stop.detail.as_deref(), Some("Stopped"));
        assert!(!canceled_stop.background);

        let child = parse_payload(
            "agy",
            None,
            r#"{"hookEventName":"SessionStart","conversationId":"sub-1","parentConversationId":"conv-9"}"#,
        );
        assert!(child.subagent);
        assert_eq!(child.session_id.as_deref(), Some("sub-1"));

        let nested = parse_payload(
            "agy",
            None,
            r#"{"hookEventName":"PreInvocation","conversationId":"sub-2","nestingDepth":1}"#,
        );
        assert!(nested.subagent);

        assert_eq!(stdout_response("agy"), Some("{}"));
        assert_eq!(stdout_response("claude"), None);
    }

    #[test]
    fn malformed_or_unsafe_payloads_never_panic() {
        assert_eq!(parse_payload("claude", Some("Stop"), "not json").state, Some(HookState::Done));
        let bad_id = parse_payload("claude", None, r#"{"hook_event_name":"SessionStart","session_id":"../evil"}"#);
        assert_eq!(bad_id.session_id, None);
    }

    #[test]
    fn claude_settings_merges_hooks_and_non_secret_env() {
        let mut env = Map::new();
        env.insert("ANTHROPIC_BASE_URL".into(), json!("https://openrouter.ai/api"));
        let text = claude_settings_json("claude-openrouter", Some(env));
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["env"]["ANTHROPIC_BASE_URL"], "https://openrouter.ai/api");
        assert!(
            v["hooks"]["SessionStart"][0]["hooks"][0]["command"]
                .as_str()
                .unwrap()
                .contains("knx hook claude-openrouter")
        );
        assert!(v["hooks"]["PermissionRequest"].is_array());
    }

    #[test]
    fn agy_hooks_never_hook_pre_tool_use_and_guard_outside_kynex() {
        let text = agy_hooks_json();
        let v: Value = serde_json::from_str(&text).unwrap();
        assert!(v["kynex-status"]["PreToolUse"].is_null());
        assert!(v["kynex-status"]["SessionStart"].is_array());
        assert!(v["kynex-status"]["Stop"].is_array());
        let win = agy_hook_command("Stop", true);
        assert!(win.contains("KYNEX_HOOK_BIN") && win.contains("hook agy Stop") && win.contains("echo {}"));
        let unix = agy_hook_command("Stop", false);
        assert!(unix.contains("KYNEX_HOOK_BIN") && unix.contains("hook agy Stop") && unix.contains("printf '{}\\n'"));
    }

    #[test]
    fn merge_agy_hooks_preserves_user_hooks_and_invalid_json() {
        let dir = std::env::temp_dir().join(format!("kynex-agy-hooks-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let file = dir.join("hooks.json");

        // Fresh creation.
        merge_agy_hooks_file(&file, agy_hooks_entry(true)).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert!(v["kynex-status"]["Stop"].is_array());

        // Preserves existing user hooks.
        std::fs::write(&file, r#"{"my-lint":{"Stop":[{"command":"echo {}"}]}}"#).unwrap();
        merge_agy_hooks_file(&file, agy_hooks_entry(false)).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert!(v["my-lint"]["Stop"].is_array());
        assert!(v["kynex-status"]["Stop"].is_array());

        // Never clobbers broken JSON.
        std::fs::write(&file, "{ invalid").unwrap();
        assert!(merge_agy_hooks_file(&file, agy_hooks_entry(true)).is_err());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "{ invalid");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
