//! What a session's process is given so the agent in it can use `knx`: who it is and where
//! the app listens (environment variables), a `knx` that runs this very Kynex (a shim in the
//! data dir, first on its `PATH`), and a short note saying so.
//!
//! A request `knx` sends from inside a session carries that session as its caller, read from
//! the environment. The agent sets its own environment, so the caller is a convenience, not
//! proof of who asks.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The session's uid in this run.
pub const SESSION_VAR: &str = "KYNEX_SESSION";
/// The workspace folder it runs in.
pub const WORKSPACE_VAR: &str = "KYNEX_WORKSPACE_DIR";
/// The control socket's address, as `control.addr` holds it.
pub const CONTROL_VAR: &str = "KYNEX_CONTROL";

/// What the agent is told (`claude --append-system-prompt`).
pub const NOTE: &str = "You are running inside Kynex, a desktop app for Claude Code. Its `knx` \
command (on your PATH) can see and arrange the app from your shell: `knx session list`, \
`knx session read <id>`, `knx session wait <id>`, `knx workspace list`, `knx pane list`, \
`knx project list`. Your session's id is in the KYNEX_SESSION environment variable, your \
workspace folder in KYNEX_WORKSPACE_DIR. \
Use it only when the user's request calls for it.";

/// The folder of `data_dir` the shims go in.
pub fn shim_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("bin")
}

/// `s` in single quotes for `sh`.
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// The shim files that run `exe` as `knx`: (file name, content). On Windows a `knx.cmd` for
/// PowerShell and cmd, and a `knx` script for Git Bash, which doesn't run `.cmd` files by
/// their bare name; elsewhere the script.
pub fn shims(exe: &Path, windows: bool) -> Vec<(&'static str, String)> {
    let exe = exe.to_string_lossy();
    let sh = |path: &str| format!("#!/bin/sh\nexec {} \"$@\"\n", sh_quote(path));
    if windows {
        vec![("knx.cmd", format!("@echo off\r\n\"{exe}\" %*\r\n")), ("knx", sh(&exe.replace('\\', "/")))]
    } else {
        vec![("knx", sh(&exe))]
    }
}

/// Writes the shims for the running executable into `dir`, leaving files that already say
/// the same alone. Blocking.
pub fn write_shims(dir: &Path, exe: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("couldn't make {}: {e}", dir.display()))?;
    for (name, content) in shims(exe, cfg!(windows)) {
        let path = dir.join(name);
        if std::fs::read_to_string(&path).is_ok_and(|c| c == content) {
            continue;
        }
        std::fs::write(&path, &content).map_err(|e| format!("couldn't write {}: {e}", path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
        }
    }
    Ok(())
}

/// The shims' folder, written once per run for this executable; `None` if that failed (the
/// session then finds whatever `knx` its `PATH` has). Blocking the first time.
pub fn ensure_shims() -> Option<&'static Path> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| {
        let exe = std::env::current_exe().ok()?;
        let dir = shim_dir(&crate::paths::data_dir());
        match write_shims(&dir, &exe) {
            Ok(()) => Some(dir),
            Err(e) => {
                eprintln!("kynex: {e}");
                None
            }
        }
    })
    .as_deref()
}

/// `path` (a `PATH` value) with `dir` first, and not again further on.
pub fn path_with(dir: &Path, path: Option<&OsStr>) -> OsString {
    let rest = path.map(|p| std::env::split_paths(p).filter(|p| p != dir).collect::<Vec<_>>()).unwrap_or_default();
    std::env::join_paths(std::iter::once(dir.to_path_buf()).chain(rest)).unwrap_or_else(|_| dir.as_os_str().into())
}

/// The variables a session's process gets: its uid, its workspace, and the address its app
/// listens on (when it does).
pub fn session_env(uid: u64, workspace: &Path, control: Option<&str>) -> Vec<(&'static str, OsString)> {
    let mut env = vec![(SESSION_VAR, uid.to_string().into()), (WORKSPACE_VAR, workspace.as_os_str().into())];
    if let Some(addr) = control.filter(|a| !a.is_empty()) {
        env.push((CONTROL_VAR, addr.into()));
    }
    env
}

/// Where this run's app listens, as it told `control.addr`. Blocking (a small read).
pub fn control_address() -> Option<String> {
    let addr = std::fs::read_to_string(super::addr_file(&crate::paths::data_dir())).ok()?;
    Some(addr.trim().to_string()).filter(|a| !a.is_empty())
}

/// The session a `knx` run inside one comes from, from `value` (`KYNEX_SESSION`).
pub fn caller_from(value: Option<&OsStr>) -> Option<u64> {
    value?.to_str()?.trim().parse().ok()
}

/// [`caller_from`] this process's environment.
pub fn caller() -> Option<u64> {
    caller_from(std::env::var_os(SESSION_VAR).as_deref())
}

/// The address `knx` was handed, if any (`KYNEX_CONTROL`).
pub fn handed_address() -> Option<String> {
    std::env::var(CONTROL_VAR).ok().map(|a| a.trim().to_string()).filter(|a| !a.is_empty())
}

/// Forgets the variables of a session this app may have been started from (a Kynex run
/// from a session's shell): they name that session and that app, and would reach this
/// one's terminals and own `knx` calls otherwise.
///
/// # Safety
/// Changes the process environment: call it before any other thread starts.
pub unsafe fn forget_inherited() {
    for var in [SESSION_VAR, WORKSPACE_VAR, CONTROL_VAR] {
        // SAFETY: the caller guarantees no other thread runs yet.
        unsafe { std::env::remove_var(var) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shims_run_this_executable_with_every_argument() {
        let win = shims(Path::new(r"C:\Program Files\Kynex\kynex.exe"), true);
        assert_eq!(win[0], ("knx.cmd", "@echo off\r\n\"C:\\Program Files\\Kynex\\kynex.exe\" %*\r\n".to_string()));
        assert_eq!(win[1], ("knx", "#!/bin/sh\nexec 'C:/Program Files/Kynex/kynex.exe' \"$@\"\n".to_string()));
        let unix = shims(Path::new("/opt/it's here/kynex"), false);
        assert_eq!(unix, [("knx", "#!/bin/sh\nexec '/opt/it'\\''s here/kynex' \"$@\"\n".to_string())]);
    }

    #[test]
    fn shims_are_written_once_and_kept_when_unchanged() {
        let dir = std::env::temp_dir().join(format!("kynex-shims-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let exe = Path::new("/somewhere/kynex");
        write_shims(&dir, exe).unwrap();
        let (name, content) = &shims(exe, cfg!(windows))[0];
        assert_eq!(&std::fs::read_to_string(dir.join(name)).unwrap(), content);
        write_shims(&dir, exe).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join("knx")).unwrap().permissions().mode();
            assert_eq!(mode & 0o111, 0o111, "runnable");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_shims_come_first_on_path_and_only_once() {
        let dir = Path::new(if cfg!(windows) { r"C:\data\kynex\bin" } else { "/data/kynex/bin" });
        let other = if cfg!(windows) { r"C:\tools" } else { "/usr/bin" };
        let path = std::env::join_paths([Path::new(other), dir]).unwrap();
        let got: Vec<PathBuf> = std::env::split_paths(&path_with(dir, Some(&path))).collect();
        assert_eq!(got, [dir.to_path_buf(), PathBuf::from(other)]);
        let alone: Vec<PathBuf> = std::env::split_paths(&path_with(dir, None)).collect();
        assert_eq!(alone, [dir.to_path_buf()]);
    }

    #[test]
    fn a_session_is_told_who_it_is_and_where_to_ask() {
        let env = session_env(42, Path::new("/w/app"), Some(r"\\.\pipe\kynex-1"));
        assert_eq!(
            env,
            [
                (SESSION_VAR, OsString::from("42")),
                (WORKSPACE_VAR, OsString::from("/w/app")),
                (CONTROL_VAR, OsString::from(r"\\.\pipe\kynex-1")),
            ]
        );
        assert_eq!(session_env(1, Path::new("/w"), None).len(), 2, "no app listening: no address");
    }

    #[test]
    fn the_caller_comes_from_the_environment() {
        assert_eq!(caller_from(Some(OsStr::new("17"))), Some(17));
        assert_eq!(caller_from(Some(OsStr::new(" 17\n"))), Some(17));
        assert_eq!(caller_from(Some(OsStr::new("x"))), None);
        assert_eq!(caller_from(None), None);
    }
}
