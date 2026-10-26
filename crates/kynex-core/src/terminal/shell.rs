//! Which shell a terminal runs, and finding it on this machine.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ShellKind {
    PowerShell,
    /// Git Bash on Windows; the user's login shell elsewhere.
    Bash,
}

impl ShellKind {
    /// PowerShell on Windows, the user's shell everywhere else.
    pub fn default_for_os() -> Self {
        if cfg!(windows) { ShellKind::PowerShell } else { ShellKind::Bash }
    }

    /// The shell that runs a fenced code block tagged `lang` (```bash, ```powershell…),
    /// or `None` when the block isn't shell commands.
    pub fn for_lang(lang: &str) -> Option<Self> {
        match lang.to_ascii_lowercase().as_str() {
            "bash" | "sh" | "shell" | "zsh" | "console" => Some(ShellKind::Bash),
            "powershell" | "pwsh" | "ps1" | "ps" => Some(ShellKind::PowerShell),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ShellKind::PowerShell => "PowerShell",
            ShellKind::Bash if cfg!(windows) => "Git Bash",
            ShellKind::Bash => "Shell",
        }
    }
}

/// A shell found on disk, ready to start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShellSpec {
    pub kind: ShellKind,
    pub program: PathBuf,
    pub args: Vec<String>,
}

/// Finds the program for `kind`.
pub fn resolve(kind: ShellKind) -> Result<ShellSpec, String> {
    let (program, args) = match kind {
        ShellKind::PowerShell => {
            let program = which("pwsh")
                .or_else(|| cfg!(windows).then(|| which("powershell")).flatten())
                .ok_or("PowerShell isn't installed (looked for pwsh on PATH)")?;
            (program, vec!["-NoLogo".to_string()])
        }
        ShellKind::Bash if cfg!(windows) => (
            git_bash().ok_or("Git Bash wasn't found. Install Git for Windows, or set CLAUDE_CODE_GIT_BASH_PATH.")?,
            vec!["--login".into(), "-i".into()],
        ),
        ShellKind::Bash => {
            let shell = std::env::var_os("SHELL").map(PathBuf::from).filter(|p| p.is_file());
            (shell.unwrap_or_else(|| PathBuf::from("/bin/bash")), vec!["-l".into()])
        }
    };
    Ok(ShellSpec { kind, program, args })
}

/// Git for Windows' bash, the one Claude Code itself uses: `CLAUDE_CODE_GIT_BASH_PATH`,
/// else next to `git` on PATH, else the default install. Never System32's WSL launcher.
fn git_bash() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("CLAUDE_CODE_GIT_BASH_PATH").map(PathBuf::from).filter(|p| p.is_file()) {
        return Some(p);
    }
    let from_git = which("git").and_then(|git| {
        // `Git\cmd\git.exe` or `Git\bin\git.exe` → `Git\bin\bash.exe`.
        let root = git.parent()?.parent()?;
        Some(root.join("bin").join("bash.exe")).filter(|p| p.is_file())
    });
    from_git.or_else(|| {
        ["ProgramFiles", "ProgramW6432", "LOCALAPPDATA"].iter().find_map(|var| {
            let base = PathBuf::from(std::env::var_os(var)?);
            let sub: &Path = if *var == "LOCALAPPDATA" {
                Path::new("Programs/Git/bin/bash.exe")
            } else {
                Path::new("Git/bin/bash.exe")
            };
            Some(base.join(sub)).filter(|p| p.is_file())
        })
    })
}

/// A program on PATH (`.exe` added on Windows).
fn which(name: &str) -> Option<PathBuf> {
    let file = if cfg!(windows) { format!("{name}.exe") } else { name.to_string() };
    std::env::split_paths(&std::env::var_os("PATH")?).map(|dir| dir.join(&file)).find(|p| p.is_file())
}

/// Whether Kynex runs with administrator (Windows) or root (elsewhere) rights.
/// Terminals inherit them. Asked once; the first call runs a process.
pub fn elevated() -> bool {
    static ELEVATED: OnceLock<bool> = OnceLock::new();
    *ELEVATED.get_or_init(|| {
        let mut cmd = if cfg!(windows) {
            // Only an elevated process may list sessions.
            let mut c = std::process::Command::new("net");
            c.arg("session");
            c
        } else {
            let mut c = std::process::Command::new("id");
            c.arg("-u");
            c
        };
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000);
        }
        match cmd.output() {
            Ok(out) if cfg!(windows) => out.status.success(),
            Ok(out) => String::from_utf8_lossy(&out.stdout).trim() == "0",
            Err(_) => false,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_fences_map_to_shells() {
        assert_eq!(ShellKind::for_lang("bash"), Some(ShellKind::Bash));
        assert_eq!(ShellKind::for_lang("sh"), Some(ShellKind::Bash));
        assert_eq!(ShellKind::for_lang("PowerShell"), Some(ShellKind::PowerShell));
        assert_eq!(ShellKind::for_lang("ps1"), Some(ShellKind::PowerShell));
        assert_eq!(ShellKind::for_lang("rust"), None);
        assert_eq!(ShellKind::for_lang(""), None);
    }

    #[test]
    fn finds_the_default_shell() {
        let spec = resolve(ShellKind::default_for_os()).unwrap();
        assert!(spec.program.is_file() || !cfg!(windows), "{spec:?}");
    }

    #[cfg(windows)]
    #[test]
    fn bash_is_never_the_wsl_launcher() {
        if let Ok(spec) = resolve(ShellKind::Bash) {
            assert!(!spec.program.to_string_lossy().to_lowercase().contains("system32"));
        }
    }
}
