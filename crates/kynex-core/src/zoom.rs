//! The image viewer's zoom and pan: fitting a picture to the window, zooming
//! about the pointer, and keeping it from being dragged out of sight.
//! Sizes are in logical pixels.

/// How far out and in the viewer goes, as a share of the image's own size.
pub const MIN: f32 = 0.05;
pub const MAX: f32 = 16.;
/// One press of + or −.
pub const STEP: f32 = 1.25;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Zoom {
    /// The image at 100 %.
    pub image: (f32, f32),
    /// The area it's shown in.
    pub area: (f32, f32),
    pub scale: f32,
    /// Where the image's centre is, from the area's centre.
    pub pan: (f32, f32),
}

impl Zoom {
    /// An image fitted to `area`.
    pub fn new(image: (f32, f32), area: (f32, f32)) -> Self {
        let mut z = Self { image, area, scale: 1., pan: (0., 0.) };
        z.fit();
        z
    }

    /// The scale that shows all of the image; small images stay at 100 %.
    pub fn fit_scale(&self) -> f32 {
        let (iw, ih) = self.image;
        let (aw, ah) = self.area;
        if iw <= 0. || ih <= 0. || aw <= 0. || ah <= 0. {
            return 1.;
        }
        (aw / iw).min(ah / ih).min(1.)
    }

    pub fn fitted(&self) -> bool {
        (self.scale - self.fit_scale()).abs() < 1e-4 && self.pan == (0., 0.)
    }

    /// The whole image, centred.
    pub fn fit(&mut self) {
        self.scale = self.fit_scale();
        self.pan = (0., 0.);
    }

    /// The image at its own size, about the centre.
    pub fn actual(&mut self) {
        self.zoom_at(1. / self.scale, (0., 0.));
    }

    /// The area changed size (the window did). A fitted image stays fitted.
    pub fn resize(&mut self, area: (f32, f32)) {
        let fitted = self.fitted();
        self.area = area;
        if fitted {
            self.fit();
        } else {
            self.clamp();
        }
    }

    /// Zooms by `factor`, keeping the point `at` (from the area's centre) still.
    pub fn zoom_at(&mut self, factor: f32, at: (f32, f32)) {
        if !factor.is_finite() || factor <= 0. {
            return;
        }
        let new = (self.scale * factor).clamp(MIN.min(self.fit_scale()), MAX);
        let k = new / self.scale;
        self.pan = (at.0 - (at.0 - self.pan.0) * k, at.1 - (at.1 - self.pan.1) * k);
        self.scale = new;
        self.clamp();
    }

    /// Drags the image by `by`.
    pub fn pan_by(&mut self, by: (f32, f32)) {
        self.pan = (self.pan.0 + by.0, self.pan.1 + by.1);
        self.clamp();
    }

    /// An image smaller than the area stays centred; a bigger one can't be
    /// dragged past its edges.
    fn clamp(&mut self) {
        let slack = |image: f32, area: f32| ((image * self.scale - area) / 2.).max(0.);
        let (sx, sy) = (slack(self.image.0, self.area.0), slack(self.image.1, self.area.1));
        self.pan = (self.pan.0.clamp(-sx, sx), self.pan.1.clamp(-sy, sy));
    }

    /// Where to draw the image in the area: left, top, width, height, on whole
    /// pixels so it stays sharp.
    pub fn rect(&self) -> (f32, f32, f32, f32) {
        let (w, h) = (self.image.0 * self.scale, self.image.1 * self.scale);
        let x = self.area.0 / 2. + self.pan.0 - w / 2.;
        let y = self.area.1 / 2. + self.pan.1 - h / 2.;
        (x.round(), y.round(), w.round(), h.round())
    }

    /// The scale as a whole percentage, for the toolbar.
    pub fn percent(&self) -> u32 {
        (self.scale * 100.).round() as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fits_big_images_and_leaves_small_ones() {
        let z = Zoom::new((2000., 1000.), (1000., 800.));
        assert_eq!(z.scale, 0.5);
        assert_eq!(z.rect(), (0., 150., 1000., 500.));
        assert_eq!(Zoom::new((100., 50.), (1000., 800.)).scale, 1.);
        // Nothing to measure yet: shown as is.
        assert_eq!(Zoom::new((0., 0.), (1000., 800.)).scale, 1.);
    }

    #[test]
    fn zooms_about_the_pointer() {
        let mut z = Zoom::new((1000., 1000.), (1000., 1000.));
        // The pixel under the pointer, 100px right of centre, stays put.
        z.zoom_at(2., (100., 0.));
        assert_eq!(z.scale, 2.);
        assert_eq!(z.pan, (-100., 0.));
        z.zoom_at(1000., (0., 0.));
        assert_eq!(z.scale, MAX);
        z.zoom_at(0., (0., 0.));
        assert_eq!(z.scale, MAX);
    }

    #[test]
    fn small_images_stay_centred_and_big_ones_in_reach() {
        let mut z = Zoom::new((100., 100.), (1000., 1000.));
        z.pan_by((300., 0.));
        assert_eq!(z.pan, (0., 0.));
        z.zoom_at(20., (0., 0.));
        z.pan_by((5000., -5000.));
        // 1600px wide in a 1000px area: 300px either way.
        assert_eq!(z.pan, (300., -300.));
    }

    #[test]
    fn stays_fitted_through_a_resize() {
        let mut z = Zoom::new((2000., 2000.), (1000., 1000.));
        z.resize((500., 800.));
        assert_eq!(z.scale, 0.25);
        z.actual();
        assert_eq!(z.percent(), 100);
        z.resize((400., 400.));
        assert_eq!(z.scale, 1.);
        assert!(!z.fitted());
    }
}
