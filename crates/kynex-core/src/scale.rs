//! The UI zoom (Ctrl+= / Ctrl+- / Ctrl+0): steps like a browser's, saved with
//! the app's state. The app turns it into gpui's rem size, so text, icons and
//! spacing scale together (#134).

/// The zoom levels, as a share of normal size.
pub const STEPS: [f32; 11] = [0.67, 0.75, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 2.5];

/// Normal size.
pub const NORMAL: f32 = 1.0;

/// The next step up (`dir` > 0) or down from `zoom`, staying within the steps.
/// A level between steps (a hand-edited state file) goes to the nearest one that way.
pub fn step(zoom: f32, dir: i32) -> f32 {
    let zoom = clamp(zoom);
    if dir > 0 {
        STEPS.iter().copied().find(|s| *s > zoom + 0.001).unwrap_or(STEPS[STEPS.len() - 1])
    } else {
        STEPS.iter().rev().copied().find(|s| *s < zoom - 0.001).unwrap_or(STEPS[0])
    }
}

/// `zoom` within the smallest and largest steps; normal size if it isn't a number.
pub fn clamp(zoom: f32) -> f32 {
    if zoom.is_finite() { zoom.clamp(STEPS[0], STEPS[STEPS.len() - 1]) } else { NORMAL }
}

/// "110%".
pub fn label(zoom: f32) -> String {
    format!("{:.0}%", clamp(zoom) * 100.)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_up_and_down_and_stops_at_the_ends() {
        assert_eq!(step(NORMAL, 1), 1.1);
        assert_eq!(step(NORMAL, -1), 0.9);
        assert_eq!(step(2.5, 1), 2.5);
        assert_eq!(step(0.67, -1), 0.67);
        // Between steps: the nearest one in that direction.
        assert_eq!(step(1.2, 1), 1.25);
        assert_eq!(step(1.2, -1), 1.1);
    }

    #[test]
    fn odd_values_and_labels() {
        assert_eq!(clamp(f32::NAN), NORMAL);
        assert_eq!(clamp(9.), 2.5);
        assert_eq!(label(1.1), "110%");
        assert_eq!(label(0.67), "67%");
    }
}
