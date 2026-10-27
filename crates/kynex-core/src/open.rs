//! Opening a project folder in another app, and editing a message in your editor.

use std::ffi::OsStr;
use std::path::Path;
use std::process::Command;

use crate::process::detach;
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
            Opener::Terminal => open_with(
                "wt",
                &["wsl.exe".as_ref(), "-d".as_ref(), w.distro.as_ref(), "--cd".as_ref(), w.path.as_ref()],
            ),
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

/// Ctrl+G: edits `text` in the user's editor and returns what they saved.
/// `$VISUAL` / `$EDITOR` if set (it must wait for the file to close, e.g.
/// `code --wait`), else Notepad on Windows and TextEdit on macOS. Blocks until
/// the editor closes.
pub fn edit_text(text: &str) -> Result<String, String> {
    let env = std::env::var("VISUAL").or_else(|_| std::env::var("EDITOR")).ok().filter(|e| !e.trim().is_empty());
    let mut words: Vec<String> = match env {
        Some(e) => e.split_whitespace().map(str::to_string).collect(),
        None if cfg!(target_os = "windows") => vec!["notepad".into()],
        None if cfg!(target_os = "macos") => ["open", "-W", "-n", "-t"].map(String::from).into(),
        None => return Err("Set $VISUAL or $EDITOR to edit messages in your editor.".into()),
    };
    let program = words.remove(0);
    let path = std::env::temp_dir().join(format!("kynex-message-{}.md", std::process::id()));
    std::fs::write(&path, text).map_err(|e| format!("Couldn't write {}: {e}", path.display()))?;
    // Editors like `code` are .cmd shims on Windows; cmd resolves them.
    let mut cmd = if cfg!(windows) && program != "notepad" {
        let mut c = Command::new("cmd");
        c.arg("/c").arg(&program);
        c
    } else {
        Command::new(&program)
    };
    let status = cmd.args(&words).arg(&path).status().map_err(|e| format!("Couldn't launch {program}: {e}"));
    let edited = std::fs::read_to_string(&path).map_err(|e| format!("Couldn't read {}: {e}", path.display()));
    let _ = std::fs::remove_file(&path);
    match status? {
        s if s.success() => Ok(edited?.trim_end().to_string()),
        s => Err(format!("{program} exited with {s}")),
    }
}

fn open_with(program: &str, args: &[&OsStr]) -> Result<(), String> {
    // `code` is a .cmd shim on Windows, so go through cmd to resolve it.
    // These apps are the user's, not Kynex's: they stay open when Kynex closes.
    let result = if cfg!(windows) && program == "code" {
        detach(Command::new("cmd").arg("/c").arg(program).args(args))
    } else {
        detach(Command::new(program).args(args))
    };
    result.map(drop).map_err(|e| format!("Couldn't launch {program}: {e}"))
}
