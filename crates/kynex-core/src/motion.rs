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

/// Smooth scrolling: a mouse-wheel notch adds its distance, and each frame
/// moves part of what's left, so the view glides there and slows as it
/// arrives. Time-based: the same path at 60 and 240 Hz.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Glide {
    /// Pixels still to scroll; positive is down.
    remaining: f32,
}

impl Glide {
    /// How quickly it closes the gap: about 95% of the way in three of these.
    const TIME_CONSTANT: f32 = 0.05;

    /// Adds a wheel notch's (or a jump's) distance, in pixels.
    pub fn push(&mut self, distance: f32) {
        // Turning the wheel the other way drops what was left the old way.
        if self.remaining * distance < 0. {
            self.remaining = 0.;
        }
        self.remaining += distance;
    }

    /// The pixels to scroll this frame, `dt` seconds after the last.
    pub fn step(&mut self, dt: f32) -> f32 {
        let part = 1. - (-dt.max(0.) / Self::TIME_CONSTANT).exp();
        let mut moved = self.remaining * part;
        // The last half pixel lands at once rather than creeping.
        if (self.remaining - moved).abs() < 0.5 {
            moved = self.remaining;
        }
        self.remaining -= moved;
        moved
    }

    pub fn is_moving(&self) -> bool {
        self.remaining != 0.
    }

    /// Stops where it is: the view hit an end, or you grabbed it.
    pub fn stop(&mut self) {
        self.remaining = 0.;
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

    /// Runs a glide for `seconds` at `hz`, returning the distance covered.
    fn glide_for(push: f32, seconds: f32, hz: f32) -> f32 {
        let mut g = Glide::default();
        g.push(push);
        let frames = (seconds * hz).round() as usize;
        (0..frames).map(|_| g.step(1. / hz)).sum()
    }

    #[test]
    fn glides_the_same_at_any_refresh_rate() {
        // Whole numbers of 60 Hz frames, so both rates run for the same time.
        for seconds in [2. / 60., 3. / 60., 6. / 60.] {
            let (slow, fast) = (glide_for(100., seconds, 60.), glide_for(100., seconds, 240.));
            assert!((slow - fast).abs() < 1., "{seconds}s: {slow} at 60 Hz, {fast} at 240 Hz");
        }
        // Eases out: most of the way early, then slows.
        let early = glide_for(100., 0.05, 240.);
        assert!(early > 55. && early < 75., "{early}");
    }

    #[test]
    fn glides_all_the_way_then_rests() {
        let mut g = Glide::default();
        g.push(120.);
        let mut total = 0.;
        for _ in 0..240 {
            total += g.step(1. / 240.);
        }
        assert!((total - 120.).abs() < 1e-3, "{total}");
        assert!(!g.is_moving());
        assert_eq!(g.step(1. / 240.), 0.);
    }

    #[test]
    fn turning_back_drops_the_rest_and_stop_stops() {
        let mut g = Glide::default();
        g.push(100.);
        g.step(1. / 60.);
        g.push(-40.);
        let total: f32 = (0..120).map(|_| g.step(1. / 60.)).sum();
        assert!((total + 40.).abs() < 1e-3, "{total}");
        g.push(80.);
        g.push(80.);
        g.stop();
        assert!(!g.is_moving());
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
