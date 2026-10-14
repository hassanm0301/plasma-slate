//! Mapping of normalized touch coordinates (0..1 over the video picture) into the
//! global logical coordinate space of the compositor.

/// A rectangle in global logical coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self { x, y, width, height }
    }

    /// Maps a normalized point into this rectangle. Input is clamped to `[0, 1]` and the result
    /// stays strictly inside the rectangle (a point exactly on the right/bottom edge would belong
    /// to the neighbouring output).
    pub fn map_normalized(&self, nx: f32, ny: f32) -> (f64, f64) {
        let clamp = |v: f32| f64::from(if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 });
        let eps = 0.01;
        let x = self.x + (clamp(nx) * self.width).min(self.width - eps).max(0.0);
        let y = self.y + (clamp(ny) * self.height).min(self.height - eps).max(0.0);
        (x, y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_corners_and_center() {
        let r = Rect::new(760.0, 1440.0, 1280.0, 800.0);
        assert_eq!(r.map_normalized(0.0, 0.0), (760.0, 1440.0));
        assert_eq!(r.map_normalized(0.5, 0.5), (1400.0, 1840.0));
        let (x, y) = r.map_normalized(1.0, 1.0);
        assert!(x < 760.0 + 1280.0 && x > 2039.0);
        assert!(y < 1440.0 + 800.0 && y > 2239.0);
    }

    #[test]
    fn clamps_out_of_range_and_nan() {
        let r = Rect::new(0.0, 0.0, 100.0, 100.0);
        assert_eq!(r.map_normalized(-1.0, f32::NAN), (0.0, 0.0));
        let (x, _) = r.map_normalized(5.0, 0.0);
        assert!(x < 100.0);
    }
}
