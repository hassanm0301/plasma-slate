//! Per-tablet stream settings: resolution and refresh rate.
//!
//! Lowering either reduces work on the PC (KWin compositing, capture, encode) and on the
//! tablet (decode), and lowers bandwidth. The virtual screen is created at the chosen size;
//! the tablet scales the picture up to its panel.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Smallest stream size accepted (long side).
pub const MIN_WIDTH: u32 = 320;
/// Lowest refresh rate accepted.
pub const MIN_FPS: u32 = 10;

/// Resolution choice. The tablet's aspect ratio is always kept, and the stream never exceeds the
/// tablet's native resolution (upscaling on the PC would only cost performance).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Resolution {
    /// The tablet's panel resolution.
    #[default]
    Native,
    /// Percentage of the native resolution, e.g. `75%`.
    Percent(u32),
    /// Largest size with the tablet's aspect ratio that fits in `W×H`, e.g. `1280x800`.
    Fit(u32, u32),
}

impl Resolution {
    /// Stream size for a panel of `native_w`×`native_h` (landscape), even dimensions.
    pub fn apply(self, native_w: u32, native_h: u32) -> (u32, u32) {
        let (nw, nh) = (f64::from(native_w.max(2)), f64::from(native_h.max(2)));
        let f = match self {
            Resolution::Native => 1.0,
            Resolution::Percent(p) => f64::from(p) / 100.0,
            Resolution::Fit(w, h) => {
                // Accept portrait-style input too: compare long side with long side.
                let (lw, lh) = (f64::from(w.max(h)), f64::from(w.min(h)));
                (lw / nw).min(lh / nh)
            }
        };
        let min_f = f64::from(MIN_WIDTH) / nw;
        let f = f.clamp(min_f.min(1.0), 1.0);
        let even = |v: f64| ((v.round() as u32) & !1).max(2);
        (even(nw * f), even(nh * f))
    }
}

impl fmt::Display for Resolution {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Resolution::Native => write!(f, "native"),
            Resolution::Percent(p) => write!(f, "{p}%"),
            Resolution::Fit(w, h) => write!(f, "{w}x{h}"),
        }
    }
}

impl FromStr for Resolution {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let t = s.trim().to_ascii_lowercase();
        if t.is_empty() || t == "native" || t == "auto" || t == "100%" {
            return Ok(Resolution::Native);
        }
        if let Some(p) = t.strip_suffix('%') {
            let p: u32 = p
                .trim()
                .parse()
                .map_err(|_| format!("bad percentage: {s}"))?;
            if !(10..=100).contains(&p) {
                return Err(format!("percentage must be 10–100: {s}"));
            }
            return Ok(Resolution::Percent(p));
        }
        let (w, h) = t
            .split_once(['x', '×', '*'])
            .ok_or_else(|| format!("expected native, NN% or WIDTHxHEIGHT: {s}"))?;
        let w: u32 = w.trim().parse().map_err(|_| format!("bad width: {s}"))?;
        let h: u32 = h.trim().parse().map_err(|_| format!("bad height: {s}"))?;
        if w.max(h) < MIN_WIDTH || w.max(h) > 8192 {
            return Err(format!("size out of range ({MIN_WIDTH}–8192): {s}"));
        }
        Ok(Resolution::Fit(w, h))
    }
}

/// Settings stored per tablet (and as defaults in `slated.toml`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct StreamSettings {
    /// `native`, `NN%` or `WIDTHxHEIGHT`.
    pub resolution: String,
    /// Frames per second; 0 = the tablet's refresh rate.
    pub fps: u32,
}

impl StreamSettings {
    pub fn new(resolution: Resolution, fps: u32) -> Self {
        Self {
            resolution: resolution.to_string(),
            fps,
        }
    }

    /// Validates and normalizes (e.g. `1280X800` → `1280x800`, fps clamped to 10–240 or 0).
    pub fn validated(&self) -> Result<Self, String> {
        let r: Resolution = self.resolution.parse()?;
        if self.fps != 0 && !(MIN_FPS..=240).contains(&self.fps) {
            return Err(format!("refresh rate must be {MIN_FPS}–240 Hz or 0 (auto)"));
        }
        Ok(Self::new(r, self.fps))
    }

    pub fn resolution(&self) -> Resolution {
        self.resolution.parse().unwrap_or_default()
    }

    /// Effective frame rate for a panel refreshing at `panel_hz`.
    pub fn effective_fps(&self, panel_hz: u32) -> u32 {
        let panel = panel_hz.max(MIN_FPS);
        if self.fps == 0 {
            panel
        } else {
            self.fps.clamp(MIN_FPS, panel)
        }
    }

    pub fn is_default(&self) -> bool {
        self.resolution() == Resolution::Native && self.fps == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_display() {
        assert_eq!("native".parse::<Resolution>(), Ok(Resolution::Native));
        assert_eq!("".parse::<Resolution>(), Ok(Resolution::Native));
        assert_eq!(" 75% ".parse::<Resolution>(), Ok(Resolution::Percent(75)));
        assert_eq!(
            "1280X800".parse::<Resolution>(),
            Ok(Resolution::Fit(1280, 800))
        );
        assert_eq!(
            "800x1280".parse::<Resolution>(),
            Ok(Resolution::Fit(800, 1280))
        );
        assert!("5%".parse::<Resolution>().is_err());
        assert!("100x50".parse::<Resolution>().is_err());
        assert!("big".parse::<Resolution>().is_err());
        assert_eq!(Resolution::Fit(1280, 800).to_string(), "1280x800");
        assert_eq!(Resolution::Percent(50).to_string(), "50%");
    }

    #[test]
    fn apply_keeps_aspect_and_never_upscales() {
        // MatePad T 10s panel.
        assert_eq!(Resolution::Native.apply(1920, 1200), (1920, 1200));
        assert_eq!(Resolution::Percent(75).apply(1920, 1200), (1440, 900));
        assert_eq!(Resolution::Percent(50).apply(1920, 1200), (960, 600));
        assert_eq!(Resolution::Fit(1280, 800).apply(1920, 1200), (1280, 800));
        // Different aspect: fits inside the box, keeps 16:10.
        assert_eq!(Resolution::Fit(1280, 720).apply(1920, 1200), (1152, 720));
        // Portrait-style input.
        assert_eq!(Resolution::Fit(800, 1280).apply(1920, 1200), (1280, 800));
        // Larger than native: clamped.
        assert_eq!(Resolution::Fit(3840, 2400).apply(1920, 1200), (1920, 1200));
        // Odd results become even.
        let (w, h) = Resolution::Percent(33).apply(1920, 1200);
        assert_eq!((w % 2, h % 2), (0, 0));
        // Never below the minimum.
        assert!(Resolution::Percent(10).apply(1920, 1200).0 >= MIN_WIDTH);
    }

    #[test]
    fn fps_rules() {
        let s = |fps| StreamSettings {
            resolution: "native".into(),
            fps,
        };
        assert_eq!(s(0).effective_fps(60), 60);
        assert_eq!(s(30).effective_fps(60), 30);
        assert_eq!(s(120).effective_fps(60), 60);
        assert_eq!(s(5).effective_fps(60), MIN_FPS);
        assert!(s(5).validated().is_err());
        assert!(s(0).validated().is_ok());
        assert_eq!(
            StreamSettings {
                resolution: "1280X800".into(),
                fps: 30
            }
            .validated()
            .unwrap()
            .resolution,
            "1280x800"
        );
        assert!(StreamSettings::default().is_default());
    }
}
