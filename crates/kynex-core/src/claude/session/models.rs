//! The models the logged-in account can use, as the CLI reports them.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::Stdio;

use serde_json::{Value, json};

use super::*;

/// A model the logged-in account can use, as the CLI reports it.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ModelInfo {
    /// What to pass to `--model`: an alias (`opus`), a full name, or `default`.
    pub value: String,
    pub name: String,
    pub description: String,
    /// `--effort` levels it takes; empty if it has no effort setting.
    pub efforts: Vec<String>,
}

/// What the CLI reports in the stream-json `initialize` handshake when started
/// in `cwd`: models, slash commands, agents and so on. Sends no message, so it
/// costs nothing. Blocks for a second or two; call it off the UI thread.
/// Asks the CLI as the user set it up ([`crate::provider::Launch::Anthropic`]).
pub(crate) fn initialize(cwd: &Path) -> Result<Value, String> {
    let mut cmd = claude_command(cwd, &crate::provider::Launch::Anthropic);
    cmd.args(["-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child =
        crate::process::dies_with_parent(&mut cmd).spawn().map_err(|e| format!("failed to start `claude`: {e}"))?;
    let request = json!({ "type": "control_request", "request_id": INIT_ID, "request": { "subtype": "initialize" } });
    let sent = child.stdin.as_mut().is_some_and(|s| writeln!(s, "{request}").and_then(|_| s.flush()).is_ok());
    let mut response = None;
    if sent {
        for line in BufReader::new(child.stdout.take().unwrap()).lines() {
            let Ok(line) = line else { break };
            let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
            if v["type"] == "control_response" {
                response = Some(v);
                break;
            }
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    let mut v = response.ok_or("claude didn't answer the initialize request")?;
    Ok(v["response"]["response"].take())
}

/// Asks the CLI which models this account can use. Blocks; see [`initialize`].
/// Only for when there's no saved list yet: sessions report it when they start.
pub fn list_models() -> Result<Vec<ModelInfo>, String> {
    let v = initialize(&std::env::temp_dir())?;
    let models = models_from_init(&v);
    if models.is_empty() {
        return Err("no models in the initialize response".into());
    }
    Ok(models)
}

/// The models a session last reported, saved so the picker has them at once
/// after a restart. Empty if there's none. Blocks on a small file read.
pub fn saved_models() -> Vec<ModelInfo> {
    let path = crate::paths::data_dir().join("models.json");
    std::fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

/// Saves the models for [`saved_models`]. Blocks on a small file write.
pub fn save_models(models: &[ModelInfo]) -> Result<(), String> {
    let dir = crate::paths::data_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("Couldn't create {}: {e}", dir.display()))?;
    project::write_json(&dir.join("models.json"), &models)
}

/// The models in an `initialize` response.
pub(super) fn models_from_init(v: &Value) -> Vec<ModelInfo> {
    v["models"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| {
            Some(ModelInfo {
                value: m["value"].as_str()?.to_string(),
                name: m["displayName"].as_str().unwrap_or("").to_string(),
                description: m["description"].as_str().unwrap_or("").to_string(),
                efforts: m["supportedEffortLevels"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|e| e.as_str().map(str::to_string))
                    .collect(),
            })
        })
        .collect()
}
