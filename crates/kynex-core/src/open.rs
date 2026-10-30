//! Opening a project folder or a file in another app, and editing a message in your editor.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
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

/// Shows `path` in the OS file manager, selected where it can be (Explorer,
/// Finder); elsewhere its folder opens.
pub fn reveal(path: &Path) -> Result<(), String> {
    if cfg!(target_os = "windows") {
        // `/select,` takes the path in the same argument, with backslashes.
        let native = path.to_string_lossy().replace('/', "\\");
        open_with("explorer", &[format!("/select,{native}").as_ref()])
    } else if cfg!(target_os = "macos") {
        open_with("open", &["-R".as_ref(), path.as_os_str()])
    } else {
        let dir = if path.is_dir() { path } else { path.parent().unwrap_or(path) };
        open_with("xdg-open", &[dir.as_os_str()])
    }
}

/// Opens a file in the app the OS gives its type (not an editor).
pub fn open_default(path: &Path) -> Result<(), String> {
    if cfg!(target_os = "windows") {
        let native = path.to_string_lossy().replace('/', "\\");
        open_with("explorer", &[native.as_ref()])
    } else if cfg!(target_os = "macos") {
        open_with("open", &[path.as_os_str()])
    } else {
        open_with("xdg-open", &[path.as_os_str()])
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

/// Opens a file in VS Code at `line` / `col` (`code -g`) if `code` is on PATH,
/// else with the OS's default app for it (which ignores the line). Doesn't wait:
/// the editor is detached and stays open when Kynex closes. Files inside WSL
/// always go to the default opener.
pub fn open_file(path: &Path, line: Option<u32>, col: Option<u32>) -> Result<(), String> {
    if wsl::parse(path).is_none()
        && let Some(code) = find_code()
    {
        let mut target = path.as_os_str().to_os_string();
        if let Some(line) = line {
            target.push(format!(":{line}"));
            if let Some(col) = col {
                target.push(format!(":{col}"));
            }
        }
        // The full path to `code.cmd`, not `cmd /c code`: std quotes a batch
        // file's arguments safely, cmd would read `&` in a path as a command.
        return detach(Command::new(&code).arg("-g").arg(&target))
            .map(drop)
            .map_err(|e| format!("Couldn't launch VS Code: {e}"));
    }
    if cfg!(target_os = "windows") {
        // Explorer opens a file with its associated app; it wants backslashes.
        let native = path.to_string_lossy().replace('/', "\\");
        open_with("explorer", &[native.as_ref()])
    } else if cfg!(target_os = "macos") {
        open_with("open", &[path.as_os_str()])
    } else {
        open_with("xdg-open", &[path.as_os_str()])
    }
}

/// VS Code's `code` command: on PATH, or in macOS's usual spot. Blocking: touches the disk.
fn find_code() -> Option<PathBuf> {
    let names: &[&str] = if cfg!(windows) { &["code.cmd", "code.exe"] } else { &["code"] };
    let on_path = std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths).find_map(|dir| names.iter().map(|n| dir.join(n)).find(|p| p.is_file()))
    });
    let mac = || Some(PathBuf::from("/usr/local/bin/code")).filter(|p| cfg!(target_os = "macos") && p.is_file());
    on_path.or_else(mac)
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
