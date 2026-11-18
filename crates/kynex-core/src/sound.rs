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
            let played = |cmd: &mut std::process::Command| {
                quiet(cmd).and_then(|mut child| child.wait()).is_ok_and(|status| status.success())
            };
            // canberra may be missing, or there but without a sound theme: then paplay.
            if !played(std::process::Command::new("canberra-gtk-play").args(["-i", id])) {
                let path = format!("/usr/share/sounds/freedesktop/stereo/{file}");
                played(std::process::Command::new("paplay").arg(path));
            }
        });
    }
}

/// Plays the sound file at `path` and returns at once; if it can't be played (missing,
/// not a format the OS plays, no player), `fallback` plays instead. Windows plays WAV
/// files only. On macOS call it from the main thread; elsewhere from anywhere.
pub fn play_file(path: &std::path::Path, fallback: Sound) {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_FILENAME, SND_NODEFAULT};
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        // SAFETY: `wide` is NUL-terminated and outlives the call; the sound is a file name, not memory.
        let played =
            unsafe { PlaySoundW(wide.as_ptr(), std::ptr::null_mut(), SND_FILENAME | SND_ASYNC | SND_NODEFAULT) };
        if played == 0 {
            play(fallback);
        }
    }
    #[cfg(target_os = "macos")]
    {
        use objc2::AnyThread;
        use objc2_app_kit::NSSound;
        use objc2_foundation::NSString;
        let file = NSString::from_str(&path.to_string_lossy());
        let sound = NSSound::initWithContentsOfFile_byReference(NSSound::alloc(), &file, false);
        match sound {
            Some(s) if s.play() => {
                // It stops when released, so it's let go of on purpose: sounds are short and rare.
                std::mem::forget(s);
            }
            _ => play(fallback),
        }
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let path = path.to_path_buf();
        // Waiting on a thread so the player doesn't linger as a zombie.
        std::thread::spawn(move || {
            let mut cmd = std::process::Command::new("paplay");
            cmd.arg(&path)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
            let played = crate::process::detach(&mut cmd)
                .and_then(|mut child| child.wait())
                .is_ok_and(|status| status.success());
            if !played {
                play(fallback);
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
