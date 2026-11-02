//! Runs the installed `grok` CLI (Grok Build) for a Grok session.
//!
//! A session's process is `grok agent stdio`, which speaks the Agent Client
//! Protocol (JSON-RPC over stdin / stdout). It stays up across messages like
//! `claude` does, until the session closes or sleeps. [`acp`] turns that
//! protocol into the stream-json events the session already draws, and the
//! session's own messages back into it. [`log`] reads Grok's record of a
//! session for the sessions list and a reopened transcript.

pub mod acp;
pub mod log;
pub mod setup;
mod usage;

use std::path::{Path, PathBuf};
use std::process::Command;

/// Where the `grok` binary is: on `PATH`, else `~/.grok/bin`.
pub fn program() -> PathBuf {
    let exe = if cfg!(windows) { "grok.exe" } else { "grok" };
    on_path(exe).unwrap_or_else(|| {
        dirs::home_dir()
            .map(|h| h.join(".grok").join("bin").join(exe))
            .filter(|p| p.is_file())
            .unwrap_or_else(|| PathBuf::from("grok"))
    })
}

/// Whether a `grok` binary was found. Looked for once, and again after [`recheck`].
pub fn installed() -> bool {
    match FOUND.load(std::sync::atomic::Ordering::Relaxed) {
        0 => {
            let found = program().is_file();
            FOUND.store(if found { 2 } else { 1 }, std::sync::atomic::Ordering::Relaxed);
            found
        }
        n => n == 2,
    }
}

/// 0 not looked for yet, 1 missing, 2 found.
static FOUND: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// Looks for `grok` again on the next [`installed`]: after an install.
pub fn recheck() {
    FOUND.store(0, std::sync::atomic::Ordering::Relaxed);
}

fn on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|dir| dir.join(name)).find(|p| p.is_file())
}

/// Grok's models as a session last reported them, saved so the picker has
/// them at once after a restart. Empty if there's none. Blocks on a small file read.
pub fn saved_models() -> Vec<crate::session::ModelInfo> {
    let path = crate::paths::data_dir().join("grok_models.json");
    std::fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

/// Saves the models for [`saved_models`]. Blocks on a small file write.
pub fn save_models(models: &[crate::session::ModelInfo]) -> Result<(), String> {
    let dir = crate::paths::data_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("Couldn't create {}: {e}", dir.display()))?;
    crate::project::write_json(&dir.join("grok_models.json"), &models)
}

/// Asks Grok which models it offers: `initialize`, then `session/new`, which reports
/// them without a message (a session waits for the first message before opening
/// one, so until then the picker has none). Starts `grok` once and stops it, in
/// `cwd`, a folder that isn't a project of yours: Grok keeps an empty session of
/// it in its own history. Blocks, up to about half a minute.
pub fn list_models(cwd: &Path) -> Result<Vec<crate::session::ModelInfo>, String> {
    use std::io::{BufRead, BufReader, Write};
    use std::process::Stdio;
    use std::sync::mpsc::channel;
    use std::time::Duration;

    let mut cmd = command(cwd);
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
    let mut child =
        crate::process::dies_with_parent(&mut cmd).spawn().map_err(|e| format!("couldn't start grok: {e}"))?;
    let (Some(mut stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
        let _ = child.kill();
        return Err("couldn't talk to grok".into());
    };
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line)
                && tx.send(v).is_err()
            {
                break;
            }
        }
    });
    // Sends a request and waits for its answer, skipping the notifications between.
    let mut ask = |id: u64, method: &str, params: serde_json::Value| -> Result<serde_json::Value, String> {
        let line = serde_json::json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        writeln!(stdin, "{line}").and_then(|_| stdin.flush()).map_err(|e| format!("couldn't talk to grok: {e}"))?;
        loop {
            let v = rx.recv_timeout(Duration::from_secs(25)).map_err(|_| "grok didn't answer".to_string())?;
            if v["id"].as_u64() == Some(id) {
                if let Some(e) = v["error"]["message"].as_str().or(v["error"]["data"].as_str()) {
                    return Err(e.to_string());
                }
                return Ok(v["result"].clone());
            }
        }
    };
    let result = ask(1, "initialize", acp::initialize_params()).and_then(|_| {
        let opened = ask(
            2,
            "session/new",
            serde_json::json!({
                "cwd": cwd.to_string_lossy(),
                "mcpServers": [],
                "_meta": { "yoloMode": false, "autoMode": false },
            }),
        )?;
        let list = models_of(&opened);
        if list.is_empty() { Err("grok reported no models".to_string()) } else { Ok(list) }
    });
    let _ = child.kill();
    let _ = child.wait();
    result
}

/// The models in a `session/new` result.
fn models_of(opened: &serde_json::Value) -> Vec<crate::session::ModelInfo> {
    let mut models = usage::Models::default();
    models.read(&opened["models"]);
    models.list
}

/// Grok's agent in `cwd`, speaking ACP on stdio. The model, mode and session
/// go over the protocol ([`acp::Acp`]), not on the command line.
pub fn command(cwd: &Path) -> Command {
    let mut cmd = crate::cli::command(program());
    cmd.current_dir(cwd);
    // Updates are the user's to run (`grok update`), not something to do under a session.
    cmd.env("GROK_DISABLE_AUTOUPDATER", "1");
    cmd.args(["agent", "stdio"]);
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_models_a_new_session_reports() {
        // The shape `grok agent stdio` sends in `session/new`'s result.
        let opened = serde_json::json!({ "sessionId": "s1", "models": {
            "currentModelId": "grok-4.7",
            "availableModels": [
                { "modelId": "grok-4.7", "name": "Grok 4.7", "description": "The latest",
                  "_meta": { "totalContextTokens": 256000 } },
                { "modelId": "grok-4.7-build-fast" },
                { "modelId": "grok-4.6" },
                { "modelId": "grok-4.5" },
            ] } });
        let ids: Vec<_> = models_of(&opened).into_iter().map(|m| m.value).collect();
        assert_eq!(ids, ["grok-4.7", "grok-4.7-build-fast", "grok-4.6", "grok-4.5"]);
        assert!(models_of(&serde_json::json!({ "sessionId": "s1" })).is_empty());
    }

    /// Grok lists its models without a message. Run with `cargo test -- --ignored grok_lists`.
    #[test]
    #[ignore = "needs the grok CLI, signed in"]
    fn grok_lists_its_models_live() {
        let list = list_models(&std::env::temp_dir()).unwrap();
        assert!(list.iter().any(|m| m.value.starts_with("grok-")), "{list:?}");
    }

    #[test]
    fn the_agent_speaks_acp_on_stdio() {
        let cmd = command(Path::new("C:/work"));
        let args: Vec<_> = cmd.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(args, ["agent", "stdio"]);
        assert_eq!(cmd.get_current_dir(), Some(Path::new("C:/work")));
    }
}
