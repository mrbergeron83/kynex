//! Where Kynex keeps its files.

use std::path::{Path, PathBuf};

/// The OS's per-user data dir plus `kynex`: `%APPDATA%\kynex` on Windows,
/// `~/Library/Application Support/kynex` on macOS, `$XDG_DATA_HOME/kynex` (or
/// `~/.local/share/kynex`) on Linux. `KYNEX_DATA_DIR` overrides it, to run a test
/// instance without touching your real state.
pub fn data_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("KYNEX_DATA_DIR") {
        return PathBuf::from(dir);
    }
    dirs::data_dir().unwrap_or_else(std::env::temp_dir).join("kynex")
}

/// The folder Chat sessions run in.
pub fn chat_dir() -> PathBuf {
    data_dir().join("chat")
}

/// A folder's last component, for tab titles; the whole path if it has none.
pub fn project_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string())
}
