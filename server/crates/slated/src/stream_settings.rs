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

/// Resolution presets, as percentages of the tablet's panel (100 = native).
pub const PRESET_PERCENTS: [u32; 5] = [100, 83, 75, 67, 50];
/// Refresh rates offered besides the tablet's own (only those below it are used).
pub const PRESET_RATES: [u32; 3] = [60, 45, 30];

impl Resolution {
    /// The preset for a percentage (`100` is `Native`).
    pub fn preset(percent: u32) -> Self {
        if percent >= 100 {
            Resolution::Native
        } else {
            Resolution::Percent(percent)
        }
    }
}

/// Preset resolutions for a panel: (setting, width, height), largest first, without duplicates.
pub fn resolution_presets(native_w: u32, native_h: u32) -> Vec<(Resolution, u32, u32)> {
    let mut out: Vec<(Resolution, u32, u32)> = Vec::new();
    for p in PRESET_PERCENTS {
        let r = Resolution::preset(p);
        let (w, h) = r.apply(native_w, native_h);
        if !out.iter().any(|&(_, ow, oh)| (ow, oh) == (w, h)) {
            out.push((r, w, h));
        }
    }
    out
}

/// Refresh rates offered for a panel refreshing at `panel_fps`, fastest first.
pub fn refresh_presets(panel_fps: u32) -> Vec<u32> {
    let mut out = vec![panel_fps.max(MIN_FPS)];
    out.extend(PRESET_RATES.iter().copied().filter(|&r| r < panel_fps));
    out
}

/// Display modes the virtual output offers in Display & Monitor settings:
/// every preset size at every preset refresh rate, as (width, height, Hz).
pub fn display_modes(native_w: u32, native_h: u32, panel_fps: u32) -> Vec<(u32, u32, u32)> {
    let rates = refresh_presets(panel_fps);
    resolution_presets(native_w, native_h)
        .into_iter()
        .flat_map(|(_, w, h)| rates.iter().map(move |&hz| (w, h, hz)))
        .collect()
}

/// Settings matching a display mode picked in Display & Monitor settings: a preset when the
/// size is one (so it scales with the tablet), otherwise the exact size. The tablet's own
/// refresh rate maps to automatic.
pub fn settings_for_mode(
    width: u32,
    height: u32,
    refresh_hz: f64,
    native_w: u32,
    native_h: u32,
    panel_fps: u32,
) -> StreamSettings {
    let resolution = resolution_presets(native_w, native_h)
        .into_iter()
        .find(|&(_, w, h)| (w, h) == (width, height))
        .map_or(Resolution::Fit(width, height), |(r, _, _)| r);
    let hz = refresh_hz.round().max(0.0) as u32;
    let fps = if hz == 0 || hz.abs_diff(panel_fps) <= 1 {
        0
    } else {
        hz.clamp(MIN_FPS, 240)
    };
    StreamSettings::new(resolution, fps)
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
    fn presets_and_display_modes() {
        let p = resolution_presets(1920, 1200);
        assert_eq!(p[0], (Resolution::Native, 1920, 1200));
        assert_eq!(p[2], (Resolution::Percent(75), 1440, 900));
        assert_eq!(p.len(), PRESET_PERCENTS.len());
        assert_eq!(refresh_presets(60), vec![60, 45, 30]);
        assert_eq!(refresh_presets(90), vec![90, 60, 45, 30]);
        assert_eq!(refresh_presets(30), vec![30]);
        let modes = display_modes(1920, 1200, 60);
        assert_eq!(modes.len(), 5 * 3);
        assert!(modes.contains(&(1440, 900, 30)));
        // Tiny panels: presets collapse onto the minimum size without duplicates.
        let tiny = resolution_presets(400, 300);
        assert!(tiny
            .windows(2)
            .all(|w| (w[0].1, w[0].2) != (w[1].1, w[1].2)));
    }

    #[test]
    fn settings_from_display_mode() {
        let s = |w, h, hz: f64| settings_for_mode(w, h, hz, 1920, 1200, 60);
        assert_eq!(
            s(1920, 1200, 60.0),
            StreamSettings::new(Resolution::Native, 0)
        );
        assert_eq!(
            s(1920, 1200, 59.95),
            StreamSettings::new(Resolution::Native, 0)
        );
        assert_eq!(
            s(1440, 900, 30.0),
            StreamSettings::new(Resolution::Percent(75), 30)
        );
        assert_eq!(
            s(1280, 720, 45.0),
            StreamSettings::new(Resolution::Fit(1280, 720), 45)
        );
        // Round trip: every offered mode maps back to settings that produce the same size.
        for (w, h, hz) in display_modes(1920, 1200, 60) {
            let st = s(w, h, f64::from(hz));
            assert_eq!(st.resolution().apply(1920, 1200), (w, h));
            assert_eq!(st.effective_fps(60), hz);
        }
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
