//! Output configuration through `kscreen-doctor` (part of Plasma).
//!
//! Reading uses `kscreen-doctor -j`; changes use `kscreen-doctor output.<name>.<prop>.<value>`.
//! Kept behind small functions so it can be replaced by native `kde_output_management_v2`
//! without touching callers.

use serde::Deserialize;
use tokio::process::Command;

use crate::{KwinError, Result};

#[derive(Debug, Clone, Deserialize)]
struct Doc {
    outputs: Vec<RawOutput>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawOutput {
    name: String,
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    current_mode_id: String,
    pos: Pos,
    #[serde(default = "one")]
    scale: f64,
    #[serde(default)]
    rotation: u32,
    #[serde(default)]
    modes: Vec<RawMode>,
}

fn one() -> f64 {
    1.0
}

#[derive(Debug, Clone, Copy, Deserialize)]
struct Pos {
    x: i32,
    y: i32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawMode {
    id: String,
    refresh_rate: f64,
    size: Size,
}

#[derive(Debug, Clone, Copy, Deserialize)]
struct Size {
    width: u32,
    height: u32,
}

/// One output mode.
#[derive(Debug, Clone, PartialEq)]
pub struct Mode {
    pub id: String,
    pub width: u32,
    pub height: u32,
    pub refresh_hz: f64,
}

/// Snapshot of one output's configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct OutputState {
    pub name: String,
    pub enabled: bool,
    pub x: i32,
    pub y: i32,
    pub scale: f64,
    pub rotation: u32,
    pub current_mode: Option<Mode>,
    pub modes: Vec<Mode>,
}

impl OutputState {
    /// Logical size (mode size divided by scale).
    pub fn logical_size(&self) -> Option<(f64, f64)> {
        let m = self.current_mode.as_ref()?;
        let s = if self.scale > 0.0 { self.scale } else { 1.0 };
        Some((f64::from(m.width) / s, f64::from(m.height) / s))
    }

    /// Picks the mode with exactly this size and the highest refresh rate not above `max_hz + 0.5`.
    pub fn best_mode(&self, width: u32, height: u32, max_hz: f64) -> Option<&Mode> {
        self.modes
            .iter()
            .filter(|m| m.width == width && m.height == height && m.refresh_hz <= max_hz + 0.5)
            .max_by(|a, b| a.refresh_hz.total_cmp(&b.refresh_hz))
    }
}

/// Parses `kscreen-doctor -j` output.
pub fn parse(json: &str) -> Result<Vec<OutputState>> {
    let doc: Doc =
        serde_json::from_str(json).map_err(|e| KwinError::Kscreen(format!("bad JSON: {e}")))?;
    Ok(doc
        .outputs
        .into_iter()
        .map(|o| {
            let modes: Vec<Mode> = o
                .modes
                .iter()
                .map(|m| Mode {
                    id: m.id.clone(),
                    width: m.size.width,
                    height: m.size.height,
                    refresh_hz: m.refresh_rate,
                })
                .collect();
            let current_mode = modes.iter().find(|m| m.id == o.current_mode_id).cloned();
            OutputState {
                name: o.name,
                enabled: o.enabled,
                x: o.pos.x,
                y: o.pos.y,
                scale: o.scale,
                rotation: o.rotation,
                current_mode,
                modes,
            }
        })
        .collect())
}

/// Reads all outputs.
pub async fn list() -> Result<Vec<OutputState>> {
    let out = Command::new("kscreen-doctor").arg("-j").output().await?;
    if !out.status.success() {
        return Err(KwinError::Kscreen(
            String::from_utf8_lossy(&out.stderr).into_owned(),
        ));
    }
    parse(&String::from_utf8_lossy(&out.stdout))
}

/// Reads one output by name.
pub async fn get(name: &str) -> Result<Option<OutputState>> {
    Ok(list().await?.into_iter().find(|o| o.name == name))
}

/// A requested change to one output.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Change {
    pub mode_id: Option<String>,
    pub position: Option<(i32, i32)>,
    pub scale: Option<f64>,
}

impl Change {
    /// The `kscreen-doctor` arguments for this change.
    pub fn args(&self, output: &str) -> Vec<String> {
        let mut args = Vec::new();
        if let Some(m) = &self.mode_id {
            args.push(format!("output.{output}.mode.{m}"));
        }
        if let Some(s) = self.scale {
            args.push(format!("output.{output}.scale.{}", format_scale(s)));
        }
        if let Some((x, y)) = self.position {
            // kscreen-doctor cannot parse negative coordinates; KWin normalizes layouts so the
            // top-left output starts at 0,0 anyway.
            args.push(format!(
                "output.{output}.position.{},{}",
                x.max(0),
                y.max(0)
            ));
        }
        args
    }

    pub fn is_empty(&self) -> bool {
        self.mode_id.is_none() && self.position.is_none() && self.scale.is_none()
    }
}

fn format_scale(s: f64) -> String {
    let s = (s * 100.0).round() / 100.0;
    let mut t = format!("{s:.2}");
    while t.ends_with('0') {
        t.pop();
    }
    if t.ends_with('.') {
        t.pop();
    }
    t
}

/// Applies a change.
pub async fn apply(output: &str, change: &Change) -> Result<()> {
    if change.is_empty() {
        return Ok(());
    }
    let out = Command::new("kscreen-doctor")
        .args(change.args(output))
        .output()
        .await?;
    if !out.status.success() {
        return Err(KwinError::Kscreen(
            String::from_utf8_lossy(&out.stderr).into_owned(),
        ));
    }
    Ok(())
}

/// Default scale for a panel: target ~150 logical dpi, rounded to 0.25, clamped to 1..=3.
pub fn default_scale(dpi: f64) -> f64 {
    if !(dpi.is_finite() && dpi > 0.0) {
        return 1.0;
    }
    ((dpi / 150.0) * 4.0).round().clamp(4.0, 12.0) / 4.0
}

/// Default position for a new output: to the right of the right-most existing output,
/// top-aligned with it.
pub fn default_position(existing: &[OutputState]) -> (i32, i32) {
    existing
        .iter()
        .filter(|o| o.enabled)
        .filter_map(|o| {
            let (w, _) = o.logical_size()?;
            Some((o.x + w.round() as i32, o.y))
        })
        .max_by_key(|(x, _)| *x)
        .unwrap_or((0, 0))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{"features":1,"outputs":[
      {"name":"Virtual-SpikeA","enabled":true,"currentModeId":"1","pos":{"x":760,"y":1440},"scale":1.5,"rotation":1,
       "modes":[{"id":"1","name":"1920x1200@60","refreshRate":60,"size":{"height":1200,"width":1920}},
                {"id":"2","name":"1920x1200@30","refreshRate":29.95,"size":{"height":1200,"width":1920}}]},
      {"name":"DP-1","enabled":true,"currentModeId":"3","pos":{"x":0,"y":0},"scale":1,"rotation":1,
       "modes":[{"id":"3","name":"3440x1440@175","refreshRate":174.96,"size":{"height":1440,"width":3440}}]}
    ],"screen":{}}"#;

    #[test]
    fn parses_kscreen_doctor_json() {
        let outs = parse(SAMPLE).unwrap();
        assert_eq!(outs.len(), 2);
        let v = &outs[0];
        assert_eq!(v.name, "Virtual-SpikeA");
        assert_eq!((v.x, v.y), (760, 1440));
        assert_eq!(v.logical_size(), Some((1280.0, 800.0)));
        assert_eq!(v.best_mode(1920, 1200, 60.0).unwrap().id, "1");
        assert_eq!(v.best_mode(1920, 1200, 30.0).unwrap().id, "2");
        assert!(v.best_mode(800, 600, 60.0).is_none());
    }

    #[test]
    fn default_position_is_right_of_rightmost() {
        let outs = parse(SAMPLE).unwrap();
        assert_eq!(default_position(&outs[1..]), (3440, 0));
        assert_eq!(default_position(&outs), (3440, 0));
        assert_eq!(default_position(&[]), (0, 0));
    }

    #[test]
    fn default_scale_from_dpi() {
        assert_eq!(default_scale(224.0), 1.5);
        assert_eq!(default_scale(96.0), 1.0);
        assert_eq!(default_scale(320.0), 2.25);
        assert_eq!(default_scale(1000.0), 3.0);
        assert_eq!(default_scale(f64::NAN), 1.0);
    }

    #[test]
    fn change_args() {
        let c = Change {
            mode_id: Some("1".into()),
            position: Some((-5, 10)),
            scale: Some(1.5),
        };
        assert_eq!(
            c.args("Virtual-X"),
            vec![
                "output.Virtual-X.mode.1",
                "output.Virtual-X.scale.1.5",
                "output.Virtual-X.position.0,10"
            ]
        );
        assert_eq!(format_scale(2.0), "2");
        assert_eq!(format_scale(1.25), "1.25");
    }
}
