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
    fn the_agent_speaks_acp_on_stdio() {
        let cmd = command(Path::new("C:/work"));
        let args: Vec<_> = cmd.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(args, ["agent", "stdio"]);
        assert_eq!(cmd.get_current_dir(), Some(Path::new("C:/work")));
    }
}
