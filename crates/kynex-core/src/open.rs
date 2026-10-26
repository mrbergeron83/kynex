//! Opening a project folder in another app.

use std::ffi::OsStr;
use std::path::Path;
use std::process::Command;

use crate::wsl;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Opener {
    Code,
    Explorer,
    Terminal,
}

/// Opens `path` with `opener`. WSL projects open VS Code over Remote - WSL and
/// the terminal inside the distro.
pub fn open_project(opener: Opener, path: &Path) -> Result<(), String> {
    if let Some(w) = wsl::parse(path) {
        return match opener {
            Opener::Code => {
                let remote = format!("wsl+{}", w.distro);
                open_with("code", &["--remote".as_ref(), remote.as_ref(), w.path.as_ref()])
            }
            Opener::Explorer => open_with("explorer", &[path.as_os_str()]),
            Opener::Terminal => {
                open_with("wt", &["wsl.exe".as_ref(), "-d".as_ref(), w.distro.as_ref(), "--cd".as_ref(), w.path.as_ref()])
            }
        };
    }
    match opener {
        Opener::Code => open_with("code", &[path.as_os_str()]),
        Opener::Explorer if cfg!(target_os = "windows") => open_with("explorer", &[path.as_os_str()]),
        Opener::Explorer if cfg!(target_os = "macos") => open_with("open", &[path.as_os_str()]),
        Opener::Explorer => open_with("xdg-open", &[path.as_os_str()]),
        Opener::Terminal if cfg!(target_os = "windows") => open_with("wt", &["-d".as_ref(), path.as_os_str()]),
        Opener::Terminal if cfg!(target_os = "macos") => {
            open_with("open", &["-a".as_ref(), "Terminal".as_ref(), path.as_os_str()])
        }
        Opener::Terminal => open_with("x-terminal-emulator", &["--working-directory".as_ref(), path.as_os_str()]),
    }
}

fn open_with(program: &str, args: &[&OsStr]) -> Result<(), String> {
    // `code` is a .cmd shim on Windows, so go through cmd to resolve it.
    let result = if cfg!(windows) && program == "code" {
        Command::new("cmd").arg("/c").arg(program).args(args).spawn()
    } else {
        Command::new(program).args(args).spawn()
    };
    result.map(drop).map_err(|e| format!("Couldn't launch {program}: {e}"))
}
