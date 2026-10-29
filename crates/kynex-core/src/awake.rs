//! Keeping the computer awake while Claude works, so long turns and background
//! tasks aren't cut off by idle sleep. Only idle *system* sleep is held off:
//! the display still dims and locks, and lid-close or a manual Sleep still win.
//!
//! - Windows: a power request (`PowerSetRequest(PowerRequestSystemRequired)`),
//!   not `SetThreadExecutionState`, which is cleared when its thread ends.
//! - macOS: an IOKit `PreventUserIdleSystemSleep` assertion.
//! - Linux: `systemd-inhibit --what=idle:sleep` as a child, holding logind's lock
//!   until it's killed; without systemd, nothing.
//!
//! The OS lets go of each of them if Kynex dies.

use serde::{Deserialize, Serialize};

/// The Keep awake setting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum KeepAwake {
    Off,
    /// While a session is mid-turn or has background tasks running.
    #[default]
    WhileWorking,
    /// As long as Kynex runs.
    Always,
}

impl KeepAwake {
    pub const ALL: [Self; 3] = [Self::Off, Self::WhileWorking, Self::Always];

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::WhileWorking => "While Claude is working or background tasks run",
            Self::Always => "Always, while Kynex is open",
        }
    }

    /// Whether to hold the computer awake, given whether any session is
    /// `working` (mid-turn, or with background tasks running).
    pub fn wanted(self, working: bool) -> bool {
        match self {
            Self::Off => false,
            Self::WhileWorking => working,
            Self::Always => true,
        }
    }
}

/// Holds idle sleep off while it lives; dropping it lets the computer sleep again.
pub struct Awake {
    _hold: imp::Hold,
}

impl Awake {
    /// Asks the OS to keep the computer awake, saying `why` (shown by
    /// `powercfg /requests`, `pmset -g assertions`, `systemd-inhibit --list`).
    /// Blocks briefly on the OS.
    pub fn hold(why: &str) -> Result<Self, String> {
        imp::Hold::new(why).map(|_hold| Self { _hold })
    }
}

#[cfg(windows)]
mod imp {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Power::{
        PowerClearRequest, PowerCreateRequest, PowerRequestSystemRequired, PowerSetRequest,
    };
    use windows_sys::Win32::System::Threading::{
        POWER_REQUEST_CONTEXT_SIMPLE_STRING, REASON_CONTEXT, REASON_CONTEXT_0,
    };

    /// `POWER_REQUEST_CONTEXT_VERSION`, which windows-sys doesn't carry.
    const CONTEXT_VERSION: u32 = 0;

    pub struct Hold(HANDLE);

    impl Hold {
        pub fn new(why: &str) -> Result<Self, String> {
            let mut reason: Vec<u16> = why.encode_utf16().chain([0]).collect();
            let context = REASON_CONTEXT {
                Version: CONTEXT_VERSION,
                Flags: POWER_REQUEST_CONTEXT_SIMPLE_STRING,
                Reason: REASON_CONTEXT_0 { SimpleReasonString: reason.as_mut_ptr() },
            };
            // SAFETY: `context` and its string outlive the call, which copies them.
            let handle = unsafe { PowerCreateRequest(&context) };
            if handle.is_null() || handle == INVALID_HANDLE_VALUE {
                return Err(format!("couldn't keep the computer awake: {}", std::io::Error::last_os_error()));
            }
            // SAFETY: `handle` is the power request just created.
            if unsafe { PowerSetRequest(handle, PowerRequestSystemRequired) } == 0 {
                let e = std::io::Error::last_os_error();
                unsafe { CloseHandle(handle) };
                return Err(format!("couldn't keep the computer awake: {e}"));
            }
            Ok(Self(handle))
        }
    }

    impl Drop for Hold {
        fn drop(&mut self) {
            // SAFETY: the request was set in `new` and is released once, here.
            unsafe {
                PowerClearRequest(self.0, PowerRequestSystemRequired);
                CloseHandle(self.0);
            }
        }
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use std::ffi::c_void;

    use objc2_foundation::NSString;

    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IOPMAssertionCreateWithName(kind: *const c_void, level: u32, name: *const c_void, id: *mut u32) -> i32;
        fn IOPMAssertionRelease(id: u32) -> i32;
    }

    /// `kIOPMAssertionLevelOn`.
    const LEVEL_ON: u32 = 255;

    pub struct Hold(u32);

    impl Hold {
        pub fn new(why: &str) -> Result<Self, String> {
            // NSString is toll-free bridged to CFStringRef.
            let kind = NSString::from_str("PreventUserIdleSystemSleep");
            let name = NSString::from_str(why);
            let mut id = 0;
            // SAFETY: both strings live across the call, which retains what it keeps.
            let status = unsafe {
                IOPMAssertionCreateWithName(
                    &*kind as *const NSString as *const c_void,
                    LEVEL_ON,
                    &*name as *const NSString as *const c_void,
                    &mut id,
                )
            };
            if status != 0 {
                return Err(format!("couldn't keep the computer awake (IOKit error {status:#x})"));
            }
            Ok(Self(id))
        }
    }

    impl Drop for Hold {
        fn drop(&mut self) {
            // SAFETY: the assertion was created in `new` and is released once, here.
            unsafe { IOPMAssertionRelease(self.0) };
        }
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use std::process::{Child, Command, Stdio};

    pub struct Hold(Child);

    impl Hold {
        /// Spawn from a long-lived thread: the child dies with the thread that started it.
        pub fn new(why: &str) -> Result<Self, String> {
            let mut cmd = Command::new("systemd-inhibit");
            cmd.args([
                "--what=idle:sleep",
                "--who=Kynex",
                &format!("--why={why}"),
                "--mode=block",
                "sleep",
                "infinity",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
            let child = crate::process::dies_with_parent(&mut cmd)
                .spawn()
                .map_err(|e| format!("couldn't keep the computer awake: systemd-inhibit: {e}"))?;
            Ok(Self(child))
        }
    }

    impl Drop for Hold {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
mod imp {
    pub struct Hold;

    impl Hold {
        pub fn new(_why: &str) -> Result<Self, String> {
            Err("keeping the computer awake isn't supported on this system".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn holds_only_when_the_setting_says_so() {
        assert!(!KeepAwake::Off.wanted(true));
        assert!(!KeepAwake::WhileWorking.wanted(false));
        assert!(KeepAwake::WhileWorking.wanted(true));
        assert!(KeepAwake::Always.wanted(false));
        assert_eq!(KeepAwake::default(), KeepAwake::WhileWorking);
    }

    #[cfg(windows)]
    #[test]
    fn a_power_request_is_taken_and_released() {
        let hold = Awake::hold("Kynex test").expect("power request");
        drop(hold);
    }
}
