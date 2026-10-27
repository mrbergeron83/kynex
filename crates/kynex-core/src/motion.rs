//! Whether Kynex animates: your pick (Appearance menu), or the operating
//! system's "reduce motion" setting when you leave it to the system.

use serde::{Deserialize, Serialize};

/// The animations setting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Motion {
    /// Follow the operating system's setting.
    #[default]
    System,
    /// Always animate.
    On,
    /// Never animate: everything appears in its final state at once.
    Off,
}

impl Motion {
    pub const ALL: [Motion; 3] = [Motion::System, Motion::On, Motion::Off];

    pub fn label(self) -> &'static str {
        match self {
            Motion::System => "Animations: as the system",
            Motion::On => "Animations: on",
            Motion::Off => "Animations: off",
        }
    }

    /// Whether to hold animations back, given whether the OS asks for less motion.
    pub fn reduce(self, os_reduces: bool) -> bool {
        match self {
            Motion::System => os_reduces,
            Motion::On => false,
            Motion::Off => true,
        }
    }
}

/// Whether the operating system asks apps for less motion: Windows' "Animation
/// effects" off, macOS' "Reduce motion" on, or GNOME's animations off. Anything
/// unreadable counts as no. Blocks (it may run `defaults` or `gsettings`).
pub fn os_reduces_motion() -> bool {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{SPI_GETCLIENTAREAANIMATION, SystemParametersInfoW};
        let mut animate: i32 = 1;
        // SAFETY: SPI_GETCLIENTAREAANIMATION writes one BOOL to the pointer.
        let ok = unsafe { SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, (&raw mut animate).cast(), 0) };
        ok != 0 && animate == 0
    }
    #[cfg(target_os = "macos")]
    {
        read("defaults", &["read", "com.apple.universalaccess", "reduceMotion"]).is_some_and(|s| s == "1")
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        read("gsettings", &["get", "org.gnome.desktop.interface", "enable-animations"]).is_some_and(|s| s == "false")
    }
}

/// A command's trimmed output, if it ran and succeeded.
#[cfg(not(windows))]
fn read(program: &str, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new(program).args(args).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn your_pick_wins_over_the_system() {
        assert!(Motion::System.reduce(true));
        assert!(!Motion::System.reduce(false));
        assert!(!Motion::On.reduce(true));
        assert!(Motion::Off.reduce(false));
    }

    #[test]
    fn reads_the_system_setting() {
        // Whatever the machine says; it just mustn't fail or hang.
        let _ = os_reduces_motion();
    }

    #[test]
    fn round_trips_through_json() {
        for m in Motion::ALL {
            assert_eq!(serde_json::from_str::<Motion>(&serde_json::to_string(&m).unwrap()).unwrap(), m);
        }
    }
}
