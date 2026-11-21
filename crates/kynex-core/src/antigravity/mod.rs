//! Google's Antigravity CLI (`agy`): finding the binary, OpenRouter gateway environment
//! variables (preserving `agy`'s own auto-updater), conversation storage lookups, and setup.

pub mod setup;

use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, Ordering};

/// OpenRouter's OpenAI-compatible API, for `agy`'s gateway override.
pub const GATEWAY_URL: &str = "https://openrouter.ai/api/v1";

fn exe() -> &'static str {
    if cfg!(windows) { "agy.exe" } else { "agy" }
}

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

static FOUND: AtomicU8 = AtomicU8::new(0);

/// Looks for `agy` again on the next [`installed`]: after an install.
pub fn recheck() {
    FOUND.store(0, Ordering::Relaxed);
}

fn on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|dir| dir.join(name)).find(|p| p.is_file())
}

/// The variables that put `agy` on OpenRouter with `key`, keeping `agy`'s own auto-updater intact.
pub fn gateway_env(key: &str) -> [(&'static str, String); 4] {
    [
        ("AGY_LLM_GATEWAY_URL", GATEWAY_URL.to_string()),
        ("AGY_LLM_GATEWAY_WIRE_PROTOCOL", "openai".to_string()),
        ("AGY_LLM_GATEWAY_API_KEY", key.trim().to_string()),
        ("AGY_ACP_ENABLE_GATEWAY_AUTH", "1".to_string()),
    ]
}

/// `agy`'s own folder: `~/.gemini/antigravity-cli`.
fn home() -> Option<PathBuf> {
    Some(dirs::home_dir()?.join(".gemini").join("antigravity-cli"))
}

/// The file `agy` keeps conversation `id` in (`conversations/<id>.db`), if it's there.
/// Blocks on a file system lookup.
pub fn conversation_file(id: &str) -> Option<PathBuf> {
    if id.is_empty() || id.contains(['/', '\\', '.']) {
        return None;
    }
    home().map(|h| h.join("conversations").join(format!("{id}.db"))).filter(|p| p.is_file())
}

/// Whether `agy`'s conversations folder can be read, so a missing file means a conversation is gone.
pub fn readable() -> bool {
    home().is_some_and(|h| std::fs::read_dir(h.join("conversations")).is_ok())
}

/// Deletes conversation `id` from `agy`'s records, with its working folder. `false` when none.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gateway_env_points_to_openrouter_without_disabling_auto_update() {
        let env = gateway_env(" sk-or-test \n");
        assert!(env.iter().any(|(k, v)| *k == "AGY_LLM_GATEWAY_URL" && v == GATEWAY_URL));
        assert!(env.iter().any(|(k, v)| *k == "AGY_LLM_GATEWAY_WIRE_PROTOCOL" && v == "openai"));
        assert!(env.iter().any(|(k, v)| *k == "AGY_LLM_GATEWAY_API_KEY" && v == "sk-or-test"));
        assert!(!env.iter().any(|(k, _)| *k == "AGY_CLI_DISABLE_AUTO_UPDATE"));
    }
}
