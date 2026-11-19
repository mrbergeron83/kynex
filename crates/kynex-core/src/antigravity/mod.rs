//! Runs the installed `agy` CLI (Google's Antigravity agent) for an
//! Antigravity session, on the user's OpenRouter key instead of a Google login.
//!
//! A session's process is `agy acp`, which speaks the Agent Client Protocol
//! like Grok's agent does; [`crate::acp`] turns it into the stream-json the
//! session draws ([`crate::acp::Agent::Antigravity`] for where it differs).
//!
//! `agy` has a model gateway for enterprises, set through `AGY_LLM_GATEWAY_*`
//! variables, that can speak OpenAI's wire protocol. Pointed at OpenRouter's
//! OpenAI-compatible API with the OpenRouter key, it lists OpenRouter's models
//! and runs on them, Gemini included, with no Google account ([`command`]).

pub mod setup;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU8, Ordering};

/// OpenRouter's OpenAI-compatible API, for `agy`'s gateway.
pub const GATEWAY_URL: &str = "https://openrouter.ai/api/v1";

/// The executable's file name.
fn exe() -> &'static str {
    if cfg!(windows) { "agy.exe" } else { "agy" }
}

/// Where the installer puts `agy`: `%LOCALAPPDATA%\agy\bin` on Windows,
/// `~/.local/bin` elsewhere.
fn install_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        dirs::data_local_dir().map(|d| d.join("agy").join("bin"))
    } else {
        dirs::home_dir().map(|h| h.join(".local").join("bin"))
    }
}

/// Where the `agy` binary is: on `PATH`, else where the installer puts it.
pub fn program() -> PathBuf {
    let exe = exe();
    on_path(exe).unwrap_or_else(|| {
        install_dir().map(|d| d.join(exe)).filter(|p| p.is_file()).unwrap_or_else(|| PathBuf::from(exe))
    })
}

/// Whether an `agy` binary was found. Looked for once, and again after [`recheck`].
pub fn installed() -> bool {
    match FOUND.load(Ordering::Relaxed) {
        0 => {
            let found = program().is_file();
            FOUND.store(if found { 2 } else { 1 }, Ordering::Relaxed);
            found
        }
        n => n == 2,
    }
}

/// 0 not looked for yet, 1 missing, 2 found.
static FOUND: AtomicU8 = AtomicU8::new(0);

/// Looks for `agy` again on the next [`installed`]: after an install.
pub fn recheck() {
    FOUND.store(0, Ordering::Relaxed);
}

fn on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|dir| dir.join(name)).find(|p| p.is_file())
}

/// The variables that put `agy` on OpenRouter with `key`: its gateway at
/// OpenRouter's OpenAI-compatible API, and the gateway's login for `agy acp`.
pub fn gateway_env(key: &str) -> [(&'static str, String); 5] {
    [
        ("AGY_LLM_GATEWAY_URL", GATEWAY_URL.to_string()),
        ("AGY_LLM_GATEWAY_WIRE_PROTOCOL", "openai".to_string()),
        ("AGY_LLM_GATEWAY_API_KEY", key.trim().to_string()),
        ("AGY_ACP_ENABLE_GATEWAY_AUTH", "1".to_string()),
        // Updates are the user's to run (`agy update`), not something to do under a session.
        ("AGY_CLI_DISABLE_AUTO_UPDATE", "1".to_string()),
    ]
}

/// Antigravity's agent in `cwd` on OpenRouter's `key`, speaking ACP on stdio.
/// The model, mode and session go over the protocol, not on the command line.
pub fn command(cwd: &Path, key: &str) -> Command {
    let mut cmd = crate::cli::command(program());
    cmd.current_dir(cwd);
    cmd.envs(crate::fresh_env::vars());
    // A Gemini API key or a Google login of the user's own would compete with the gateway.
    cmd.env_remove("GEMINI_API_KEY");
    cmd.envs(gateway_env(key));
    cmd.arg("acp");
    cmd
}

/// The models `agy` offers through OpenRouter, from `agy models` (one per line,
/// the id and the name). Blocks on the process (a network fetch).
pub fn list_models(key: &str) -> Result<Vec<crate::session::ModelInfo>, String> {
    let mut cmd = crate::cli::command(program());
    cmd.envs(crate::fresh_env::vars()).env_remove("GEMINI_API_KEY").envs(gateway_env(key)).arg("models");
    let out = cmd.output().map_err(|e| format!("couldn't start agy: {e}"))?;
    let list = models_in(&String::from_utf8_lossy(&out.stdout));
    if list.is_empty() {
        let why = crate::cli::last_line(&out.stderr).or(crate::cli::last_line(&out.stdout)).unwrap_or_default();
        return Err(format!("agy listed no models: {why}"));
    }
    Ok(list)
}

/// The models in `agy models`' output: "id<TAB>name" lines after a header.
/// `:batch` variants are left out: they answer within a day, not a session.
fn models_in(text: &str) -> Vec<crate::session::ModelInfo> {
    text.lines()
        .filter_map(|line| {
            let id = line.split('\t').next()?.trim();
            (!id.is_empty() && id.contains('/') && !id.contains(' ') && !id.ends_with(":batch")).then(|| {
                crate::session::ModelInfo {
                    value: id.to_string(),
                    name: id.to_string(),
                    description: String::new(),
                    efforts: Vec::new(),
                }
            })
        })
        .collect()
}

/// `agy`'s own folder: `~/.gemini/antigravity-cli`.
fn home() -> Option<PathBuf> {
    Some(dirs::home_dir()?.join(".gemini").join("antigravity-cli"))
}

/// The file `agy` keeps conversation `id` in (`conversations/<id>.db`), if it's there.
/// Blocks on a file system lookup.
pub fn conversation_file(id: &str) -> Option<PathBuf> {
    // An id is a UUID: anything that could leave the folder isn't one.
    if id.is_empty() || id.contains(['/', '\\', '.']) {
        return None;
    }
    home().map(|h| h.join("conversations").join(format!("{id}.db"))).filter(|p| p.is_file())
}

/// Whether `agy`'s conversations folder can be read, so a missing file means a
/// conversation is gone. Blocks on a file system lookup.
pub fn readable() -> bool {
    home().is_some_and(|h| std::fs::read_dir(h.join("conversations")).is_ok())
}

/// Deletes conversation `id` from `agy`'s records, with its working folder. `false`
/// when there was none. Blocks on the file system.
pub fn delete(id: &str) -> Result<bool, String> {
    let Some(db) = conversation_file(id) else { return Ok(false) };
    std::fs::remove_file(&db).map_err(|e| format!("couldn't delete {}: {e}", db.display()))?;
    for extra in ["db-shm", "db-wal"] {
        let _ = std::fs::remove_file(db.with_extension(extra));
    }
    if let Some(h) = home() {
        let _ = std::fs::remove_dir_all(h.join("brain").join(id));
    }
    Ok(true)
}

/// Antigravity's models as last listed, saved so the picker has them at once
/// after a restart. Empty if there's none. Blocks on a small file read.
pub fn saved_models() -> Vec<crate::session::ModelInfo> {
    let path = crate::paths::data_dir().join("antigravity_models.json");
    std::fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

/// Saves the models for [`saved_models`]. Blocks on a small file write.
pub fn save_models(models: &[crate::session::ModelInfo]) -> Result<(), String> {
    let dir = crate::paths::data_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("Couldn't create {}: {e}", dir.display()))?;
    crate::project::write_json(&dir.join("antigravity_models.json"), &models)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(cmd: &Command) -> Vec<String> {
        cmd.get_args().map(|a| a.to_string_lossy().into_owned()).collect()
    }

    fn env<'a>(cmd: &'a Command, name: &str) -> Option<Option<&'a std::ffi::OsStr>> {
        cmd.get_envs().find(|(n, _)| *n == name).map(|(_, v)| v)
    }

    #[test]
    fn the_agent_speaks_acp_through_openrouter() {
        let cmd = command(Path::new("C:/work"), " sk-or-test \n");
        assert_eq!(args(&cmd), ["acp"]);
        assert_eq!(cmd.get_current_dir(), Some(Path::new("C:/work")));
        assert_eq!(env(&cmd, "AGY_LLM_GATEWAY_URL"), Some(Some("https://openrouter.ai/api/v1".as_ref())));
        assert_eq!(env(&cmd, "AGY_LLM_GATEWAY_WIRE_PROTOCOL"), Some(Some("openai".as_ref())));
        assert_eq!(env(&cmd, "AGY_LLM_GATEWAY_API_KEY"), Some(Some("sk-or-test".as_ref())));
        assert_eq!(env(&cmd, "AGY_ACP_ENABLE_GATEWAY_AUTH"), Some(Some("1".as_ref())));
        // A Gemini key of the user's own is taken away, not passed on.
        assert_eq!(env(&cmd, "GEMINI_API_KEY"), Some(None));
    }

    #[test]
    fn reads_the_model_list() {
        let text = "Fetching available models...\n\
            google/gemini-4-hydrogen\tgoogle/gemini-4-hydrogen\n\
            google/gemini-4-hydrogen:batch\tgoogle/gemini-4-hydrogen:batch\n\
            qwen/qwen4-max\tqwen/qwen4-max\n\
            \n";
        let ids: Vec<_> = models_in(text).into_iter().map(|m| m.value).collect();
        assert_eq!(ids, ["google/gemini-4-hydrogen", "qwen/qwen4-max"]);
        assert!(models_in("").is_empty());
    }

    /// `agy` lists OpenRouter's models. Run with `cargo test -- --ignored agy_lists`
    /// with `OPENROUTER_API_KEY` set.
    #[test]
    #[ignore = "needs the agy CLI and an OpenRouter key"]
    fn agy_lists_openrouter_models_live() {
        let key = std::env::var("OPENROUTER_API_KEY").unwrap();
        let list = list_models(&key).unwrap();
        assert!(list.iter().any(|m| m.value.starts_with("google/gemini")), "{list:?}");
    }
}
