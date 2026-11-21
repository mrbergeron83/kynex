//! xAI's Grok Build CLI (`grok`): finding the binary, reading its session summaries for the
//! sessions tree, and installing/updating it. Runs directly in an `agent_cli` terminal pane
//! with its native auto-updater intact.

pub mod log;
pub mod setup;

use std::path::PathBuf;

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

static FOUND: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// Looks for `grok` again on the next [`installed`]: after an install.
pub fn recheck() {
    FOUND.store(0, std::sync::atomic::Ordering::Relaxed);
}

fn on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|dir| dir.join(name)).find(|p| p.is_file())
}
