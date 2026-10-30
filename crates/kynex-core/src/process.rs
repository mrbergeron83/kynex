//! Child processes live no longer than Kynex, however it ends (closed, killed,
//! crashed, or replaced by a hot-patch rebuild). Otherwise a `claude` mid-turn
//! keeps working with nobody watching, and a restarted Kynex resumes the same
//! session beside it.
//!
//! - Windows: Kynex puts itself in a job object that kills every process in it
//!   when its last handle closes, which the OS does when Kynex exits. Children
//!   join the job by default; apps Kynex merely opens (VS Code, Explorer, a
//!   terminal window) break away from it with [`detach`].
//! - Linux: each child asks for SIGKILL when Kynex dies ([`dies_with_parent`]).
//! - macOS has no such hook; a child there notices when its stdin closes.

use std::process::{Child, Command};

/// Ties every child process Kynex starts from now on to Kynex's own lifetime
/// (on Windows; elsewhere see [`dies_with_parent`]). Call once, at startup.
pub fn tie_children_to_kynex() -> Result<(), String> {
    #[cfg(windows)]
    {
        windows::kill_on_exit()
    }
    #[cfg(not(windows))]
    {
        Ok(())
    }
}

/// Makes `cmd`'s process die with Kynex. On Windows the job object already does
/// this; on Linux it asks the kernel for SIGKILL when Kynex's thread that
/// spawned it ends (spawn from a long-lived thread); on macOS it's a no-op.
pub fn dies_with_parent(cmd: &mut Command) -> &mut Command {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: prctl is async-signal-safe and touches no memory of ours.
        unsafe {
            cmd.pre_exec(|| {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    cmd
}

/// Starts `cmd`'s process as the head of a group [`kill_tree`] can end whole:
/// for launchers like `npx`, whose real work runs in a process of its own.
pub fn own_group(cmd: &mut Command) -> &mut Command {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd
}

/// Ends `child` and every process it started, then reaps it. Started with
/// [`own_group`] on Unix; on Windows `taskkill /T` follows the parent links.
pub fn kill_tree(child: &mut Child) {
    // Gone already: its id may belong to someone else by now.
    if !matches!(child.try_wait(), Ok(None)) {
        return;
    }
    let pid = child.id().to_string();
    #[cfg(windows)]
    let mut cmd = {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut c = Command::new("taskkill");
        c.args(["/PID", &pid, "/T", "/F"]).creation_flags(CREATE_NO_WINDOW);
        c
    };
    #[cfg(not(windows))]
    let mut cmd = {
        let mut c = Command::new("kill");
        c.args(["-KILL", "--", &format!("-{pid}")]);
        c
    };
    let quiet = cmd.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status();
    if !quiet.is_ok_and(|s| s.success()) {
        let _ = child.kill();
    }
    let _ = child.wait();
}

/// Starts `cmd` as an app of its own that outlives Kynex (an editor, a file
/// manager): out of Kynex's job on Windows, plain `spawn` elsewhere.
pub fn detach(cmd: &mut Command) -> std::io::Result<Child> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        // Breaking away fails if a job Kynex was started in forbids it; then the
        // app stays tied to that job, which is still better than not opening.
        match cmd.creation_flags(CREATE_BREAKAWAY_FROM_JOB).spawn() {
            Ok(child) => Ok(child),
            Err(_) => cmd.creation_flags(0).spawn(),
        }
    }
    #[cfg(not(windows))]
    {
        cmd.spawn()
    }
}

#[cfg(windows)]
mod windows {
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_BREAKAWAY_OK, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation, SetInformationJobObject,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    pub fn kill_on_exit() -> Result<(), String> {
        let err = |what: &str| format!("Couldn't {what}: {}", std::io::Error::last_os_error());
        // SAFETY: plain Win32 calls on a handle we own. The job handle is never
        // closed: the OS closes it when Kynex exits, which is what kills the job.
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return Err(err("create a job object"));
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_BREAKAWAY_OK;
            let ok = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                (&raw const info).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if ok == 0 {
                return Err(err("set up the job object"));
            }
            if AssignProcessToJobObject(job, GetCurrentProcess()) == 0 {
                return Err(err("join the job object"));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Setup succeeds and children still start. That a tied child dies with its
    /// parent and a detached one doesn't was checked by hand, running the test
    /// binary outside cargo: cargo runs tests in its own job, which forbids
    /// breaking away.
    #[test]
    fn children_still_start_once_tied() {
        tie_children_to_kynex().unwrap();
        let (program, args): (&str, &[&str]) =
            if cfg!(windows) { ("cmd", &["/c", "exit", "0"]) } else { ("sh", &["-c", "exit 0"]) };
        let status = dies_with_parent(Command::new(program).args(args)).status().unwrap();
        assert!(status.success());
        let mut opened = detach(Command::new(program).args(args)).unwrap();
        assert!(opened.wait().unwrap().success());
    }

    #[test]
    fn a_tree_dies_with_its_head() {
        // A shell that starts a long sleep of its own, as `npx` starts `node`.
        let (program, args): (&str, &[&str]) = if cfg!(windows) {
            ("cmd", &["/c", "ping -n 30 127.0.0.1 >nul"])
        } else {
            ("sh", &["-c", "sleep 30; true"])
        };
        let mut child = own_group(Command::new(program).args(args)).spawn().unwrap();
        let start = std::time::Instant::now();
        kill_tree(&mut child);
        assert!(child.try_wait().unwrap().is_some());
        assert!(start.elapsed().as_secs() < 10);
    }
}
