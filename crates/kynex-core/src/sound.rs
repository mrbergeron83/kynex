//! Short sounds for notes and toasts: the OS's own, never blocking and never failing
//! visibly. `Windows`: `MessageBeep` (the user's sound scheme); `macOS`: system sounds;
//! elsewhere the sound theme through `canberra-gtk-play`, else `paplay`.

/// Which sound. A later one outranks an earlier one when both are due at once.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Sound {
    /// A session finished.
    Done,
    /// A session needs your answer.
    NeedsYou,
}

/// Plays `sound` and returns at once. On macOS it must be called from the main thread
/// (the UI thread is one); elsewhere from anywhere.
pub fn play(sound: Sound) {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Diagnostics::Debug::MessageBeep;
        use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONASTERISK, MB_ICONEXCLAMATION};
        let kind = match sound {
            Sound::Done => MB_ICONASTERISK,
            Sound::NeedsYou => MB_ICONEXCLAMATION,
        };
        // SAFETY: no pointers; the beep is queued by the OS.
        unsafe { MessageBeep(kind) };
    }
    #[cfg(target_os = "macos")]
    {
        use objc2_app_kit::NSSound;
        use objc2_foundation::NSString;
        let name = match sound {
            Sound::Done => "Glass",
            Sound::NeedsYou => "Funk",
        };
        if let Some(s) = NSSound::soundNamed(&NSString::from_str(name)) {
            s.play();
        }
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let (id, file) = match sound {
            Sound::Done => ("complete", "complete.oga"),
            Sound::NeedsYou => ("dialog-warning", "dialog-warning.oga"),
        };
        // Waiting on a thread so the player doesn't linger as a zombie.
        std::thread::spawn(move || {
            let quiet = |cmd: &mut std::process::Command| {
                cmd.stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null());
                crate::process::detach(cmd)
            };
            let mut canberra = std::process::Command::new("canberra-gtk-play");
            let started = quiet(canberra.args(["-i", id]));
            let child = started.or_else(|_| {
                let path = format!("/usr/share/sounds/freedesktop/stereo/{file}");
                quiet(std::process::Command::new("paplay").arg(path))
            });
            if let Ok(mut child) = child {
                let _ = child.wait();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn needs_you_outranks_done() {
        assert!(Sound::NeedsYou > Sound::Done);
    }
}
