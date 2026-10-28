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
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Glide {
    /// Pixels still to scroll; positive is down.
    remaining: f32,
    /// How quickly it closes the gap, in seconds: about 95% of the way in
    /// three of these.
    time_constant: f32,
}

impl Default for Glide {
    /// A jump across the transcript: brisk.
    fn default() -> Self {
        Self::new(0.05)
    }
}

impl Glide {
    pub const fn new(time_constant: f32) -> Self {
        Self { remaining: 0., time_constant }
    }

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
        let part = 1. - (-dt.max(0.) / self.time_constant).exp();
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

    /// Pixels still to scroll.
    pub fn remaining(&self) -> f32 {
        self.remaining
    }

    /// Stops where it is: the view hit an end, or you grabbed it.
    pub fn stop(&mut self) {
        self.remaining = 0.;
    }
}

/// Mouse-wheel turns made smooth, for every scroll area at once: a notch
/// becomes a browser-like distance that glides out over a few frames, as
/// pixels. Signs follow the wheel: positive scrolls back towards the top
/// (or the left).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Wheel {
    x: Glide,
    y: Glide,
    /// Turns that land on the next frame without gliding.
    now: (f32, f32),
}

impl Default for Wheel {
    fn default() -> Self {
        Self { x: Glide::new(Self::TIME_CONSTANT), y: Glide::new(Self::TIME_CONSTANT), now: (0., 0.) }
    }
}

impl Wheel {
    /// Pixels per wheel line: Windows' usual three-line notch scrolls 120px,
    /// about what browsers and VS Code do.
    pub const LINE: f32 = 40.;
    /// Mostly there in ~120ms, settled in ~250ms.
    const TIME_CONSTANT: f32 = 0.04;
    /// Windows' "one screen at a time" reports a huge line count; cap a turn.
    const MAX_TURN: f32 = 2000.;

    /// Adds a wheel turn of `lines` (x, y). It glides when `glide`; otherwise,
    /// and for turns under a line (touchpads, free-spinning wheels, already
    /// smooth), it lands whole on the next step.
    pub fn push(&mut self, lines: (f32, f32), glide: bool) {
        let px = |l: f32| (l * Self::LINE).clamp(-Self::MAX_TURN, Self::MAX_TURN);
        let (x, y) = (px(lines.0), px(lines.1));
        if glide && lines.0.abs().max(lines.1.abs()) >= 1. {
            self.x.push(x);
            self.y.push(y);
        } else {
            // A turn the other way drops what was still gliding.
            if self.x.remaining * x < 0. {
                self.x.stop();
            }
            if self.y.remaining * y < 0. {
                self.y.stop();
            }
            self.now.0 += x;
            self.now.1 += y;
        }
    }

    /// The pixels (x, y) to scroll this frame, `dt` seconds after the last.
    pub fn step(&mut self, dt: f32) -> (f32, f32) {
        let now = std::mem::take(&mut self.now);
        (now.0 + self.x.step(dt), now.1 + self.y.step(dt))
    }

    pub fn is_moving(&self) -> bool {
        self.x.is_moving() || self.y.is_moving() || self.now != (0., 0.)
    }

    /// Drops whatever was still to scroll: you clicked or grabbed something.
    pub fn stop(&mut self) {
        *self = Self::default();
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

    /// Runs a wheel at 120 Hz for `seconds`, returning the distance covered.
    fn wheel_for(w: &mut Wheel, seconds: f32) -> (f32, f32) {
        (0..(seconds * 120.).round() as usize).map(|_| w.step(1. / 120.)).fold((0., 0.), |a, s| (a.0 + s.0, a.1 + s.1))
    }

    #[test]
    fn a_notch_glides_a_browser_distance() {
        let mut w = Wheel::default();
        w.push((0., -3.), true);
        let (_, first) = w.step(1. / 120.);
        assert!(first < 0. && first > -40., "glides rather than jumps: {first}");
        let (x, y) = wheel_for(&mut w, 0.3);
        assert_eq!(x, 0.);
        assert!((first + y + 120.).abs() < 1e-3, "{}", first + y);
        assert!(!w.is_moving());
    }

    #[test]
    fn small_turns_and_reduced_motion_land_at_once() {
        let mut w = Wheel::default();
        w.push((0., 0.25), true);
        assert_eq!(w.step(1. / 120.), (0., 10.));
        w.push((-3., 0.), false);
        assert_eq!(w.step(1. / 120.), (-120., 0.));
        assert!(!w.is_moving());
    }

    #[test]
    fn notches_add_up_and_a_page_turn_is_capped() {
        let mut w = Wheel::default();
        w.push((0., 3.), true);
        w.push((0., 3.), true);
        assert!((wheel_for(&mut w, 0.5).1 - 240.).abs() < 1e-3);
        w.push((0., -4_294_967_295. / 120.), false);
        assert_eq!(w.step(0.).1, -Wheel::MAX_TURN);
        w.push((0., 3.), true);
        w.stop();
        assert!(!w.is_moving());
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
