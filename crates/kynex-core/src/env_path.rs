//! PATH for an app started from Finder or the Dock.
//!
//! macOS gives such an app launchd's PATH (`/usr/bin:/bin:/usr/sbin:/sbin`), not
//! the one the user's shell sets up, so a `claude`, `pwsh` or `code` installed
//! with Homebrew or npm wouldn't be found. [`fix`] asks the login shell for its
//! PATH once at startup, as editors like VS Code and Zed do. Other platforms
//! hand GUI apps the user's PATH already, and keep theirs.

use std::ffi::OsString;
use std::path::PathBuf;

/// Folders Homebrew and other installers use, in case the shell doesn't say.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const FALLBACK: [&str; 3] = ["/opt/homebrew/bin", "/usr/local/bin", "/opt/homebrew/sbin"];

/// Marks the PATH in the shell's output, which startup files may add to.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const MARK: &str = "__KYNEX_PATH__";

/// Puts the login shell's PATH in this process's environment, on macOS only.
/// Blocks for as long as the shell takes to start, at most a few seconds.
///
/// # Safety
/// Sets an environment variable: call it at the start of `main`, before any
/// other thread runs.
pub unsafe fn fix() {
    #[cfg(target_os = "macos")]
    {
        let current = std::env::var_os("PATH").unwrap_or_default();
        let shell = login_shell_path();
        let merged = merge(shell.as_deref(), &current, &FALLBACK);
        // SAFETY: the caller promises no other thread is running yet.
        unsafe { std::env::set_var("PATH", merged) };
    }
}

/// The PATH `$SHELL` sets up as an interactive login shell, or `None` if it
/// fails or takes too long.
#[cfg(target_os = "macos")]
fn login_shell_path() -> Option<String> {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    let shell = std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/bin/zsh".into());
    let mut child = Command::new(shell)
        .args(["-l", "-i", "-c", &format!("printf '%s' \"{MARK}$PATH{MARK}\"")])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() < Duration::from_secs(3) => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let mut out = String::new();
    std::io::Read::read_to_string(&mut child.stdout.take()?, &mut out).ok()?;
    between_marks(&out).map(str::to_string)
}

/// The text between the first two [`MARK`]s.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn between_marks(out: &str) -> Option<&str> {
    let (_, rest) = out.split_once(MARK)?;
    let (path, _) = rest.split_once(MARK)?;
    (!path.trim().is_empty()).then_some(path)
}

/// The shell's PATH first, then what the process had, then the fallback
/// folders that exist; each folder once.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn merge(shell: Option<&str>, current: &std::ffi::OsStr, fallback: &[&str]) -> OsString {
    let mut dirs: Vec<PathBuf> = Vec::new();
    let shell = shell.map(|s| std::env::split_paths(s).collect::<Vec<_>>()).unwrap_or_default();
    let fallback = fallback.iter().map(PathBuf::from).filter(|p| p.is_dir());
    for dir in shell.into_iter().chain(std::env::split_paths(current)).chain(fallback) {
        if !dir.as_os_str().is_empty() && !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    std::env::join_paths(dirs).unwrap_or_else(|_| current.to_os_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_path_among_startup_noise() {
        let out = format!("Welcome!\n{MARK}/opt/homebrew/bin:/usr/bin{MARK}\nbye");
        assert_eq!(between_marks(&out), Some("/opt/homebrew/bin:/usr/bin"));
        assert_eq!(between_marks("no marks"), None);
        assert_eq!(between_marks(&format!("{MARK}{MARK}")), None);
    }

    #[test]
    fn shell_path_comes_first_without_repeats() {
        let sep = if cfg!(windows) { ";" } else { ":" };
        let here = std::env::temp_dir();
        let here = here.to_str().unwrap();
        let current = format!("/usr/bin{sep}/bin");
        let merged = merge(Some(&format!("/opt/x{sep}/usr/bin")), current.as_ref(), &[here, "/no/such/dir"]);
        let dirs: Vec<PathBuf> = std::env::split_paths(&merged).collect();
        let want: Vec<PathBuf> = ["/opt/x", "/usr/bin", "/bin", here].iter().map(PathBuf::from).collect();
        assert_eq!(dirs, want);
        // No shell answer: the process's PATH stays as it was, plus what exists.
        let merged = merge(None, current.as_ref(), &[]);
        assert_eq!(merged, OsString::from(current));
    }
}
